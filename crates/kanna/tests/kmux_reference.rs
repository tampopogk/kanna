#![cfg(kmux_reference)]

extern crate kanna_client as kmux_client;

#[allow(dead_code)]
mod output {
    include!(concat!(env!("OUT_DIR"), "/kmux_output.rs"));
}

#[allow(dead_code)]
mod commands {
    include!(concat!(env!("OUT_DIR"), "/kmux_commands.rs"));
}

use kanna::args::Argv;
use serde_json::{json, Value};

const KANNA_ONLY: &[&str] = &["read", "screenshot", "send-key", "status", "agents", "markup", "comment", "tour", "rename-pane", "task", "quiz", "projects", "toast", "diagrams", "machine"];

const KMUX_ONLY: &[&str] = &[];

const SOME_ACTIONS: &[(&str, &[&str])] = &[("workspace", &["list"])];

const FOR_TASKS: &[&str] = &["window"];

const EXTENDED: &[(&str, &str)] = &[
    ("open", "notes"),
    ("open", "usage"),
    ("open", "examples"),
    ("navigate", "usage"),
    ("send", "notes"),
    ("move", "notes"),
];

const OPTIONS_DIFFER: &[(&str, &str)] = &[("move", "--window WINDOW|new"), ("open", "--window ID|new"), ("arrange", "--window WINDOW")];

const CODE_PANES: &[(&str, &str)] = &[("open", "summary"), ("navigate", "summary")];

fn from_kmux(text: &str) -> String {
    text.replace("kmux", "kanna")
}

fn from_kanna(text: &str) -> String {
    text.replace("Kanna.app", "kanna").replace("Kanna", "kanna")
}

fn without_code_panes(text: &str) -> String {
    text.replace("\n       kanna open code FILE[:LINE] [same placement options]", "")
        .replace("\n       kanna open diff [GIT-DIFF-ARGS] [same placement options]", "")
        .replace("\n       kanna open md FILE --diff [REF|A..B] [same placement options]", "")
        .replace("\n       kanna navigate PANE FILE[:LINE]", "")
        .replace("\n       kanna navigate PANE diff [GIT-DIFF-ARGS]", "")
}

fn words(line: &str) -> Vec<String> {
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

fn kmux_parse(line: &str) -> Result<Value, (u8, String)> {
    let words = words(line);
    let mut args = kmux_client::Args::new(words[1..].to_vec());
    args.flag("--json");
    let name = args.positional().unwrap();
    let command = commands::find(&name).ok_or((2, format!("unknown command {name}")))?;
    let request = (command.parse)(&mut args).map_err(|f| (f.code, f.message))?;
    if let Some(extra) = args.first() {
        let f = commands::usage(command.name, &format!("unexpected argument \"{extra}\""));
        return Err((f.code, f.message));
    }
    Ok(request)
}

fn kanna_parse(line: &str) -> Result<Value, (u8, String)> {
    let words = words(&line.replace("kmux", "kanna"));
    let mut args = Argv::new(words[1..].to_vec());
    args.flag("--json");
    let name = args.positional().unwrap();
    let command = kanna::commands::find(&name).ok_or((2, format!("unknown command {name}")))?;
    let request = (command.parse)(&mut args).map_err(|f| (f.code, f.message))?;
    if let Some(extra) = args.first() {
        let f = kanna::commands::usage(command.name, &format!("unexpected argument \"{extra}\""));
        return Err((f.code, f.message));
    }
    Ok(request)
}

#[test]
fn kanna_has_every_kmux_command_in_kmux_order() {
    let kmux: Vec<(&str, &str)> = commands::COMMANDS.iter().filter(|c| !KMUX_ONLY.contains(&c.name)).map(|c| (c.name, c.group)).collect();
    let kanna: Vec<(&str, &str)> = kanna::commands::COMMANDS.iter().filter(|c| !KANNA_ONLY.contains(&c.name)).map(|c| (c.name, c.group)).collect();
    assert_eq!(kanna, kmux);
    assert_eq!(kanna::commands::GROUPS, commands::GROUPS);
}

fn same_options<'a>(command: &str, options: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    options.iter().filter(|(option, _)| !OPTIONS_DIFFER.contains(&(command, *option))).copied().collect()
}

