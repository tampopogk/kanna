use crate::version::{compare, numbers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Working,
    NeedsYou,
    Idle,
}

impl Status {
    pub const ALL: [Status; 3] = [Status::Working, Status::NeedsYou, Status::Idle];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::NeedsYou => "needs-you",
            Status::Idle => "idle",
        }
    }

    pub fn parse(name: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|s| s.as_str() == name)
    }
}

pub const MINIMUMS: &[(&str, &str)] = &[("claude", "2.1.295"), ("codex", "0.163.0"), ("opencode", "2.2.5"), ("antigravity", "1.4.2")];

pub fn minimum(agent: &str) -> Option<&'static str> {
    MINIMUMS.iter().find(|(name, _)| *name == agent).map(|(_, version)| *version)
}

pub fn too_old(agent: &str, version: &str) -> Option<String> {
    let needs = minimum(agent)?;
    let found = numbers(version)?;
    let needed = numbers(needs)?;
    compare(&found, &needed).is_lt().then(|| {
        let name = if agent == "antigravity" { "agy" } else { agent };
        format!("{name} {version} is too old for Kanna: it needs {needs} or newer, which reports its status (OSC 7501). Update it, or run it in a plain terminal.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_older_than_its_minimum_is_too_old() {
        assert_eq!(too_old("claude", "2.1.295"), None);
        assert_eq!(too_old("claude", "2.1.316"), None);
        assert_eq!(too_old("claude", "2.1.290").as_deref(), Some("claude 2.1.290 is too old for Kanna: it needs 2.1.295 or newer, which reports its status (OSC 7501). Update it, or run it in a plain terminal."));
        assert!(too_old("codex", "0.162.0").is_some());
        assert!(too_old("opencode", "2.2.4").is_some());
        assert!(too_old("antigravity", "1.4.1").unwrap().starts_with("agy 1.4.1"));
        assert_eq!(too_old("mock", "0.1.0"), None, "the mock has no minimum");
        assert_eq!(too_old("claude", "unknown"), None, "a version Kanna can't read isn't refused");
    }
}
