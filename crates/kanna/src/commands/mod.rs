pub mod accounts;
pub mod agents;
pub mod arrange;
pub mod build;
pub mod capabilities;
pub mod close;
pub mod comment;
pub mod completions;
pub mod config;
pub mod crashes;
pub mod diagrams;
pub mod focus;
pub mod guests;
pub mod help;
pub mod instances;
pub mod machine;
pub mod markup;
pub mod r#move;
pub mod move_tab;
pub mod navigate;
pub mod open;
pub mod pipeline;
pub mod project;
pub mod projects;
pub mod phone;
pub mod quiz;
pub mod raw;
pub mod read;
pub mod rename_pane;
pub mod rename_tab;
pub mod resize;
pub mod restart;
pub mod run;
pub mod schedule;
pub mod screenshot;
pub mod send;
pub mod send_key;
pub mod serial;
pub mod shared;
pub mod stats;
pub mod status;
pub mod task;
pub mod toast;
pub mod tour;
pub mod window;
pub mod workspace;
pub mod zoom;

use crate::args::Argv;
use crate::target::{Env, Session, Target};
use kanna_client::{exit, fail, Failure};
use serde_json::Value;
pub use shared::*;

pub struct Command {
    pub name: &'static str,
    pub group: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
    pub options: &'static [(&'static str, &'static str)],
    pub notes: &'static str,
    pub examples: &'static [(&'static str, &'static str)],
    pub parse: fn(&mut Argv) -> Result<Value, Failure>,
    pub show: fn(&Value, &Value) -> String,
    pub flow: Flow,
}

pub struct Flow {
    pub wire: Option<fn(&Value) -> &'static str>,
    pub unplaced: Option<fn(&Value) -> Option<&'static str>>,
    pub offline: Option<fn(&Value, &Invocation) -> Option<Result<(), Failure>>>,
    pub online: Option<fn(&Value, &mut Session, bool) -> Option<Result<(), Failure>>>,
    pub before: Option<fn(&mut Value, &mut Session) -> Result<Call, Failure>>,
    pub after: Option<fn(&Value, &mut Value, &mut Session, &mut Call) -> Result<(), Failure>>,
}

impl Flow {
    pub const PLAIN: Flow = Flow { wire: None, unplaced: None, offline: None, online: None, before: None, after: None };
}

impl Command {
    pub fn wire(&self, request: &Value) -> &'static str {
        self.flow.wire.map_or(self.name, |wire| wire(request))
    }
}

pub struct Invocation<'a> {
    pub target: &'a Target,
    pub env: Env<'a>,
    pub host: Option<String>,
    pub json_output: bool,
}

#[derive(Default)]
pub struct Call {
    pub task: Option<String>,
    pub shorts: Vec<(String, String)>,
    pub listed: bool,
    pub waiting: Option<Value>,
    pub warnings: Vec<String>,
}

pub const GROUPS: &[&str] = &["Panes", "Tabs and windows", "Information"];

pub const LOCAL: &[&str] = &["diagrams", "pipeline"];

pub static COMMANDS: &[Command] = &[
    open::COMMAND,
    close::COMMAND,
    focus::COMMAND,
    zoom::COMMAND,
    restart::COMMAND,
    send::COMMAND,
    send_key::COMMAND,
    read::COMMAND,
    screenshot::COMMAND,
    resize::COMMAND,
    r#move::COMMAND,
    arrange::COMMAND,
    rename_tab::COMMAND,
    move_tab::COMMAND,
    workspace::COMMAND,
    window::COMMAND,
    rename_pane::COMMAND,
    task::COMMAND,
    navigate::COMMAND,
    capabilities::COMMAND,
    diagrams::COMMAND,
    pipeline::COMMAND,
    run::COMMAND,
    status::COMMAND,
    toast::COMMAND,
    serial::COMMAND,
    crashes::COMMAND,
    phone::COMMAND,
    quiz::COMMAND,
    stats::COMMAND,
    projects::COMMAND,
    schedule::COMMAND,
    project::COMMAND,
    machine::COMMAND,
    accounts::COMMAND,
    agents::COMMAND,
    markup::COMMAND,
    comment::COMMAND,
    tour::COMMAND,
];

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

pub fn similar(name: &str) -> Vec<&'static str> {
    let mut names: Vec<(usize, &str)> =
        COMMANDS.iter().map(|c| (distance(name, c.name), c.name)).filter(|(d, n)| *d <= 2 || n.starts_with(name) || name.starts_with(n)).collect();
    names.sort();
    names.into_iter().map(|(_, n)| n).take(3).collect()
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let next = (row[j + 1] + 1).min(row[j] + 1).min(prev + usize::from(ca != b[j]));
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

pub fn sent_as(name: &str) -> &str {
    match name {
        "workspace" => "list",
        name => name,
    }
}