fn usage_lines(usage: &str) -> Vec<String> {
    usage.lines().map(|line| line.trim_start_matches("usage: ").trim().to_string()).collect()
}

#[test]
fn commands_kanna_has_some_actions_of_are_kmuxs_for_those_actions() {
    for (name, actions) in SOME_ACTIONS {
        let (theirs, ours) = (commands::find(name).unwrap(), kanna::commands::find(name).unwrap());
        assert_eq!((ours.group, from_kanna(&format!("{:?}", ours.options))), (theirs.group, from_kmux(&format!("{:?}", theirs.options))), "{name}");
        let kept: Vec<String> = usage_lines(&from_kmux(theirs.usage)).into_iter().filter(|line| actions.iter().any(|a| line.starts_with(&format!("kanna {name} {a}")))).collect();
        assert_eq!(usage_lines(ours.usage), kept, "{name}: kanna's usage is kmux's for {actions:?}");
    }
}

#[test]
fn options_kanna_documents_differently_are_still_kmuxs_and_differ() {
    for (name, option) in OPTIONS_DIFFER {
        let theirs = commands::find(name).unwrap().options.iter().find(|(o, _)| o == option);
        let ours = kanna::commands::find(name).unwrap().options.iter().find(|(o, _)| o == option);
        let (Some(theirs), Some(ours)) = (theirs, ours) else { panic!("{name} {option}: kmux or kanna no longer has it") };
        assert_ne!(from_kanna(ours.1), from_kmux(theirs.1), "{name} {option}: kanna documents it as kmux does now");
    }
}

#[test]
fn kanna_has_none_of_kmuxs_own_commands() {
    for name in KMUX_ONLY {
        assert!(commands::find(name).is_some(), "kmux has no {name} any more");
        assert!(kanna::commands::find(name).is_none(), "kanna has {name} now: compare it with kmux's");
    }
}

#[test]
fn commands_kanna_documents_for_tasks_are_kmuxs_and_differ() {
    for name in FOR_TASKS {
        let (theirs, ours) = (commands::find(name).unwrap(), kanna::commands::find(name).unwrap());
        assert_eq!((ours.group, ours.options), (theirs.group, theirs.options), "{name}");
        assert_ne!(from_kanna(ours.usage), from_kmux(theirs.usage), "{name}: kanna documents it as kmux does now");
        assert!(ours.usage.contains("TASK") && theirs.usage.contains("WORKSPACE"), "{name}");
    }
}

#[test]
fn every_command_is_documented_as_kmux_documents_it() {
    for theirs in commands::COMMANDS.iter().filter(|c| !KMUX_ONLY.contains(&c.name) && !FOR_TASKS.contains(&c.name) && !SOME_ACTIONS.iter().any(|(n, _)| *n == c.name)) {
        let ours = kanna::commands::find(theirs.name).unwrap();
        let fields = [
            ("summary", theirs.summary.to_string(), ours.summary.to_string()),
            ("usage", theirs.usage.to_string(), ours.usage.to_string()),
            ("notes", theirs.notes.to_string(), ours.notes.to_string()),
            ("options", format!("{:?}", same_options(theirs.name, theirs.options)), format!("{:?}", same_options(theirs.name, ours.options))),
            ("examples", format!("{:?}", theirs.examples), format!("{:?}", &ours.examples[..ours.examples.len().min(theirs.examples.len())])),
        ];
        for (field, kmux, kanna) in fields {
            let (kmux, kanna) = (from_kmux(&kmux), from_kanna(&kanna));
            if CODE_PANES.contains(&(theirs.name, field)) {
                assert!(kanna.contains("code") && kanna.contains("diff pane"), "{} {field}: kanna's names code and diff panes\nkanna: {kanna}", theirs.name);
            } else if EXTENDED.contains(&(theirs.name, field)) {
                let added = if field == "examples" { ours.examples.len() > theirs.examples.len() } else { kanna.len() > kmux.len() };
                assert!(kanna.starts_with(&kmux) && added, "{} {field}: kanna should add to kmux's\nkmux:  {kmux}\nkanna: {kanna}", theirs.name);
            } else {
                assert_eq!(kanna, kmux, "{} {field}", theirs.name);
            }
        }
    }
}

