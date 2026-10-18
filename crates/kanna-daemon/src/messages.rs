use kanna_detect::Status;
use std::time::{Duration, Instant};

pub const PLANNER: &str = "planner";

pub const USER: &str = "user";

pub const TYPED_HOLD: Duration = Duration::from_secs(5);

pub const DRAFT_HOLD: Duration = Duration::from_secs(120);

pub const PICKUP: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ready {
    status: Option<Status>,
    since: Instant,
    reported: bool,
}

impl Ready {
    pub fn new(now: Instant) -> Ready {
        Ready { status: None, since: now, reported: false }
    }

    pub fn see(&mut self, status: Option<Status>, ended: bool, now: Instant) {
        if status != self.status {
            self.status = status;
            self.since = now;
        }
        self.reported = ended && status == Some(Status::Idle);
    }

    pub fn sent(&mut self, now: Instant) {
        self.since = now;
        self.reported = false;
    }

    pub fn at(&self, settle: Duration) -> Option<Instant> {
        (self.status == Some(Status::Idle)).then(|| if self.reported { self.since } else { self.since + settle })
    }
}

pub fn prompt(messages: &[(String, String)]) -> String {
    messages.iter().map(|(from, text)| format!("[message from {from}] {}", text.trim())).collect::<Vec<_>>().join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_waits_for_the_agent_to_stay_idle_or_to_report_its_turn_over() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let settle = Duration::from_millis(3000);
        let mut ready = Ready::new(t0);
        assert_eq!(ready.at(settle), None, "no agent yet");
        ready.see(Some(Status::Working), false, at(0));
        assert_eq!(ready.at(settle), None, "busy");
        ready.see(Some(Status::Idle), false, at(1000));
        assert_eq!(ready.at(settle), Some(at(4000)), "idle, held 3s");
        ready.see(Some(Status::Idle), false, at(2000));
        assert_eq!(ready.at(settle), Some(at(4000)), "the same idle");
        ready.see(Some(Status::NeedsYou), false, at(2500));
        assert_eq!(ready.at(settle), None, "never while it asks");
        ready.see(Some(Status::Working), false, at(2600));
        ready.see(Some(Status::Idle), true, at(5000));
        assert_eq!(ready.at(settle), Some(at(5000)), "the agent said its turn is over");
        ready.sent(at(5100));
        assert_eq!(ready.at(settle), Some(at(8100)), "after a delivery, it settles again");
    }

    #[test]
    fn messages_go_in_one_prompt_oldest_first_each_with_its_sender() {
        let one = prompt(&[("task-5e2c \"Send\"".into(), "Is your spec final?\n".into()), ("user".into(), "Also rebase.".into())]);
        assert_eq!(one, "[message from task-5e2c \"Send\"] Is your spec final?\n\n[message from user] Also rebase.");
    }
}
