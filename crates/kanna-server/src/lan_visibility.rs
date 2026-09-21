//! Whether this machine is actually discoverable on the LAN, and if not, why.
//!
//! Bonjour registration and browse both fail in a way that produces no user
//! symptom beyond "the phone never finds this Mac": the supervisors below
//! retry forever, each attempt logging one warning, and the product itself had
//! no idea anything was wrong. On this developer's Mac that was 68,233
//! identical warnings across three instances over two days while every LAN
//! client silently fell back to the relay.
//!
//! The missing distinction is authorization. macOS answers a register or
//! browse for a service type the bundle did not declare — or for an app whose
//! Local Network access is off — with `kDNSServiceErr_NoAuth` (-65555). That
//! never heals by retrying at five-second intervals, and it is the one DNS-SD
//! failure with a concrete remedy a person can act on. So it is classified
//! separately, retried slowly, logged once per streak rather than once per
//! attempt, and — the part that actually reaches a user — recorded here, where
//! `/v1/status` reads it.
//!
//! This registry is process-global for the same reason
//! [`crate::workspace_commands::write_path_health`] is: the reporters are
//! supervisor threads owned by `bonjour`/`lan_discovery` and the reader is a
//! status handler, and threading a handle between them would buy nothing.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// A Bonjour operation whose health is worth reporting on its own. A desktop
/// that can advertise but not browse is half-broken in a way one aggregate
/// boolean cannot express.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Operation {
    /// `advertise` or `browse`.
    pub(crate) action: &'static str,
    /// The service type in its `NSBonjourServices` spelling.
    pub(crate) service_type: String,
}

impl Operation {
    pub(crate) fn advertise(registration_type: &str) -> Self {
        Self {
            action: "advertise",
            service_type: declared_service_type(registration_type),
        }
    }

    pub(crate) fn browse(registration_type: &str) -> Self {
        Self {
            action: "browse",
            service_type: declared_service_type(registration_type),
        }
    }
}

fn declared_service_type(registration_type: &str) -> String {
    kanna_runtime_defaults::bonjour_services::service_for(registration_type)
        .map(|service| service.service_type.to_string())
        .unwrap_or_else(|| registration_type.trim_end_matches(".local.").to_string())
}

/// Why an operation is failing. Only the authorization case has a remedy, and
/// only it is known not to recover on its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FailureKind {
    /// `kDNSServiceErr_NoAuth`: the bundle does not declare this service type,
    /// or Local Network access is off for this app.
    Unauthorized,
    /// Anything else — a responder restart, a lost connection, a poll error.
    /// Retrying at the normal interval is the right response.
    Transient,
}

impl FailureKind {
    fn state(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::Transient => "failing",
        }
    }
}

/// What the caller should do about a failure it just reported.
pub(crate) struct FailureReport {
    /// This failure is new — the operation was healthy, or was failing
    /// differently. The one attempt in a streak that earns a warning.
    pub(crate) first: bool,
    /// How many consecutive attempts have now failed this way.
    pub(crate) consecutive: u64,
}

/// One operation's current health, as `/v1/status` reports it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanVisibilityOperation {
    pub action: String,
    pub service_type: String,
    /// `ok`, `unauthorized`, or `failing`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub consecutive_failures: u64,
    /// How long this failure streak has lasted. Absent while healthy. A
    /// duration rather than a timestamp, matching
    /// [`crate::workspace_commands::WritePathHealth`] beside it in the same
    /// status payload — and this crate has no wall-clock formatter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failing_for_seconds: Option<u64>,
}

/// Whether other machines can find this one, and what to do if they cannot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanVisibility {
    /// Every reported operation is currently working. `false` the moment one
    /// is not, so a client never has to interpret the list to answer "can the
    /// phone find this Mac"; `null` when nothing has reported, which is not
    /// the same claim as "no". Reporting is wired for the macOS DNS-SD path,
    /// where the authorization refusal this exists for is raised; other
    /// platforms' discovery loops do not report yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discoverable: Option<bool>,
    /// `ok`, `unauthorized`, `failing`, or `unknown` when nothing has reported
    /// yet — a platform whose discovery path does not report, or a server too
    /// early in startup to have tried.
    pub status: String,
    /// One sentence naming the fault and its remedy. Absent while healthy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub operations: Vec<LanVisibilityOperation>,
}