#[test]
fn every_kmux_example_and_request_parses_the_same() {
    let kept = |c: &commands::Command, example: &str| SOME_ACTIONS.iter().find(|(n, _)| *n == c.name).is_none_or(|(n, actions)| actions.iter().any(|a| example.starts_with(&format!("kmux {n} {a}"))));
    let mut lines: Vec<String> = commands::COMMANDS.iter().filter(|c| !KMUX_ONLY.contains(&c.name)).flat_map(|c| c.examples.iter().filter(|(e, _)| kept(c, e)).map(|(e, _)| e.to_string())).collect();
    lines.extend(
        [
            "kmux open",
            "kmux open --name logs --split right --size 1/3 --cmd 'tail -f a.log'",
            "kmux open web localhost:3000 --split down",
            "kmux open --tab --no-wait",
            "kmux open --window win-2 --name x",
            "kmux close logs",
            "kmux close --window win-2",
            "kmux close --tab tab-3",
            "kmux focus --window win-1",
            "kmux rename-tab tab-1 my servers",
            "kmux send server npm test",
            "kmux send server ls --color",
            "kmux move logs --to server --side bottom",
            "kmux move logs --tab new --window win-2",
            "kmux move-tab tab-2 --index 0",
            "kmux move-tab tab-2 --window=new",
            "kmux open web a.test --history",
            "kmux navigate site --back",
            "kmux navigate site --forward",
            "kmux navigate site https://x.dev/a.md",
            "kmux open md /tmp/a.md",
            "kmux navigate spec /tmp/b.md",
            "kmux open ios --app com.apple.Preferences --device 'iPhone 16'",
            "kmux resize logs 25%",
            "kmux arrange '{\"split\":\"column\",\"children\":[{\"pane\":\"a\"}]}' --window win-1",
            "kmux workspace list --all --json",
            "kmux capabilities",
            "kmux open --dock right --cmd claude",
            "kmux open --dock --frozen --cmd claude",
            "kmux open --dock md /tmp/a.md",
            "kmux open web a.test --dock=right --window new",
            "kmux move agent --dock",
            "kmux move agent --dock right --window win-2",
            "kmux move agent --tab tab-3",
            "kmux move agent --tab --window win-2",
            "kmux resize agent 1/3",
            "kmux window assign win-4 ws-1",
            "kmux window free win-4",
        ]
        .map(String::from),
    );
    // Run from a Kanna terminal, kanna would name its pane as the caller, which kmux doesn't know about.
    std::env::remove_var("KANNA_TERMINAL");
    for line in lines {
        let kmux = kmux_parse(&line).unwrap_or_else(|f| panic!("kmux: {line}: {f:?}"));
        let kanna = kanna_parse(&line).unwrap_or_else(|f| panic!("kanna: {line}: {f:?}"));
        assert_eq!(kanna, kmux, "{line}");
    }
}

#[test]
fn usage_errors_are_kmux_errors() {
    for line in [
        "kmux close",
        "kmux zoom",
        "kmux open web",
        "kmux move logs",
        "kmux zoom a b",
        "kmux open --history",
        "kmux open md",
        "kmux open notes.md",
        "kmux open ios",
        "kmux open --app x",
        "kmux open --device x web u",
        "kmux navigate site --back x",
        "kmux navigate site --back --forward",
        "kmux navigate site",
        "kmux send term-1",
        "kmux resize term-1",
        "kmux arrange",
        "kmux arrange '{nope'",
        "kmux rename-tab",
        "kmux rename-tab tab-1",
        "kmux move-tab",
        "kmux move-tab tab-1 --index x",
        "kmux open --name",
        "kmux capabilities extra",
        "kmux open --dock --split right",
        "kmux open --dock left --tab",
        "kmux move agent --dock --tab",
        "kmux move agent --dock --to b",
        "kmux move agent --dock top",
        "kmux open --frozen",
    ] {
        let kmux = kmux_parse(line).expect_err(line);
        let kanna = kanna_parse(line).expect_err(line);
        assert_eq!(kanna.0, kmux.0, "{line}");
        assert_eq!(without_code_panes(&from_kanna(&kanna.1)), from_kmux(&kmux.1), "{line}");
    }
}

#[test]
fn an_unknown_pane_type_lists_code_and_diff_panes_too() {
    let kmux = kmux_parse("kmux open top").expect_err("kmux");
    let kanna = kanna_parse("kmux open top").expect_err("kanna");
    assert_eq!(kanna.0, kmux.0);
    assert_eq!(without_code_panes(&from_kanna(&kanna.1)), from_kmux(&kmux.1).replace("ios or md", "ios, md, code or diff"));
}

