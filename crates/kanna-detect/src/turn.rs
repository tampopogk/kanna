use crate::agents::Status;
use std::time::{Duration, Instant};

pub const GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Background {
    #[default]
    None,
    Running,
    Ended(Instant),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    worked: bool,
    quiet: Option<(Instant, Status)>,
    background: Background,
}

impl Turn {
    pub fn underway() -> Turn {
        Turn { worked: true, quiet: None, background: Background::None }
    }

    pub fn see(&mut self, status: Option<Status>, at: Instant) {
        match status {
            Some(Status::Working) => *self = Turn::underway(),
            Some(status) if self.worked => self.quiet = Some((self.quiet.map_or(at, |(since, _)| since), status)),
            _ => self.quiet = None,
        }
    }

    pub fn quiet(&self, now: Instant) -> bool {
        self.quiet.is_some_and(|(since, _)| since <= now)
    }

    pub fn background(&mut self, running: bool, at: Instant) -> bool {
        let before = self.background;
        self.background = match (running, before) {
            (true, _) => Background::Running,
            (false, Background::Running) => Background::Ended(at),
            (false, kept) => kept,
        };
        running && before != Background::Running
    }

    pub fn waiting(&self) -> bool {
        self.background == Background::Running
    }

    pub fn ended(&self, now: Instant) -> Option<Status> {
        let free = match self.background {
            Background::None => true,
            Background::Running => false,
            Background::Ended(at) => at + GRACE <= now,
        };
        self.quiet.filter(|(since, _)| *since <= now && free).map(|(_, status)| status)
    }

    pub fn due(&self) -> Option<Instant> {
        let (since, _) = self.quiet?;
        match self.background {
            Background::None => Some(since),
            Background::Running => None,
            Background::Ended(at) => Some(since.max(at + GRACE)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn background_work_holds_a_turns_end_until_it_exits_and_the_grace_passes() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = Turn::underway();
        turn.see(Some(Status::Idle), at(0));
        assert!(turn.quiet(at(0)));
        assert!(turn.background(true, at(0)), "the wait starts");
        assert!(!turn.background(true, at(1000)), "and is said once");
        assert!(turn.waiting());
        assert_eq!(turn.ended(at(60_000)), None, "no time limit while it runs");
        assert_eq!(turn.due(), None);
        turn.background(false, at(5000));
        assert!(!turn.waiting());
        assert_eq!(turn.due(), Some(at(5000) + GRACE));
        assert_eq!(turn.ended(at(5000) + GRACE - Duration::from_millis(1)), None, "the grace");
        assert_eq!(turn.ended(at(5000) + GRACE), Some(Status::Idle));
        turn.background(false, at(9000));
        assert_eq!(turn.due(), Some(at(5000) + GRACE), "a later look without work doesn't move it");
    }

    #[test]
    fn a_working_report_in_the_grace_cancels_the_stop() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = Turn::underway();
        turn.see(Some(Status::Idle), at(0));
        turn.background(true, at(0));
        turn.background(false, at(3000));
        turn.see(Some(Status::Working), at(3050));
        assert_eq!(turn.ended(at(3000) + GRACE), None, "the agent woke for it");
        turn.see(Some(Status::Idle), at(8000));
        assert_eq!(turn.ended(at(8000)), Some(Status::Idle), "its next turn ends at once with nothing running");
        let mut blocked = Turn::underway();
        blocked.see(Some(Status::NeedsYou), at(0));
        blocked.background(true, at(0));
        assert_eq!(blocked.ended(at(9000)), None, "asking with work running waits too");
    }
}
