//! Slash command registry and the filtered menu shown while typing `/…`.

use crate::protocol::HarnessCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    Local,
    Harness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
    pub source: CommandSource,
}

pub const LOCAL_COMMANDS: &[(&str, &str, &str)] = &[
    ("help", "Keys and commands", ""),
    ("status", "Show session details", ""),
    ("theme", "Switch skin", "[skin]"),
    ("stop", "Stop the current turn", ""),
    ("new", "Start a new session", ""),
    ("quit", "Close the session and exit", ""),
];

pub fn local_commands() -> Vec<CommandSpec> {
    LOCAL_COMMANDS
        .iter()
        .map(|(n, d, a)| CommandSpec {
            name: n.to_string(),
            description: d.to_string(),
            argument_hint: a.to_string(),
            source: CommandSource::Local,
        })
        .collect()
}

pub fn harness_commands(cmds: &[HarnessCommand]) -> Vec<CommandSpec> {
    cmds.iter()
        .map(|c| CommandSpec {
            name: c.name.clone(),
            description: c.description.clone(),
            argument_hint: c.argument_hint.clone(),
            source: CommandSource::Harness,
        })
        .collect()
}

/// The command token being typed, if the draft is a single-line `/word`
/// with the cursor inside that word.
pub fn typed_command(text: &str, cursor: usize) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    if text.contains('\n') || rest.contains(char::is_whitespace) || cursor == 0 {
        return None;
    }
    Some(rest)
}

/// Splits `/name args` into (name, args).
pub fn parse_command(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim_end().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if name.is_empty() {
        return None;
    }
    Some((name, args.trim()))
}

#[derive(Debug, Default)]
pub struct SlashMenu {
    pub open: bool,
    pub items: Vec<CommandSpec>,
    pub selected: usize,
    /// Draft text for which the person dismissed the menu with Esc.
    dismissed_for: Option<String>,
}

impl SlashMenu {
    /// Recomputes the menu for the current draft.
    pub fn update(&mut self, text: &str, cursor: usize, all: &[CommandSpec]) {
        if self.dismissed_for.as_deref() == Some(text) {
            self.open = false;
            return;
        }
        self.dismissed_for = None;
        let Some(query) = typed_command(text, cursor) else {
            self.open = false;
            return;
        };
        let q = query.to_lowercase();
        let mut items: Vec<CommandSpec> = all
            .iter()
            .filter(|c| c.name.to_lowercase().starts_with(&q))
            .cloned()
            .collect();
        items.extend(
            all.iter()
                .filter(|c| {
                    !c.name.to_lowercase().starts_with(&q) && c.name.to_lowercase().contains(&q)
                })
                .cloned(),
        );
        let prev = self
            .items
            .get(self.selected)
            .map(|c| (c.name.clone(), c.source));
        self.items = items;
        self.selected = prev
            .and_then(|(n, s)| self.items.iter().position(|c| c.name == n && c.source == s))
            .unwrap_or(0);
        self.open = !self.items.is_empty();
    }

    pub fn dismiss(&mut self, text: &str) {
        self.open = false;
        self.dismissed_for = Some(text.to_string());
    }

    pub fn next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    pub fn prev(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + self.items.len() - 1) % self.items.len();
        }
    }

    pub fn current(&self) -> Option<&CommandSpec> {
        if self.open {
            self.items.get(self.selected)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<CommandSpec> {
        let mut v = local_commands();
        v.extend(harness_commands(&[
            HarnessCommand {
                name: "compact".into(),
                description: "Compact".into(),
                argument_hint: "".into(),
            },
            HarnessCommand {
                name: "cost".into(),
                description: "Cost".into(),
                argument_hint: "".into(),
            },
        ]));
        v
    }

    #[test]
    fn filters_and_wraps() {
        let mut m = SlashMenu::default();
        m.update("/", 1, &all());
        assert!(m.open);
        assert_eq!(m.items.len(), 8);
        m.prev();
        assert_eq!(m.selected, 7, "wraps from first to last");
        m.next();
        assert_eq!(m.selected, 0, "wraps from last to first");
        m.update("/co", 3, &all());
        let names: Vec<_> = m.items.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["compact", "cost"]);
        m.update("/xyz", 4, &all());
        assert!(!m.open);
    }

    #[test]
    fn closes_after_space_and_on_dismiss() {
        let mut m = SlashMenu::default();
        m.update("/theme ", 7, &all());
        assert!(!m.open);
        m.update("/th", 3, &all());
        assert!(m.open);
        m.dismiss("/th");
        m.update("/th", 3, &all());
        assert!(!m.open, "stays closed for the dismissed text");
        m.update("/the", 4, &all());
        assert!(m.open, "reopens once the text changes");
    }

    #[test]
    fn parse() {
        assert_eq!(parse_command("/theme matrix "), Some(("theme", "matrix")));
        assert_eq!(parse_command("/help"), Some(("help", "")));
        assert_eq!(parse_command("/"), None);
        assert_eq!(parse_command("hello"), None);
    }
}
