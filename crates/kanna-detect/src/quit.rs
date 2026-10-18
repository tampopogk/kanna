use crate::store::bundled_quit;
use regex::Regex;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    agent: String,
    keys: Vec<String>,
    #[serde(default)]
    session: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Quit {
    pub agent: String,
    pub keys: Vec<String>,
    pub session: Option<Regex>,
}

impl Quit {
    pub fn parse(text: &str) -> Result<Quit, String> {
        let file: File = serde_json::from_str(text).map_err(|e| format!("not a quit file: {e}"))?;
        let session = file.session.map(|pattern| Regex::new(&pattern).map_err(|e| format!("session: {e}"))).transpose()?;
        if session.as_ref().is_some_and(|r| r.captures_len() < 2) {
            return Err("session needs a group that captures the ID".into());
        }
        Ok(Quit { agent: file.agent, keys: file.keys, session })
    }

    pub fn of(agent: &str) -> Option<Quit> {
        let (_, text) = bundled_quit().find(|(name, _)| *name == agent)?;
        Quit::parse(text).map_err(|error| eprintln!("kanna: agent-rules/quit/{agent}.json: {error}")).ok()
    }

    pub fn session_in(&self, text: &str) -> Option<String> {
        let session = self.session.as_ref()?;
        let last = |text: &str| session.captures_iter(text).last().and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
        last(text).or_else(|| last(&text.replace('\n', "")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_quit_file_loads_and_names_its_agent() {
        for (name, text) in bundled_quit() {
            let quit = Quit::parse(text).unwrap_or_else(|e| panic!("{name}.json: {e}"));
            assert_eq!(quit.agent, name);
            assert!(crate::program::known_agents().contains(&name), "{name} isn't an agent Kanna knows");
        }
    }

    #[test]
    fn codex_says_its_session_as_it_quits() {
        let codex = Quit::of("codex").unwrap();
        let screen = "Token usage: total=10 input=5 output=5\nTo continue this session, run: codex resume 019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b\n";
        assert_eq!(codex.session_in(screen).as_deref(), Some("019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b"));
        assert_eq!(codex.session_in("To continue this session, run:\ncodex resume 019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b").as_deref(), Some("019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b"));
        assert_eq!(codex.session_in("run: codex resume 019a2b3c-4d5e-7f60-8a9b-0c\n1d2e3f4a5b\n").as_deref(), Some("019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b"), "wrapped in a narrow pane");
        assert_eq!(codex.session_in("You can resume a previous conversation by running `codex resume`"), None);
        assert!(Quit::of("aider").is_none());
        assert!(Quit::parse(r#"{"agent": "x", "keys": [], "session": "no group"}"#).is_err());
    }

    #[test]
    fn every_agent_quits_with_its_keys_and_says_its_session() {
        let id = "11111111-2222-4333-8444-555555555555";
        let said = [
            ("claude", format!("Press Ctrl-C again to exit\nResume this session with:\nclaude --resume {id}\n")),
            ("copilot", format!(" ▔▔▔▔    Resume     copilot --resume={id}\n")),
            ("antigravity", format!("Resume with -c (or command below):\nagy --conversation={id}\n")),
        ];
        for (agent, screen) in said {
            let quit = Quit::of(agent).unwrap();
            assert_eq!(quit.keys, ["ctrl+c", "ctrl+c"], "{agent}");
            assert_eq!(quit.session_in(&screen).as_deref(), Some(id), "{agent}");
            assert_eq!(quit.session_in("Press Ctrl-C again to exit"), None, "{agent}");
        }
        assert_eq!(Quit::of("codex").unwrap().keys, ["ctrl+c", "ctrl+c"]);
        let opencode = Quit::of("opencode").unwrap();
        assert_eq!(opencode.keys, ["ctrl+c", "ctrl+c"]);
        assert!(opencode.session.is_none(), "opencode's session is found in its database");
    }
}