#[test]
fn replies_print_as_kmux_prints_them() {
    let pane = json!({ "id": "term-3", "name": "logs", "type": "term", "state": "running", "cmd": "tail -f a.log" });
    let layout = json!({ "split": "row", "children": [{ "pane": "term-1", "size": "2/3" }, { "pane": "logs", "size": "1/3" }] });
    let cases: Vec<(&str, Value, Value)> = vec![
        ("open", json!({ "type": "term" }), json!({ "pane": pane, "window": "win-1", "tab": "tab-1" })),
        ("open", json!({ "type": "term" }), json!({ "pane": { "id": "term-5", "state": "failed", "error": "No such file" }, "window": "win-1", "tab": "tab-1" })),
        ("close", json!({ "pane": "logs" }), json!({ "closed": ["term-3"] })),
        ("close", json!({ "tab": "tab-2" }), json!({ "closed": ["term-3", "term-4"] })),
        ("close", json!({ "window": "win-2" }), json!({ "closed": ["term-5"] })),
        ("focus", json!({ "pane": "logs" }), json!({ "window": "win-1" })),
        ("focus", json!({ "tab": "tab-2" }), json!({ "window": "win-1" })),
        ("focus", json!({ "window": "win-2" }), json!({ "window": "win-2" })),
        ("zoom", json!({ "pane": "logs" }), json!({ "zoomed": true })),
        ("zoom", json!({ "pane": "logs" }), json!({ "zoomed": false })),
        ("restart", json!({ "pane": "logs" }), json!({ "pane": pane })),
        ("send", json!({ "pane": "logs", "text": "x" }), json!({})),
        ("resize", json!({ "pane": "logs", "size": "1/3" }), json!({ "layout": layout })),
        ("move", json!({ "pane": "logs", "to": "term-1" }), json!({ "layout": layout })),
        ("arrange", json!({ "layout": layout }), json!({ "layout": layout })),
        ("rename-tab", json!({ "tab": "tab-1", "title": "servers" }), json!({ "tab": "tab-1", "title": "servers" })),
        ("move-tab", json!({ "tab": "tab-2", "index": 0 }), json!({ "window": "win-1", "tabs": ["tab-2", "tab-1"] })),
        ("navigate", json!({ "pane": "site", "url": "x" }), json!({ "pane": { "id": "page-2", "name": "site", "state": "running", "url": "http://x/" } })),
        ("navigate", json!({ "pane": "doc", "path": "/tmp/a.md" }), json!({ "pane": { "id": "file-7", "name": "doc", "state": "running", "path": "/tmp/a.md" } })),
        ("open", json!({ "type": "term", "dock": "left" }), json!({ "pane": pane, "window": "win-1", "tab": null, "dock": "left" })),
        ("move", json!({ "pane": "logs", "dock": "right" }), json!({ "window": "win-1", "dock": { "pane": "logs", "edge": "right", "width": "2/5" } })),
        ("resize", json!({ "pane": "logs", "size": "1/4" }), json!({ "window": "win-1", "dock": { "pane": "logs", "edge": "left", "width": "1/4" } })),
        ("capabilities", json!({}), json!({ "mux": "kmux", "paneTypes": ["term", "web"], "commands": ["capabilities", "list"] })),
        ("window", json!({ "action": "free", "window": "win-2" }), json!({ "window": "win-2", "was": null, "shown": true })),
        ("window", json!({ "action": "free", "window": "win-2" }), json!({ "window": "win-2", "was": { "id": "ws-1", "hidden": false }, "shown": true })),
    ];
    for (name, args, reply) in cases {
        let kmux = (commands::find(name).unwrap().show)(&args, &reply);
        let kanna = (kanna::commands::find(name).unwrap().show)(&args, &reply);
        assert_eq!(from_kanna(&kanna), from_kmux(&kmux), "{name} {reply}");
    }
}

#[test]
fn kmux_cannot_type_what_kanna_can_after_a_double_dash() {
    let kanna = kanna_parse("kanna send term-1 -- --json --mux").unwrap();
    assert_eq!(kanna, json!({ "pane": "term-1", "text": "--json --mux" }));
}
