use regex::Regex;
use serde::Deserialize;

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    agent: String,
    #[serde(default)]
    exit: Option<ExitFile>,
    #[serde(default)]
    rest: Option<String>,
    keys: Vec<String>,
    #[serde(default)]
    session: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitFile {
    text: String,
    #[serde(rename = "box")]
    shown: String,
}

#[derive(Clone, Debug)]
pub struct Exit {
    pub text: String,
    pub shown: Regex,
}

#[derive(Clone, Debug)]
pub struct Quit {
    pub agent: String,
    pub exit: Option<Exit>,
    pub rest: Option<String>,
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
        let exit = file.exit.map(|exit| Regex::new(&exit.shown).map(|shown| Exit { text: exit.text, shown }).map_err(|e| format!("exit: {e}"))).transpose()?;
        Ok(Quit { agent: file.agent, exit, rest: file.rest, keys: file.keys, session })
    }

    pub fn of(agent: &str) -> Option<Quit> {
        let (_, text) = BUNDLED_QUIT.iter().copied().find(|(name, _)| *name == agent)?;
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
        for (name, text) in BUNDLED_QUIT.iter().copied() {
            let quit = Quit::parse(text).unwrap_or_else(|e| panic!("{name}.json: {e}"));
            assert_eq!(quit.agent, name);
            assert_eq!(quit.rest.as_deref(), Some("esc"), "{name} rests with esc");
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
        assert_eq!(codex.session_in("To reconnect, run:\n  codex --remote unix:///tmp/k/codex/codex-7.sock resume 01a1\n51f7-7e84-76e1-a224-e75afb524fa2\n").as_deref(), Some("01a151f7-7e84-76e1-a224-e75afb524fa2"), "attached to its app-server");
        assert_eq!(codex.session_in("You can resume a previous conversation by running `codex resume`"), None);
        assert!(Quit::of("aider").is_none());
        assert!(Quit::parse(r#"{"agent": "x", "keys": [], "session": "no group"}"#).is_err());
    }

    #[test]
    fn claude_says_its_session_with_the_flags_it_started_with() {
        let claude = Quit::of("claude").unwrap();
        let id = "20d69fea-c186-462c-b7fd-c75acf7272d6";
        let cases = [
            (format!("Resume this session with:\nclaude --model mock-model --dangerously-skip-permissions --resume {id}\n"), "flags"),
            (format!("claude --model claude-opus-5-5 --append-system-prompt 'two words' --resume {id}\n"), "a flag naming claude, and a quoted value"),
            ("claude --model mock-model --dangerously-skip-permissions --resume 20d69fea-c186-\n462c-b7fd-c75acf7272d6\n".to_string(), "wrapped in the ID"),
            (format!("claude --model mock-model --dangerously-skip-permissions\n--resume {id}\n"), "wrapped at the space before --resume"),
            (format!("claude --model mock-model --resume\n{id}\n"), "wrapped at the space after --resume"),
        ];
        for (screen, case) in cases {
            assert_eq!(claude.session_in(&screen).as_deref(), Some(id), "{case}");
        }
        let later = "22222222-2222-4333-8444-555555555555";
        assert_eq!(claude.session_in(&format!("claude --resume {id}\nclaude --model m --resume {later}\n")).as_deref(), Some(later), "the last one it said");
        assert_eq!(claude.session_in(&format!("use claude with --resume {id}\n")), None, "prose, not a command");
        assert_eq!(claude.session_in(&format!("claude-code --resume {id}\n")), None, "another program");
    }

    #[test]
    fn the_box_holds_only_the_exit_command_on_each_agents_real_screen() {
        let rule = "─".repeat(40);
        let screens = [
            ("claude", format!("  ❯ /exit    Exit the CLI\n{rule}\n❯ /exit\n{rule}\n"), format!("{rule}\n❯ my draft/exit\n{rule}\n"), format!("{rule}\n❯ line one\n  /exit\n{rule}\n")),
            ("codex", "› /exit  exit Codex\n\n› /exit\n\n  mock-model default\n".into(), "\n\n› my draft/exit\n\n  mock-model default\n".into(), "\n\n› line one\n  /exit\n\n  mock-model default\n".into()),
            ("antigravity", format!("{rule}\n> /exit\n{rule}\n> /exit  Exit the CLI\n"), format!("{rule}\n> my draft/exit\n{rule}\n"), format!("{rule}\n> line one\n  /exit\n{rule}\n")),
            ("opencode", "   ┃ /exit       Exit the app      ┃\n   ┃\n   ┃  /exit\n   ┃\n   ┃  Build\n".into(), "   ┃\n   ┃  my draft/exit\n   ┃\n   ┃  Build\n".into(), "   ┃\n   ┃  line one\n   ┃  /exit\n   ┃\n   ┃  Build\n".into()),
        ];
        for (agent, only, draft, lines) in screens {
            let exit = Quit::of(agent).unwrap().exit.unwrap();
            assert_eq!(exit.text, "/exit", "{agent}");
            assert!(exit.shown.is_match(&only), "{agent} holds only /exit: {only}");
            assert!(!exit.shown.is_match(&draft), "{agent}: a draft before it");
            assert!(!exit.shown.is_match(&lines), "{agent}: a draft of several lines");
        }
        let claude = Quit::of("claude").unwrap().exit.unwrap();
        assert!(claude.shown.is_match(&format!("{rule}\n❯\u{a0}/exit\n{rule}\n")), "claude's prompt ends in a no-break space");
        let codex = Quit::of("codex").unwrap().exit.unwrap();
        assert!(!codex.shown.is_match("\n\n› /exit\n  second line\n\n"), "a draft after it");
        assert!(Quit::parse(r#"{"agent": "x", "keys": [], "exit": {"text": "/exit", "box": "("}}"#).is_err());
    }

    #[test]
    fn every_agent_quits_with_its_keys_and_says_its_session() {
        let id = "11111111-2222-4333-8444-555555555555";
        let said = [
            ("claude", format!("Press Ctrl-C again to exit\nResume this session with:\nclaude --resume {id}\n")),
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
