//! Optional user-installed terminal editors. No shell evaluates commands or file names.
use crate::{daemon_client::DaemonClient, db::Db, session_replacements::SessionReplacements};
use kanna_daemon::protocol::{Command, Event, SessionState};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditorChoice {
    pub command: String,
    pub executable: String,
    pub args: Vec<String>,
}

// This is an argv preference, not a shell program. Quotes and escapes let a
// user name paths/arguments with spaces; expansion, pipelines and redirects
// are deliberately unsupported.
fn command_words(command: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for c in command.chars() {
        if c == '\0' || c == '\n' || c == '\r' {
            return Err("Editor command must be one executable with arguments".into());
        }
        if escaped {
            word.push(c);
            escaped = false;
            started = true;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            started = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            started = true;
        } else if c.is_whitespace() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else if "|;&<>`$".contains(c) {
            return Err(
                "Shell expressions are unsupported; use a terminal editor executable and arguments"
                    .into(),
            );
        } else {
            word.push(c);
            started = true;
        }
    }
    if quote.is_some() || escaped {
        return Err("Unfinished quote or escape in terminal editor command".into());
    }
    if started {
        words.push(word);
    }
    if words.first().is_none_or(|s| s.is_empty()) {
        return Err("Terminal editor command is empty".into());
    }
    Ok(words)
}

fn terminal_name(name: &str) -> bool {
    matches!(
        name,
        "nvim"
            | "vim"
            | "vi"
            | "nano"
            | "pico"
            | "micro"
            | "hx"
            | "helix"
            | "emacs"
            | "emacsclient"
    )
}

fn resolve_choice(command: &str) -> Result<EditorChoice, String> {
    let words = command_words(command)?;
    let name = Path::new(&words[0])
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if matches!(
        name,
        "code"
            | "code-insiders"
            | "zed"
            | "subl"
            | "mate"
            | "open"
            | "gvim"
            | "mvim"
            | "gedit"
            | "kate"
    ) {
        return Err("This is a graphical editor. Use IDE Command for external editors".into());
    }
    if matches!(name, "emacs" | "emacsclient")
        && !words
            .iter()
            .any(|s| matches!(s.as_str(), "-nw" | "--no-window-system" | "-t" | "--tty"))
    {
        return Err("Use emacs -nw or emacsclient -t for a terminal interface".into());
    }
    let executable = kanna_runtime_defaults::which_binary(&words[0])
        .or_else(|| kanna_runtime_defaults::find_user_binary(&words[0]))
        .ok_or_else(|| {
            format!(
                "Terminal editor '{}' is not installed or executable",
                words[0]
            )
        })?;
    Ok(EditorChoice {
        command: command.into(),
        executable: executable.to_string_lossy().into_owned(),
        args: words[1..].to_vec(),
    })
}

pub(crate) fn editor_choices(db: &Db) -> Result<Vec<EditorChoice>, String> {
    let preference = db
        .get_setting("terminalEditorCommand")
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if !preference.trim().is_empty() {
        return resolve_choice(preference.trim()).map(|c| vec![c]);
    }
    let mut commands = Vec::new();
    for key in ["VISUAL", "EDITOR"] {
        if let Ok(command) = std::env::var(key) {
            if command_words(&command)
                .ok()
                .and_then(|w| {
                    Path::new(&w[0])
                        .file_name()
                        .map(|s| terminal_name(&s.to_string_lossy()))
                })
                .unwrap_or(false)
            {
                commands.push(command);
            }
        }
    }
    commands.extend(["nvim", "vim", "hx", "micro", "nano", "vi", "emacs -nw"].map(str::to_string));
    let mut choices: Vec<EditorChoice> = Vec::new();
    for command in commands {
        if let Ok(choice) = resolve_choice(&command) {
            if !choices
                .iter()
                .any(|c| c.executable == choice.executable && c.args == choice.args)
            {
                choices.push(choice);
            }
        }
    }
    if choices.is_empty() {
        return Err("No terminal editor found. Set Terminal Editor Command in Preferences to an installed terminal editor".into());
    }
    Ok(choices)
}

pub(crate) fn session_prefix(task_id: &str) -> String {
    format!("shell-editor-{}-{task_id}-", task_id.len())
}

pub(crate) fn session_id(task_id: &str, worktree: &str, file: &str, command: &str) -> String {
    let digest = Sha256::digest(format!("{worktree}\0{file}\0{command}").as_bytes());
    format!("{}{:x}", session_prefix(task_id), digest)
}

/// Closing the task owns editor cleanup, including editors in older stage
/// workspaces and tabs hidden in another window. A stage change preserves them.
pub(crate) async fn close_task_editors(
    daemon: &mut DaemonClient,
    replacements: &SessionReplacements,
    task_id: &str,
) -> Result<(), String> {
    let sessions = match daemon
        .send_command(&Command::List)
        .await
        .map_err(|e| e.to_string())?
    {
        Event::SessionList { sessions } => sessions,
        other => return Err(format!("Could not inspect task editors: {other:?}")),
    };
    for session in sessions
        .into_iter()
        .filter(|s| s.session_id.starts_with(&session_prefix(task_id)))
    {
        crate::task_creator::kill_session_replacing(daemon, replacements, &session.session_id)
            .await?;
    }
    Ok(())
}

pub(crate) fn session_is_live(state: &SessionState) -> bool {
    !matches!(state, SessionState::Exited(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_arguments_are_literal_and_quoted() {
        assert_eq!(
            command_words("'/Applications/My Editor/bin/editor' --option 'two words' \"x;y\"")
                .unwrap(),
            vec![
                "/Applications/My Editor/bin/editor",
                "--option",
                "two words",
                "x;y"
            ]
        );
        for invalid in [
            "",
            "vim; touch /bad",
            "vim $(whoami)",
            "vim | cat",
            "vim > file",
            "vim 'oops",
            "vim\nvi",
        ] {
            assert!(command_words(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn rejects_graphical_and_missing_commands_without_fallback() {
        for command in [
            "code --wait",
            "open -a TextEdit",
            "gvim",
            "emacs",
            "emacsclient",
            "/no/such/terminal-editor",
        ] {
            assert!(resolve_choice(command).is_err(), "{command}");
        }
        let db = Db::open_for_tests(&Db::test_db_path("terminal-editor-preference")).unwrap();
        db.set_setting("terminalEditorCommand", "/no/such/terminal-editor")
            .unwrap();
        assert!(editor_choices(&db).unwrap_err().contains("not installed"));
    }
    #[test]
    fn identity_binds_task_workspace_file_and_command() {
        let id = session_id("a", "/wt/a", "file", "vim");
        assert_eq!(id, session_id("a", "/wt/a", "file", "vim"));
        for other in [
            session_id("ab", "/wt/a", "file", "vim"),
            session_id("a", "/wt/a-2", "file", "vim"),
            session_id("a", "/wt/a", "other", "vim"),
            session_id("a", "/wt/a", "file", "nano"),
        ] {
            assert_ne!(id, other);
        }
        assert!(!session_id("a-b", "/wt/a", "file", "vim").starts_with(&session_prefix("a")));
    }
}
