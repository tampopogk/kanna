//! Receipts for App Design feedback delivery (docs/specs/app-design.md §8).
//!
//! kanna-server sends each batch of queued feedback with a stable delivery
//! id. This daemon remembers what became of each id, so a server that lost
//! the answer (a timeout, its own restart) asks instead of typing the message
//! again. Receipts move to a successor with the sessions on a graceful daemon
//! upgrade; a daemon that died loses them, and the server then reports those
//! deliveries as uncertain rather than guessing.

use crate::protocol::{DesignDeliveryOutcome, DesignReceiptTransfer};
use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};

/// Enough for weeks of feedback; the oldest receipts go first.
const MAX_RECEIPTS: usize = 4096;
const MAX_INSTANCES: usize = 64;

struct Receipts {
    instance: String,
    inherited: Vec<String>,
    outcomes: HashMap<String, DesignDeliveryOutcome>,
    order: VecDeque<String>,
}

static RECEIPTS: LazyLock<Mutex<Receipts>> = LazyLock::new(|| {
    let mut bytes = [0u8; 12];
    let instance = std::fs::File::open("/dev/urandom")
        .and_then(|mut random| std::io::Read::read_exact(&mut random, &mut bytes))
        .map(|_| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        })
        .unwrap_or_else(|_| {
            format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            )
        });
    Mutex::new(Receipts {
        instance,
        inherited: Vec::new(),
        outcomes: HashMap::new(),
        order: VecDeque::new(),
    })
});

fn with<T>(f: impl FnOnce(&mut Receipts) -> T) -> T {
    let mut receipts = RECEIPTS.lock().unwrap_or_else(|poison| poison.into_inner());
    f(&mut receipts)
}

/// This daemon process's identity.
pub(crate) fn instance() -> String {
    with(|receipts| receipts.instance.clone())
}

/// Every instance whose receipts this daemon holds.
pub(crate) fn known_instances() -> Vec<String> {
    with(|receipts| {
        let mut known = receipts.inherited.clone();
        known.push(receipts.instance.clone());
        known
    })
}

pub(crate) fn outcome(delivery_id: &str) -> DesignDeliveryOutcome {
    with(|receipts| {
        receipts
            .outcomes
            .get(delivery_id)
            .cloned()
            .unwrap_or(DesignDeliveryOutcome::Unknown)
    })
}

/// Claim `delivery_id` for writing. `Err` with the recorded outcome when it
/// was claimed before: the caller answers that and writes nothing.
pub(crate) fn claim(delivery_id: &str) -> Result<(), DesignDeliveryOutcome> {
    with(|receipts| {
        if let Some(existing) = receipts.outcomes.get(delivery_id) {
            return Err(existing.clone());
        }
        receipts.record(delivery_id, DesignDeliveryOutcome::Accepted);
        Ok(())
    })
}

/// Forget a claim that wrote nothing (the agent turned out not to be free,
/// or the session was gone), so the same id can be sent again later.
pub(crate) fn release(delivery_id: &str) {
    with(|receipts| {
        if receipts.outcomes.get(delivery_id) == Some(&DesignDeliveryOutcome::Accepted) {
            receipts.outcomes.remove(delivery_id);
            receipts.order.retain(|id| id != delivery_id);
        }
    })
}

pub(crate) fn settle(delivery_id: &str, outcome: DesignDeliveryOutcome) {
    with(|receipts| receipts.record(delivery_id, outcome));
}

impl Receipts {
    fn record(&mut self, delivery_id: &str, outcome: DesignDeliveryOutcome) {
        if self
            .outcomes
            .insert(delivery_id.to_string(), outcome)
            .is_none()
        {
            self.order.push_back(delivery_id.to_string());
        }
        while self.order.len() > MAX_RECEIPTS {
            if let Some(oldest) = self.order.pop_front() {
                self.outcomes.remove(&oldest);
            }
        }
    }
}

/// What a handing-off daemon sends its successor.
pub(crate) fn export() -> DesignReceiptTransfer {
    with(|receipts| DesignReceiptTransfer {
        instances: {
            let mut instances = receipts.inherited.clone();
            instances.push(receipts.instance.clone());
            instances
        },
        receipts: receipts
            .order
            .iter()
            .filter_map(|id| Some((id.clone(), receipts.outcomes.get(id)?.clone())))
            .collect(),
    })
}

/// Adopt a predecessor's receipts.
pub(crate) fn import(transfer: DesignReceiptTransfer) {
    with(|receipts| {
        for instance in transfer.instances {
            if instance != receipts.instance && !receipts.inherited.contains(&instance) {
                receipts.inherited.push(instance);
            }
        }
        let overflow = receipts.inherited.len().saturating_sub(MAX_INSTANCES);
        receipts.inherited.drain(..overflow);
        for (id, outcome) in transfer.receipts {
            // A write still in flight on the predecessor cannot be known to
            // have finished: the successor holds it as unknown-outcome.
            let outcome = match outcome {
                DesignDeliveryOutcome::Accepted => DesignDeliveryOutcome::WriteFailed {
                    message: "the delivery was being written when the daemon handed off".into(),
                },
                outcome => outcome,
            };
            if !receipts.outcomes.contains_key(&id) {
                receipts.record(&id, outcome);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claimed_id_is_never_claimed_twice_and_a_release_frees_it() {
        let id = format!("test-{}", instance());
        assert!(claim(&id).is_ok());
        assert_eq!(claim(&id), Err(DesignDeliveryOutcome::Accepted));
        release(&id);
        assert!(claim(&id).is_ok());
        settle(&id, DesignDeliveryOutcome::Delivered);
        release(&id);
        assert_eq!(outcome(&id), DesignDeliveryOutcome::Delivered);
        assert_eq!(claim(&id), Err(DesignDeliveryOutcome::Delivered));
    }

    #[test]
    fn receipts_survive_a_handoff_and_in_flight_writes_become_uncertain() {
        let delivered = format!("delivered-{}", instance());
        let in_flight = format!("in-flight-{}", instance());
        import(DesignReceiptTransfer {
            instances: vec!["predecessor".into()],
            receipts: vec![
                (delivered.clone(), DesignDeliveryOutcome::Delivered),
                (in_flight.clone(), DesignDeliveryOutcome::Accepted),
            ],
        });
        assert_eq!(outcome(&delivered), DesignDeliveryOutcome::Delivered);
        assert!(matches!(
            outcome(&in_flight),
            DesignDeliveryOutcome::WriteFailed { .. }
        ));
        assert!(known_instances().contains(&"predecessor".to_string()));
        assert!(export().receipts.iter().any(|(id, _)| id == &delivered));
    }
}