#[derive(Clone, Debug)]
struct OperationHealth {
    kind: Option<FailureKind>,
    detail: Option<String>,
    consecutive: u64,
    since: Option<Instant>,
}

fn registry() -> &'static Mutex<BTreeMap<Operation, OperationHealth>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<Operation, OperationHealth>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn lock() -> std::sync::MutexGuard<'static, BTreeMap<Operation, OperationHealth>> {
    registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// This operation is working. Returns whether that is a change, so a recovery
/// can be logged without the caller tracking its own previous state.
pub(crate) fn record_ok(operation: &Operation) -> bool {
    let mut registry = lock();
    let previous = registry.insert(
        operation.clone(),
        OperationHealth {
            kind: None,
            detail: None,
            consecutive: 0,
            since: None,
        },
    );
    previous.is_some_and(|health| health.kind.is_some())
}

/// This operation just failed. `detail` is the raw failure text; the
/// authorization remedy is added here so every surface words it identically.
pub(crate) fn record_failure(
    operation: &Operation,
    kind: FailureKind,
    detail: &str,
) -> FailureReport {
    let mut registry = lock();
    let previous = registry.get(operation);
    let same_failure = previous.is_some_and(|health| {
        health.kind == Some(kind) && health.detail.as_deref() == Some(detail)
    });
    let consecutive = if same_failure {
        previous.map_or(1, |health| health.consecutive.saturating_add(1))
    } else {
        1
    };
    let since = if same_failure {
        previous.and_then(|health| health.since)
    } else {
        None
    }
    .or_else(|| Some(Instant::now()));
    registry.insert(
        operation.clone(),
        OperationHealth {
            kind: Some(kind),
            detail: Some(detail.to_string()),
            consecutive,
            since,
        },
    );
    FailureReport {
        first: !same_failure,
        consecutive,
    }
}

/// Stop reporting on an operation whose supervisor has shut down, so a
/// withdrawn advertisement does not read as a permanent fault.
pub(crate) fn forget(operation: &Operation) {
    lock().remove(operation);
}

/// What a person can actually do about an authorization refusal.
///
/// Both causes produce the identical DNS-SD code and the server cannot tell
/// them apart from inside the process, so the sentence names both rather than
/// guessing — one of them is a shipped-build bug and the other is a System
/// Settings toggle, and sending someone to the wrong one wastes the whole
/// diagnosis.
pub(crate) fn unauthorized_remedy(service_type: &str) -> String {
    format!(
        "macOS refused Bonjour for {service_type} (DNS-SD error -65555, kDNSServiceErr_NoAuth), \
         so this Mac is not discoverable on the local network. Either this build's Info.plist \
         does not list {service_type} under NSBonjourServices, or Local Network access is turned \
         off for Kanna in System Settings > Privacy & Security > Local Network."
    )
}

pub fn snapshot() -> LanVisibility {
    summarize(&lock())
}

/// The aggregate `/v1/status` reports, given every operation's health.
///
/// Pure over the map rather than reading the registry itself: the registry is
/// process-global, so a test that asserted on the aggregate would be asserting
/// on whatever sibling tests had reported.
fn summarize(registry: &BTreeMap<Operation, OperationHealth>) -> LanVisibility {
    let operations: Vec<LanVisibilityOperation> = registry
        .iter()
        .map(|(operation, health)| LanVisibilityOperation {
            action: operation.action.to_string(),
            service_type: operation.service_type.clone(),
            state: health.kind.map_or("ok", FailureKind::state).to_string(),
            detail: health.detail.clone(),
            consecutive_failures: health.consecutive,
            failing_for_seconds: health.since.map(|since| since.elapsed().as_secs()),
        })
        .collect();
    // An authorization refusal outranks a transient fault: it is the one with
    // a remedy, and reporting the other first would send someone looking at
    // the network.
    let unauthorized = registry
        .iter()
        .find(|(_, health)| health.kind == Some(FailureKind::Unauthorized));
    let failing = registry.iter().find(|(_, health)| health.kind.is_some());
    let (status, detail) = match (operations.is_empty(), unauthorized, failing) {
        (true, _, _) => ("unknown", None),
        (_, Some((operation, _)), _) => (
            "unauthorized",
            Some(unauthorized_remedy(&operation.service_type)),
        ),
        (_, None, Some((operation, health))) => (
            "failing",
            Some(format!(
                "Bonjour {} for {} is failing: {}",
                operation.action,
                operation.service_type,
                health.detail.as_deref().unwrap_or("unknown error")
            )),
        ),
        _ => ("ok", None),
    };
    LanVisibility {
        discoverable: (status != "unknown").then_some(status == "ok"),
        status: status.to_string(),
        detail,
        operations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> OperationHealth {
        OperationHealth {
            kind: None,
            detail: None,
            consecutive: 0,
            since: None,
        }
    }

    fn failed(kind: FailureKind, detail: &str) -> OperationHealth {
        OperationHealth {
            kind: Some(kind),
            detail: Some(detail.to_string()),
            consecutive: 3,
            since: Some(Instant::now()),
        }
    }

    fn book(entries: Vec<(Operation, OperationHealth)>) -> BTreeMap<Operation, OperationHealth> {
        entries.into_iter().collect()
    }

    #[test]
    fn nothing_reported_is_not_a_claim_that_the_machine_is_invisible() {
        let summary = summarize(&book(vec![]));
        assert_eq!(summary.status, "unknown");
        assert_eq!(summary.discoverable, None);
        assert!(summary.detail.is_none());
    }

    #[test]
    fn every_operation_working_reports_a_discoverable_machine() {
        let summary = summarize(&book(vec![
            (Operation::advertise("_kanna-lan._tcp.local."), healthy()),
            (Operation::browse("_kanna-lan._tcp.local."), healthy()),
        ]));
        assert_eq!(summary.status, "ok");
        assert_eq!(summary.discoverable, Some(true));
        assert!(summary.detail.is_none());
        assert_eq!(summary.operations.len(), 2);
        assert!(summary.operations.iter().all(|entry| entry.state == "ok"));
    }

    #[test]
    fn an_authorization_refusal_outranks_a_transient_fault_and_carries_the_remedy() {
        let summary = summarize(&book(vec![
            (
                Operation::advertise("_kanna-mobile._tcp.local."),
                failed(FailureKind::Transient, "responder disconnected"),
            ),
            (
                Operation::browse("_kanna-lan._tcp.local."),
                failed(FailureKind::Unauthorized, "DNS-SD error -65555"),
            ),
        ]));
        assert_eq!(summary.status, "unauthorized");
        assert_eq!(summary.discoverable, Some(false));
        let detail = summary.detail.expect("a refusal names its remedy");
        assert!(detail.contains("_kanna-lan._tcp"));
        assert!(detail.contains("NSBonjourServices"));
        assert!(detail.contains("Local Network"));
    }

    #[test]
    fn a_transient_fault_alone_reports_failing_without_the_authorization_remedy() {
        let summary = summarize(&book(vec![(
            Operation::browse("_kanna-lan._tcp.local."),
            failed(FailureKind::Transient, "responder disconnected"),
        )]));
        assert_eq!(summary.status, "failing");
        assert_eq!(summary.discoverable, Some(false));
        let detail = summary.detail.expect("a fault says what failed");
        assert!(detail.contains("responder disconnected"));
        assert!(!detail.contains("NSBonjourServices"));
    }

    #[test]
    fn an_operation_names_its_service_type_the_way_the_bundle_declares_it() {
        let operation = Operation::browse("_kanna-lan._tcp.local.");
        assert_eq!(operation.service_type, "_kanna-lan._tcp");
        assert_eq!(operation.action, "browse");
        // A type the shared table does not know still reports readably rather
        // than under a raw registration string.
        assert_eq!(
            Operation::advertise("_unknown._tcp.local.").service_type,
            "_unknown._tcp"
        );
    }

    #[test]
    fn a_repeated_identical_failure_is_only_first_once() {
        let operation = Operation::advertise("_kanna-test-repeat._tcp");
        forget(&operation);
        assert!(record_failure(&operation, FailureKind::Unauthorized, "refused").first);
        let second = record_failure(&operation, FailureKind::Unauthorized, "refused");
        assert!(!second.first);
        assert_eq!(second.consecutive, 2);
        // A different failure is news again, and its streak restarts.
        let changed = record_failure(&operation, FailureKind::Transient, "disconnected");
        assert!(changed.first);
        assert_eq!(changed.consecutive, 1);
        // Recovery is reported once, then stays quiet.
        assert!(record_ok(&operation));
        assert!(!record_ok(&operation));
        forget(&operation);
    }
}
