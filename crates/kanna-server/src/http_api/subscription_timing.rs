//! Internal subscription scheduling. Public waits, peer legs and adapters do
//! not own this policy. Defaults adopted by the manager, not measured tuning.
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;

/// The one timing knob: a subscription admits a wake no more often than this,
/// and the batch it delivers is everything observed since the last admission.
/// Nothing waits for quiet and nothing resets on new activity, so a busy
/// repository wakes its manager exactly on this interval while a quiet one
/// wakes it as soon as the interval since the last wake has elapsed. Owner
/// decision, 2026-09-21: the trailing-quiet window this replaces made a lone
/// event wait out a full quiet period, which is precisely the delay it was
/// asked to remove.
pub(super) const ADMISSION_INTERVAL: Duration = Duration::from_millis(60_000);
/// Floor for a per-subscription override of the admission interval
/// (validated at registration in `event_subscriptions::subscribe`). Without
/// one, a near-zero override would defeat the pacing this module exists to
/// provide.
pub(super) const MIN_OVERRIDE: Duration = Duration::from_millis(1_000);

/// One subscription's accumulation of relevant events between two admissions.
/// Batching is the whole point of this type — the rate limit says how often a
/// manager may be woken, and everything observed while the gate is shut goes
/// into the single batch that wake carries.
#[derive(Debug)]
pub(super) struct Collection {
    /// The instant this subscription's rate limit next permits a wake, fixed
    /// when the collection is created from the subscription's own
    /// `Admission`. Unlike the trailing-quiet deadline it replaces, no
    /// observation ever moves it: a collection restarted by an unrelated
    /// state notification resumes against the same gate rather than
    /// restarting its window, and sustained relevant traffic cannot defer a
    /// wake at all, so no derived hard cap is needed to bound it.
    gate: Instant,
    observed: bool,
    urgent: bool,
}

impl Default for Collection {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Collection {
    /// `gate` is the subscription's next permitted admission instant
    /// (`Admission::deadline`), or `None` when its rate limit is already
    /// satisfied — a fresh registration, or one whose last wake is more than
    /// an interval old. `None` therefore seals on the first relevant
    /// observation: the interval since the last wake has elapsed, so there
    /// is nothing left to wait for.
    pub(super) fn new(gate: Option<Instant>) -> Self {
        Self {
            gate: gate.unwrap_or_else(Instant::now),
            observed: false,
            urgent: false,
        }
    }

    pub(super) fn observe(&mut self, events: &[Value]) {
        if !events.is_empty() {
            self.observed = true;
            self.urgent |= events.iter().any(urgent);
        }
    }

    /// The subscription's own rate-limit deadline, independent of any single
    /// native call's receiver. `None` until something relevant has been
    /// observed, so a collection with nothing to deliver blocks on the event
    /// log rather than re-arming short calls against an already-elapsed
    /// gate. A caller that chains several (up to 240s) native calls to honor
    /// a larger interval reads this to size each next request and to know
    /// when it has genuinely finished, not merely run out of one call's own
    /// budget.
    pub(super) fn intrinsic_deadline(&self) -> Option<Instant> {
        self.observed.then_some(self.gate)
    }

    /// Capped by `receiver` (one native call's own hard budget) for sizing
    /// how long that call should sleep before rechecking — never for deciding
    /// whether the subscription's own window is satisfied; see `ready`.
    pub(super) fn deadline(&self, receiver: Instant) -> Instant {
        self.intrinsic_deadline().unwrap_or(receiver).min(receiver)
    }

    /// Whether the subscription's own criteria are genuinely satisfied:
    /// urgent, a full page, or the rate limit's gate reached. Deliberately
    /// ignores any single native call's own receiver — a caller chaining
    /// several calls to cover a window larger than one call's budget must not
    /// mistake "this call's budget ran out" for "done".
    pub(super) fn ready(&self, count: usize, capacity: i64, now: Instant) -> bool {
        count > 0 && (self.urgent || count >= capacity as usize || now >= self.gate)
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
