//! Internal subscription scheduling. Public waits, peer legs and adapters do
//! not own this policy. Defaults adopted by the manager, not measured tuning.
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;

/// A lone event should collect for the full window below, not seal early on
/// a short trailing-quiet gap. See docs/kanna-server-boundary.md.
///
/// This used to be two knobs — `quiet` (deadline resets on each new relevant
/// event) and `max_hold` (a hard cap from the first relevant event) — whose
/// deadline was `(last + quiet).min(first + max_hold)`. Shipped equal at
/// 300000/300000, `max_hold` always won the `min` and `quiet` could never
/// bind, so debouncing a steady trickle of events never actually happened:
/// every relevant event pushed `last` forward, but the deadline stayed pinned
/// to `first + max_hold` regardless. Collapsed to the one knob that was ever
/// load-bearing — an unbounded-by-count trailing-quiet window, capped only by
/// the wait's own outer timeout.
pub(super) const HOLD: Duration = Duration::from_millis(300_000);
pub(super) const ADMISSION_INTERVAL: Duration = Duration::from_millis(60_000);
/// Floor for a per-subscription override of hold/admission spacing
/// (validated at registration in `event_subscriptions::subscribe`). Without
/// one, a near-zero override would defeat the pacing this module exists to
/// provide.
pub(super) const MIN_OVERRIDE: Duration = Duration::from_millis(1_000);

#[derive(Debug)]
pub(super) struct Collection {
    last: Option<Instant>,
    urgent: bool,
    hold: Duration,
}

impl Default for Collection {
    fn default() -> Self {
        Self::new(HOLD)
    }
}

impl Collection {
    pub(super) fn new(hold: Duration) -> Self {
        Self {
            last: None,
            urgent: false,
            hold,
        }
    }

    /// Build from a subscription's own (already-validated) `hold_ms`
    /// override, falling back to the manager-adopted default when absent —
    /// including for a pre-existing row created before this override existed.
    pub(super) fn from_query(hold_ms: Option<u64>) -> Self {
        Self::new(hold_ms.map(Duration::from_millis).unwrap_or(HOLD))
    }

    pub(super) fn observe(&mut self, events: &[Value], now: Instant) {
        if !events.is_empty() {
            self.last = Some(now);
            self.urgent |= events.iter().any(urgent);
        }
    }

    /// The subscription's own trailing-quiet deadline, independent of any
    /// single native call's receiver. `None` until something relevant has
    /// been observed. A caller that chains several (up to 240s) native calls
    /// to honor a larger window reads this to size each next request and to
    /// know when it has genuinely finished, not merely run out of one call's
    /// own budget.
    pub(super) fn intrinsic_deadline(&self) -> Option<Instant> {
        self.last.map(|last| last + self.hold)
    }

    /// Capped by `receiver` (one native call's own hard budget) for sizing
    /// how long that call should sleep before rechecking — never for deciding
    /// whether the subscription's own window is satisfied; see `ready`.
    pub(super) fn deadline(&self, receiver: Instant) -> Instant {
        self.intrinsic_deadline().unwrap_or(receiver).min(receiver)
    }

    /// Whether the subscription's own criteria are genuinely satisfied:
    /// urgent, a full page, or the intrinsic trailing-quiet deadline reached.
    /// Deliberately ignores any single native call's own receiver — a caller
    /// chaining several calls to cover a window larger than one call's budget
    /// must not mistake "this call's budget ran out" for "done".
    pub(super) fn ready(&self, count: usize, capacity: i64, now: Instant) -> bool {
        count > 0
            && (self.urgent
                || count >= capacity as usize
                || self
                    .intrinsic_deadline()
                    .is_some_and(|deadline| now >= deadline))
    }
}

/// Called only on relevant structured facts. Unknown attention is conservative;
/// no provider-authored prose participates in this decision.
fn urgent(event: &Value) -> bool {
    let payload = &event["payload"];
    match event["type"].as_str() {
        Some("run.finished") => payload["status"] != "succeeded",
        Some("task.runtime_changed") => {
            payload["runtimeState"] == "waiting"
                || payload["runtimeState"] == "exited"
                || payload["notificationContext"]["providerParked"] == true
                || payload["notificationContext"]["latestRun"]["status"] == "failed"
                || payload["currentTask"]["latestRun"]["status"] == "failed"
        }
        Some(
            "task.awaiting_advance"
            | "task.blocked"
            | "task.unblocked"
            | "task.closed"
            | "task.pr_created"
            | "task.merge_signaled"
            | "task.workflow_changed"
            | "task.revision_requested",
        ) => false,
        // Includes confirmed input, failed lifecycle/handoff/teardown, provider
        // parking and future unknown attention. Watch faults seal separately.
        _ => true,
    }
}

/// Monotonic process-local gate. The record persists only that an admission
/// occurred; recovery never trusts a wall-clock timestamp or accumulates credit.
pub(super) struct Admission {
    next: Option<Instant>,
    interval: Duration,
}

impl Admission {
    pub(super) fn new(recovered: bool, interval: Duration) -> Self {
        Self {
            next: recovered.then(|| Instant::now() + interval),
            interval,
        }
    }

    /// A subscription's own (already-validated) override, or the
    /// manager-adopted default when absent — including for a pre-existing
    /// row created before this override existed.
    pub(super) fn interval_from_query(query: &Value) -> Duration {
        query
            .get("minAdmissionIntervalMs")
            .and_then(Value::as_u64)
            .map(Duration::from_millis)
            .unwrap_or(ADMISSION_INTERVAL)
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.next.filter(|deadline| *deadline > Instant::now())
    }

    pub(super) fn admitted(&mut self) {
        self.next = Some(Instant::now() + self.interval);
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(super) enum TestEvent {
    Observed(usize, Instant),
    Admitted(i64, Instant),
    Delivered,
    /// One chained native call returned `"waitOutcome": "timeout"` (its own
    /// receiver budget ran out, not the subscription's intrinsic deadline)
    /// and the chain is re-issuing another call. A test barrier on this event
    /// proves a native receiver boundary was actually crossed mid-collection,
    /// rather than inferring it from a single large `advance()`.
    LegTimedOut,
}

#[cfg(test)]
pub(super) fn observed(state: &super::AppState, count: usize) {
    if let Some(events) = &state.subscription_test_events {
        let _ = events.send(TestEvent::Observed(count, Instant::now()));
    }
}

#[cfg(test)]
pub(super) fn leg_timed_out(state: &super::AppState) {
    if let Some(events) = &state.subscription_test_events {
        let _ = events.send(TestEvent::LegTimedOut);
    }
}
