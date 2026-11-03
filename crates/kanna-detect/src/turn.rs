use crate::agents::Status;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    worked: bool,
    quiet: Option<(Instant, Status)>,
}

impl Turn {
    pub fn underway() -> Turn {
        Turn { worked: true, quiet: None }
    }

    pub fn see(&mut self, status: Option<Status>, at: Instant) {
        match status {
            Some(Status::Working) => *self = Turn::underway(),
            Some(status) if self.worked => self.quiet = Some((self.quiet.map_or(at, |(since, _)| since), status)),
            _ => self.quiet = None,
        }
    }

    pub fn ended(&self, now: Instant) -> Option<Status> {
        self.quiet.filter(|(since, _)| *since <= now).map(|(_, status)| status)
    }

    pub fn due(&self) -> Option<Instant> {
        self.quiet.map(|(since, _)| since)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_turn_ends_when_its_agent_reports_idle_or_blocked_after_working() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = Turn::default();
        turn.see(Some(Status::Idle), at(0));
        assert_eq!(turn.ended(at(5000)), None, "idle before any work is the agent starting");
        turn.see(Some(Status::Working), at(100));
        assert_eq!(turn.ended(at(5000)), None);
        turn.see(Some(Status::NeedsYou), at(200));
        assert_eq!(turn.ended(at(200)), Some(Status::NeedsYou), "blocked ends it at once");
        turn.see(Some(Status::Working), at(300));
        turn.see(Some(Status::Idle), at(400));
        assert_eq!(turn.due(), Some(at(400)));
        assert_eq!(turn.ended(at(400)), Some(Status::Idle));
        turn.see(None, at(500));
        assert_eq!(turn.ended(at(9000)), None, "no agent in the pane");
        let mut known = Turn::underway();
        known.see(Some(Status::Idle), at(0));
        assert_eq!(known.ended(at(0)), Some(Status::Idle), "an agent running when Kanna started has worked");
    }

    #[test]
    fn a_turn_given_time_to_start_ends_only_once_that_time_is_up() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = Turn::underway();
        turn.see(Some(Status::Idle), at(5000));
        assert_eq!(turn.ended(at(4000)), None);
        assert_eq!(turn.ended(at(5000)), Some(Status::Idle));
        turn.see(Some(Status::Working), at(1000));
        assert_eq!(turn.ended(at(9000)), None, "it started on it");
    }
}