pub fn lookup(name: &str) -> Result<&'static Command, Failure> {
    find(name).ok_or_else(|| {
        let similar = similar(name);
        let hint = if similar.is_empty() { String::new() } else { format!(" Did you mean: {}?", similar.join(", ")) };
        fail(exit::USAGE, format!("unknown command \"{name}\".{hint} Run `kanna --help` for all commands."))
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::output;
    use kanna_client::exit;
    use serde_json::json;

    pub fn shell_words(line: &str) -> Vec<String> {
        let mut words = vec![];
        let mut word = String::new();
        let mut quote: Option<char> = None;
        let mut started = false;
        for c in line.chars() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), c) => word.push(c),
                (None, '"' | '\'') => {
                    quote = Some(c);
                    started = true;
                }
                (None, ' ') => {
                    if started || !word.is_empty() {
                        words.push(std::mem::take(&mut word));
                    }
                    started = false;
                }
                (None, c) => word.push(c),
            }
        }
        if started || !word.is_empty() {
            words.push(word);
        }
        words
    }

    pub fn parse(line: &str) -> Result<(String, Value), Failure> {
        // Run from a Kanna terminal, a command would name its pane as the caller.
        std::env::remove_var("KANNA_TERMINAL");
        let words = shell_words(line);
        assert_eq!(words[0], "kanna");
        let mut args = Argv::new(words[1..].to_vec());
        args.flag("--json");
        let name = args.positional().unwrap();
        let command = lookup(&name)?;
        let request = (command.parse)(&mut args)?;
        if let Some(extra) = args.first() {
            return Err(usage(command.name, &format!("unexpected argument \"{extra}\"")));
        }
        Ok((command.name.to_string(), request))
    }

    #[test]
    fn every_documented_example_parses() {
        for command in COMMANDS {
            assert!(!command.examples.is_empty(), "{} has no examples", command.name);
            for (example, _) in command.examples {
                let (name, _) = parse(example).unwrap_or_else(|f| panic!("{example}: {}", f.message));
                assert_eq!(name, command.name, "{example}");
            }
        }
    }

    #[test]
    fn requests() {
        let cases = [
            ("kanna open", json!({ "type": "term" })),
            ("kanna open --name logs --split right --size 1/3 --cmd 'tail -f a.log'", json!({ "type": "term", "name": "logs", "split": "right", "size": "1/3", "cmd": "tail -f a.log" })),
            ("kanna open web localhost:3000 --split down", json!({ "type": "web", "url": "localhost:3000", "split": "down" })),
            ("kanna open --tab --no-wait", json!({ "type": "term", "tab": true, "wait": false })),
            ("kanna close logs", json!({ "pane": "logs" })),
            ("kanna close --window win-2", json!({ "window": "win-2" })),
            ("kanna rename-tab tab-1 my servers", json!({ "tab": "tab-1", "title": "my servers" })),
            ("kanna send server npm test", json!({ "pane": "server", "text": "npm test" })),
            ("kanna move logs --to server --side bottom", json!({ "pane": "logs", "to": "server", "side": "bottom" })),
            ("kanna move-tab tab-2 --index 0", json!({ "tab": "tab-2", "index": 0 })),
            ("kanna open web a.test --history", json!({ "type": "web", "url": "a.test", "history": true })),
            ("kanna navigate site --back", json!({ "pane": "site", "back": true })),
            ("kanna open md /tmp/a.md", json!({ "type": "md", "path": "/tmp/a.md" })),
            ("kanna navigate spec /tmp/b.md", json!({ "pane": "spec", "path": "/tmp/b.md" })),
            ("kanna read s1", json!({ "pane": "s1" })),
            ("kanna read s1 --lines 5", json!({ "pane": "s1", "lines": 5 })),
            ("kanna send p1 -- --json", json!({ "pane": "p1", "text": "--json" })),
            ("kanna send-key agent return", json!({ "pane": "agent", "keys": ["return"] })),
            ("kanna send-key p3 up up shift+tab ctrl+c", json!({ "pane": "p3", "keys": ["up", "up", "shift+tab", "ctrl+c"] })),
            ("kanna send p1 echo -- --mux -x", json!({ "pane": "p1", "text": "echo --mux -x" })),
            ("kanna open --cwd /tmp", json!({ "type": "term", "cwd": "/tmp" })),
            ("kanna move agent --dock", json!({ "pane": "agent", "dock": "left" })),
            ("kanna move agent --dock right --window win-2", json!({ "pane": "agent", "dock": "right", "window": "win-2" })),
            ("kanna move build --background", json!({ "pane": "build", "background": true })),
            ("kanna move agent --tab", json!({ "pane": "agent", "tab": "new" })),
            ("kanna move agent --tab tab-3", json!({ "pane": "agent", "tab": "tab-3" })),
            ("kanna move agent --tab --window win-2", json!({ "pane": "agent", "tab": "new", "window": "win-2" })),
            ("kanna open code /tmp/a.rs:42", json!({ "type": "code", "path": "/tmp/a.rs", "line": 42 })),
            ("kanna open code /tmp/a.rs", json!({ "type": "code", "path": "/tmp/a.rs" })),
            ("kanna open code /tmp/a.rs:x", json!({ "type": "code", "path": "/tmp/a.rs:x" })),
            ("kanna navigate src /etc/hosts:3", json!({ "pane": "src", "path": "/etc/hosts", "line": 3 })),
            ("kanna navigate src /etc/hosts", json!({ "pane": "src", "path": "/etc/hosts" })),
            ("kanna navigate site localhost:5173", json!({ "pane": "site", "url": "localhost:5173" })),
        ];
        for (line, expected) in cases {
            let (_, request) = parse(line).unwrap_or_else(|f| panic!("{line}: {}", f.message));
            assert_eq!(request, expected, "{line}");
        }
    }

    #[test]
    fn usage_errors_say_what_to_do() {
        for (line, hint) in [
            ("kanna close", "PANE, --tab TAB or --window WINDOW"),
            ("kanna zoom", "missing PANE"),
            ("kanna open top", "--cmd \"top\""),
            ("kanna open web", "need a URL"),
            ("kanna move logs", "say where"),
            ("kanna zoom a b", "unexpected argument \"b\""),
            ("kanna open --history", "only for web panes"),
            ("kanna open md", "need a file"),
            ("kanna open notes.md", "kanna open md notes.md"),
            ("kanna navigate site --back x", "one of URL"),
            ("kanna workspce list", "Did you mean: workspace"),
            ("kanna rename tab-1 x", "Did you mean: rename-tab"),
            ("kanna read", "missing PANE"),
            ("kanna send-key", "missing PANE"),
            ("kanna send-key p1", "missing KEY"),
            ("kanna read p1 --lines 0", "--lines needs a whole number"),
            ("kanna open ios", "need an app"),
            ("kanna open --app x", "only for iOS panes"),
            ("kanna open --name agent --dock", "make a task with `kanna task new`"),
            ("kanna open --dock right --cmd claude", "a docked pane is a task's agent"),
            ("kanna open web a.test --dock=right --window new", "a docked pane is a task's agent"),
            ("kanna open --frozen", "a docked pane is a task's agent"),
            ("kanna markup a.md --dock", "a docked pane is a task's agent"),
            ("kanna move agent --dock --tab", "--dock goes on its own"),
            ("kanna move agent --dock --to b", "--dock goes on its own"),
            ("kanna move build --background --tab", "--background goes on its own"),
        ] {
            let failure = parse(line).expect_err(line);
            assert_eq!(failure.code, exit::USAGE, "{line}");
            assert!(failure.message.contains(hint), "{line}: {}", failure.message);
        }
    }

    #[test]
    fn relative_paths_are_made_absolute_here() {
        let (_, request) = parse("kanna open md docs/a.md --cwd sub").unwrap();
        assert!(request["path"].as_str().unwrap().starts_with('/'));
        assert!(request["cwd"].as_str().unwrap().starts_with('/'));
    }

    #[test]
    fn layouts_print_as_expressions() {
        let layout = json!({ "split": "row", "children": [
            { "pane": "a", "size": "2/3" },
            { "split": "column", "size": "1/3", "children": [{ "pane": "b", "size": "1/2" }, { "pane": "c", "size": "1/2" }] }
        ] });
        assert_eq!(output::expression(&layout, true), "a:2/3 | (b:1/2 / c:1/2):1/3");
    }

    #[test]
    fn docked_panes_print_on_their_own_line() {
        let all = json!({ "action": "list", "all": true });
        let reply = json!({
            "windows": [],
            "workspaces": [{ "id": "ws-1", "where": { "desk": "mac" }, "dock": { "pane": "agent", "id": "p1", "edge": "left", "width": "2/5", "frozen": true }, "focused": "agent",
                             "tabs": [{ "id": "tab-1", "title": "Tab 1", "active": true, "layout": { "pane": "p2" } }], "windows": [], "panes": ["p2", "p1"] }],
            "panes": [{ "id": "p1", "name": "agent", "type": "term", "state": "running", "cmd": "claude" }, { "id": "p2", "name": null, "type": "term", "state": "running" }]
        });
        let text = workspace::show(&all, &reply);
        assert!(text.contains("\nws-1, on the Desk\n  dock: p1 (agent)  left 2/5  frozen\n  focus: agent\n  *tab-1 \"Tab 1\"  p2"), "{text}");
        let opened = json!({ "pane": { "id": "p1", "name": "agent", "state": "running" }, "window": null, "tab": null, "dock": "left", "workspace": { "id": "ws-1", "where": { "desk": "mac" } } });
        assert_eq!(open::show(&json!({}), &opened), "opened p1 (agent) docked left in ws-1 (on the Desk): running");
        assert_eq!(open::show(&json!({ "frozen": true }), &opened), "opened p1 (agent) docked left, frozen, in ws-1 (on the Desk): running");
        let moved = json!({ "window": null, "workspace": { "id": "ws-1" }, "dock": { "pane": "agent", "edge": "right", "width": "1/3" } });
        assert_eq!(output::moved(&moved), "agent is docked right in ws-1 (width 1/3)");
    }

}
