use crate::commands::{Command, COMMANDS};
use crate::target::{Choice, Env, Target};
use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub enum Want {
    Words(Vec<(String, String)>),
    Panes,
    Tabs,
    Windows,
    Instances,
    Files(&'static [&'static str]),
    Dirs,
    Commands,
    Devices,
    Threads,
}

const MARKDOWN: &[&str] = &["md", "markdown", "mdown"];
const HTML: &[&str] = &["html", "htm"];

pub const AGENTS: &[&str] = &["antigravity", "claude", "codex", "copilot", "opencode"];

const GLOBAL: &[(&str, &str)] = &[
    ("--mux kanna|kmux", "Which multiplexer to control."),
    ("--instance INSTANCE", "For developing Kanna: which instance to control."),
    ("--bg", "Start Kanna.app in the background if it isn't running."),
    ("--fresh", "Start a new Kanna without restoring the saved layout."),
    ("--json", "Print the reply as JSON."),
    ("--help", "Help for kanna or a command."),
    ("--version", "Print kanna's version."),
];

const MORE: &[(&str, &str)] = &[
    ("help", "Details, options and examples for one command."),
    ("commands", "All commands with summaries."),
    ("instances", "For developing Kanna: list the running instances."),
    ("raw", "Send a control-protocol request as JSON."),
    ("completions", "Tab completion for zsh or bash."),
    ("config", "Kanna's settings: config schema prints their JSON Schema."),
];

const KEYS: &[&str] = &[
    "return", "esc", "tab", "shift+tab", "space", "backspace", "forward-delete", "up", "down", "left", "right", "home", "end", "pageup", "pagedown", "insert",
    "ctrl+c", "ctrl+d", "ctrl+z", "ctrl+l", "ctrl+r", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
];

const TYPES: &[(&str, &str)] = &[
    ("term", "A terminal (the default)."),
    ("web", "A web page, or a local HTML file."),
    ("md", "A markdown file."),
    ("ios", "An app in the iOS simulator."),
    ("code", "A source file, highlighted."),
    ("diff", "What git diff shows, with untracked files."),
];

pub fn wants(words: &[String]) -> Vec<Want> {
    let (done, current) = words.split_at(words.len().saturating_sub(1));
    let current = current.first().map(String::as_str).unwrap_or("");
    let mut state = State::default();
    let mut i = 0;
    while i < done.len() {
        let word = done[i].as_str();
        i += 1;
        if state.text {
            state.positionals.push(word.into());
            continue;
        }
        if word == "--" {
            state.text = true;
            continue;
        }
        if word.starts_with('-') {
            let Some(option) = options(state.command).find(|o| o.flag == word) else { continue };
            match option.value {
                Takes::Nothing => {}
                Takes::Value(_) => i += 1,
                Takes::Maybe(spec) => {
                    if done.get(i).is_some_and(|next| takes(spec, next)) {
                        i += 1;
                    }
                }
            }
            continue;
        }
        if state.name.is_none() {
            state.name = Some(word.into());
            state.command = COMMANDS.iter().find(|c| c.name == word);
        } else {
            state.positionals.push(word.into());
        }
    }

    if !state.text {
        if let Some(previous) = done.last().filter(|w| w.starts_with("--")) {
            let skipped = done.len() >= 2 && is_value_slot(state.command, &done[..done.len() - 1]);
            if !skipped {
                if let Some(option) = options(state.command).find(|o| o.flag == previous.as_str()) {
                    match option.value {
                        Takes::Value(spec) => return slot(spec),
                        Takes::Maybe(spec) if !current.starts_with('-') => {
                            let mut out = slot(spec);
                            out.extend(positional_wants(&state));
                            return out;
                        }
                        _ => {}
                    }
                }
            }
        }
        if current.starts_with('-') {
            return vec![Want::Words(options(state.command).map(|o| (o.flag.to_string(), o.about.to_string())).collect())];
        }
    }
    let out = positional_wants(&state);
    if out.is_empty() && !state.text && state.command.is_some_and(|c| forms(c).iter().all(|f| next_slot(f, &state.positionals).is_none())) {
        return vec![Want::Words(options(state.command).map(|o| (o.flag.to_string(), o.about.to_string())).collect())];
    }
    out
}

#[derive(Default)]
struct State {
    name: Option<String>,
    command: Option<&'static Command>,
    positionals: Vec<String>,
    text: bool,
}

#[derive(Clone, Copy)]
enum Takes {
    Nothing,
    Value(&'static str),
    Maybe(&'static str),
}

#[derive(Clone, Copy)]
struct Opt {
    flag: &'static str,
    value: Takes,
    about: &'static str,
}

fn opt((spec, about): &(&'static str, &'static str)) -> Opt {
    let (flag, value) = spec.split_once(' ').unwrap_or((spec, ""));
    let value = match value {
        "" => Takes::Nothing,
        v if v.starts_with('[') => Takes::Maybe(v.trim_start_matches('[').trim_end_matches(']')),
        v => Takes::Value(v),
    };
    Opt { flag, value, about }
}

fn options(command: Option<&'static Command>) -> impl Iterator<Item = Opt> {
    let own = command.map(|c| c.options).unwrap_or(&[]);
    let from_usage = command.map(usage_flags).unwrap_or_default();
    own.iter()
        .map(opt)
        .chain(from_usage.into_iter().filter(move |f| !own.iter().any(|o| opt(o).flag == *f)).map(|flag| Opt { flag, value: Takes::Nothing, about: "" }))
        .chain(GLOBAL.iter().map(opt).filter(move |g| !own.iter().any(|o| opt(o).flag == g.flag)))
}

fn usage_flags(command: &'static Command) -> Vec<&'static str> {
    let mut flags: Vec<&'static str> = command
        .usage
        .split_whitespace()
        .flat_map(|t| t.split('|'))
        .map(|t| t.trim_matches(|c| c == '[' || c == ']'))
        .filter(|t| t.starts_with("--"))
        .collect();
    let mut seen = Vec::new();
    flags.retain(|f| !seen.contains(f) && { seen.push(*f); true });
    flags
}

fn takes(spec: &str, word: &str) -> bool {
    !word.starts_with('-') && spec.split('|').any(|alt| alt == word || is_placeholder(alt))
}

fn is_value_slot(command: Option<&'static Command>, before: &[String]) -> bool {
    let Some(option) = before.last().and_then(|w| options(command).find(|o| o.flag == w.as_str())) else { return false };
    matches!(option.value, Takes::Value(_))
}

fn is_placeholder(word: &str) -> bool {
    word.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

fn slot(spec: &str) -> Vec<Want> {
    let mut literals = Vec::new();
    let mut out = Vec::new();
    for alt in spec.split('|') {
        if is_placeholder(alt) {
            out.extend(placeholder(alt));
        } else {
            literals.push((alt.to_string(), String::new()));
        }
    }
    if !literals.is_empty() {
        out.insert(0, Want::Words(literals));
    }
    out
}

fn placeholder(name: &str) -> Vec<Want> {
    let words = |list: &[&str]| vec![Want::Words(list.iter().map(|w| (w.to_string(), String::new())).collect())];
    match name.trim_end_matches('…').trim_end_matches("...") {
        "PANE" | "OTHER" => vec![Want::Panes],
        "TAB" => vec![Want::Tabs],
        "WINDOW" | "ID" => vec![Want::Windows],
        "INSTANCE" => vec![Want::Instances],
        "DIR" => vec![Want::Dirs],
        "FILE" | "SCREEN" | "FILE:LINE" | "FILE:LINE[-LAST]" => vec![Want::Files(&[])],
        "FILE.md" => vec![Want::Files(MARKDOWN)],
        "FILE.html" => vec![Want::Files(HTML)],
        "APP" => vec![Want::Files(&["app"])],
        "CMD" => vec![Want::Commands],
        "DEVICE" => vec![Want::Devices],
        "THREAD" => vec![Want::Threads],
        "AGENT" => words(AGENTS),
        "KEY" => words(KEYS),
        "TYPE" => vec![Want::Words(TYPES.iter().map(|(w, d)| (w.to_string(), d.to_string())).collect())],
        "SIDE" => words(&["left", "right", "top", "bottom", "swap"]),
        "FRACTION" => words(&["1/2", "1/3", "2/3", "1/4", "3/4"]),
        _ => vec![],
    }
}

fn positional_wants(state: &State) -> Vec<Want> {
    let Some(name) = &state.name else {
        let mut words: Vec<(String, String)> = COMMANDS.iter().map(|c| (c.name.to_string(), c.summary.to_string())).collect();
        words.extend(MORE.iter().map(|(w, d)| (w.to_string(), d.to_string())));
        return vec![Want::Words(words)];
    };
    let Some(command) = state.command else {
        return match (name.as_str(), state.positionals.len()) {
            ("help", 0) => {
                let mut words: Vec<(String, String)> = COMMANDS.iter().map(|c| (c.name.to_string(), c.summary.to_string())).collect();
                words.extend(MORE[2..].iter().map(|(w, d)| (w.to_string(), d.to_string())));
                vec![Want::Words(words)]
            }
            ("completions", 0) => vec![Want::Words(vec![("zsh".into(), String::new()), ("bash".into(), String::new())])],
            _ => vec![],
        };
    };
    let mut out: Vec<Want> = Vec::new();
    for form in forms(command) {
        if let Some(next) = next_slot(&form, &state.positionals) {
            for want in slot(refine(&form, next)) {
                merge(&mut out, want);
            }
        }
    }
    if command.name == "open" && state.positionals.is_empty() {
        out.retain(|w| !matches!(w, Want::Words(_)));
        out.insert(0, placeholder("TYPE").remove(0));
    }
    out
}

fn merge(out: &mut Vec<Want>, want: Want) {
    if let Want::Words(more) = want {
        if let Some(Want::Words(have)) = out.iter_mut().find(|w| matches!(w, Want::Words(_))) {
            for word in more {
                if !have.iter().any(|(w, _)| *w == word.0) {
                    have.push(word);
                }
            }
            return;
        }
        out.insert(0, Want::Words(more));
    } else if !out.contains(&want) {
        out.push(want);
    }
}

fn forms(command: &'static Command) -> Vec<Vec<&'static str>> {
    command.usage.lines().map(|line| form(line.split_whitespace().skip(2))).collect()
}

fn form(tokens: impl Iterator<Item = &'static str>) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut group = false;
    for token in tokens {
        let word = token.trim_matches(|c| c == '[' || c == ']');
        let opens = token.starts_with('[') && !token.ends_with(']');
        if word.starts_with("--") || (opens && !is_placeholder(word)) {
            break;
        }
        if !group && !word.is_empty() && word != "…" {
            out.push(word);
        }
        group = (group || opens) && !token.ends_with(']');
    }
    out
}

fn next_slot(form: &[&'static str], typed: &[String]) -> Option<&'static str> {
    for (i, word) in typed.iter().enumerate() {
        let Some(slot) = form.get(i) else {
            return form.last().filter(|s| repeats(s)).copied();
        };
        if !is_placeholder(slot) && !slot.split('|').any(|alt| alt == word) {
            return None;
        }
    }
    form.get(typed.len()).copied().or_else(|| form.last().filter(|s| repeats(s)).copied())
}

fn refine(form: &[&'static str], slot: &'static str) -> &'static str {
    match (form.first().copied(), slot) {
        (Some("md"), "FILE") => "FILE.md",
        (Some("web"), "URL") => "FILE.html",
        (_, slot) if slot.starts_with("FILE[") => "FILE",
        _ => slot,
    }
}

fn repeats(slot: &str) -> bool {
    slot.ends_with("...") || slot.ends_with('…')
}

pub fn lines(wants: &[Want], words: &[String], env: Env) -> Vec<String> {
    let mut out = Vec::new();
    let mut list: Option<Value> = None;
    let mut listed = |words: &[String]| -> Value {
        list.get_or_insert_with(|| live(words, env).unwrap_or(json!({}))).clone()
    };
    let current = words.last().map(String::as_str).unwrap_or("");
    let kinds: Vec<&[&str]> = wants.iter().filter_map(|w| match w { Want::Files(kinds) => Some(*kinds), _ => None }).collect();
    if !kinds.is_empty() {
        let all = kinds.iter().any(|k| k.is_empty());
        let extensions: Vec<&str> = if all { vec![] } else { kinds.concat() };
        out.push(format!(":files {}", extensions.join(" ")).trim_end().to_string());
    }
    for want in wants {
        match want {
            Want::Words(list) => out.extend(list.iter().map(|(w, d)| line(w, d))),
            Want::Panes => out.extend(panes(&listed(words), current)),
            Want::Tabs => out.extend(tabs(&listed(words))),
            Want::Windows => out.extend(windows(&listed(words))),
            Want::Instances => out.extend(instances(env)),
            Want::Files(_) => {}
            Want::Dirs => out.push(":dirs".into()),
            Want::Commands => out.push(":commands".into()),
            Want::Devices => out.extend(devices()),
            Want::Threads => out.extend(crate::threads::open_threads(words.iter().any(|w| w == "reopen")).into_iter().map(|(id, what)| line(&id, &what))),
        }
    }
    out
}

fn line(word: &str, about: &str) -> String {
    let about = about.split(". ").next().unwrap_or("").trim_end_matches('.');
    if about.is_empty() { word.to_string() } else { format!("{word}\t{about}") }
}

fn target(words: &[String], env: Env) -> Option<Target> {
    let mut choice = Choice { mux: None, instance: None, background: false, fresh: false };
    for pair in words.windows(2) {
        match pair[0].as_str() {
            "--mux" => choice.mux = Some(pair[1].clone()),
            "--instance" => choice.instance = Some(pair[1].clone()),
            _ => {}
        }
    }
    Target::resolve(&kanna_build::Build::current(), choice, env).ok()
}

fn live(words: &[String], env: Env) -> Option<Value> {
    target(words, env)?.try_connect()?.call("list", json!({})).ok()
}

pub fn panes(list: &Value, current: &str) -> Vec<String> {
    let by_id = ["term", "file", "page", "ios", "diff"].iter().any(|prefix| current.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('-')).is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())));
    let mut out = Vec::new();
    for pane in list["panes"].as_array().into_iter().flatten() {
        let id = pane["id"].as_str().unwrap_or_default();
        let about = describe(pane);
        match pane["name"].as_str() {
            Some(name) if !by_id => out.push(format!("{name}\t{id} · {about}")),
            _ => out.push(format!("{id}\t{about}")),
        }
    }
    out
}

fn describe(pane: &Value) -> String {
    let file = |path: &str| path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).split(['?', '#']).next().unwrap_or("").to_string();
    let kind = pane["type"].as_str().unwrap_or("term");
    let what = match kind {
        "web" => pane["url"].as_str().map(|u| if u.starts_with("file:") { file(u) } else { u.split('?').next().unwrap_or(u).to_string() }),
        "md" => pane["path"].as_str().map(file),
        "code" => pane["path"].as_str().map(|path| match pane["line"].as_u64() {
            Some(line) => format!("{}:{line}", file(path)),
            None => file(path),
        }),
        "ios" => pane["app"].as_str().map(String::from),
        "diff" => Some(std::iter::once("git diff".to_string()).chain(pane["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from))).collect::<Vec<_>>().join(" ")),
        _ => Some(pane["cmd"].as_str().map(short).unwrap_or_else(|| "shell".into())),
    };
    let mut parts = vec![kind.to_string()];
    parts.extend(what.filter(|w| !w.is_empty()));
    if pane["markup"] == true {
        parts.push("markup".into());
    }
    if let Some(status) = pane["status"].as_str() {
        parts.push(status.into());
    } else if pane["state"].as_str().is_some_and(|s| s != "running") {
        parts.push(pane["state"].as_str().unwrap_or_default().into());
    }
    parts.join(" · ")
}

fn short(text: &str) -> String {
    match text.char_indices().nth(40) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

pub fn tabs(list: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for window in list["windows"].as_array().into_iter().flatten() {
        let wid = window["id"].as_str().unwrap_or_default();
        for tab in window["tabs"].as_array().into_iter().flatten() {
            let active = if tab["active"] == true { ", shown" } else { "" };
            out.push(format!("{}\t\"{}\" in {wid}{active}", tab["id"].as_str().unwrap_or_default(), tab["title"].as_str().unwrap_or_default()));
        }
    }
    out
}

pub fn windows(list: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for window in list["windows"].as_array().into_iter().flatten() {
        let count = window["tabs"].as_array().map_or(0, Vec::len);
        let key = if window["key"] == true { "front, " } else { "" };
        let plural = if count == 1 { "" } else { "s" };
        out.push(format!("{}\t{key}{count} tab{plural}", window["id"].as_str().unwrap_or_default()));
    }
    out
}

fn instances(env: Env) -> Vec<String> {
    let Some(target) = target(&[], env) else { return vec![] };
    target.running(env).into_iter().map(|t| t.instance).collect()
}

fn devices() -> Vec<String> {
    let Ok(out) = std::process::Command::new("xcrun").args(["simctl", "list", "devices", "available", "-j"]).output() else { return vec![] };
    let Ok(reply) = serde_json::from_slice::<Value>(&out.stdout) else { return vec![] };
    let mut names = Vec::new();
    for (runtime, list) in reply["devices"].as_object().into_iter().flatten() {
        let os = runtime.rsplit('.').next().unwrap_or_default().replacen('-', " ", 1).replace('-', ".");
        for device in list.as_array().into_iter().flatten() {
            if let Some(name) = device["name"].as_str() {
                let booted = if device["state"] == "Booted" { ", booted" } else { "" };
                names.push(format!("{name}\t{os}{booted}"));
            }
        }
    }
    names.sort();
    names.dedup_by(|a, b| a.split('\t').next() == b.split('\t').next());
    names
}

pub const ZSH: &str = include_str!("completion.zsh");
pub const BASH: &str = include_str!("completion.bash");

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: &str) -> Vec<Want> {
        let mut words: Vec<String> = line.split(' ').map(String::from).collect();
        if line.is_empty() {
            words = vec![String::new()];
        }
        wants(&words)
    }

    fn words(wants: &[Want]) -> Vec<String> {
        wants.iter().flat_map(|w| match w { Want::Words(list) => list.iter().map(|(w, _)| w.clone()).collect(), _ => vec![] }).collect()
    }

    #[test]
    fn the_first_word_is_a_command() {
        let first = words(&at(""));
        for name in ["open", "close", "markup", "workspace", "help", "instances", "raw"] {
            assert!(first.contains(&name.to_string()), "{first:?}");
        }
    }

    #[test]
    fn options_before_the_command_are_global() {
        let options = words(&at("-"));
        assert!(options.contains(&"--instance".into()) && options.contains(&"--mux".into()), "{options:?}");
        assert_eq!(words(&at("--mux ")), ["kanna", "kmux"]);
        assert_eq!(at("--instance "), [Want::Instances]);
        assert!(words(&at("--mux kmux ")).contains(&"open".into()));
    }

    #[test]
    fn open_completes_the_pane_type_then_its_file() {
        assert_eq!(words(&at("open ")), ["term", "web", "md", "ios", "code", "diff"]);
        assert_eq!(at("open code "), [Want::Files(&[])]);
        assert_eq!(at("open md "), [Want::Files(MARKDOWN)]);
        assert_eq!(at("open md --name x "), [Want::Files(MARKDOWN)]);
        assert!(words(&at("open md notes.md ")).contains(&"--split".into()));
        assert_eq!(at("open web "), [Want::Files(HTML)]);
        assert_eq!(at("open --cwd "), [Want::Dirs]);
        assert_eq!(at("open --cmd "), [Want::Commands]);
        assert_eq!(at("open ios --app "), [Want::Files(&["app"])]);
        assert_eq!(at("open ios --device "), [Want::Devices]);
        assert_eq!(words(&at("open --split ")), ["right", "down", "auto"]);
        assert_eq!(at("open --window "), [Want::Words(vec![("new".into(), String::new())]), Want::Windows]);
    }

    #[test]
    fn options_come_from_the_command() {
        let options = words(&at("open md -"));
        for flag in ["--name", "--split", "--dock", "--json", "--history", "--tab"] {
            assert!(options.contains(&flag.to_string()), "{options:?}");
        }
        let nav = words(&at("navigate p1 -"));
        assert!(nav.contains(&"--back".into()) && nav.contains(&"--forward".into()), "{nav:?}");
    }

    #[test]
    fn pane_tab_and_window_slots_are_live() {
        assert_eq!(at("close "), [Want::Panes]);
        assert_eq!(at("close --tab "), [Want::Tabs]);
        assert_eq!(at("focus --window "), [Want::Windows]);
        assert_eq!(at("send "), [Want::Panes]);
        assert_eq!(at("send p1 "), []);
        assert_eq!(at("send-key "), [Want::Panes]);
        assert!(matches!(&at("send-key p1 up ")[..], [Want::Words(keys)] if keys.iter().any(|(k, _)| k == "ctrl+c")));
        assert_eq!(at("move p1 --to "), [Want::Panes]);
        assert_eq!(words(&at("move p1 --side ")), ["left", "right", "top", "bottom", "swap"]);
        assert_eq!(at("rename-tab "), [Want::Tabs]);
        assert_eq!(at("navigate "), [Want::Panes]);
        assert_eq!(at("navigate p1 "), [Want::Words(vec![("diff".into(), String::new())]), Want::Files(MARKDOWN), Want::Files(&[])]);
        assert_eq!(at("markup ")[1..], [Want::Files(MARKDOWN), Want::Files(HTML), Want::Panes]);
        assert_eq!(lines(&[Want::Files(MARKDOWN), Want::Files(HTML)], &[String::new()], &|_| None), [":files md markdown mdown html htm"]);
        assert_eq!(lines(&[Want::Files(MARKDOWN), Want::Files(&[])], &[String::new()], &|_| None), [":files"]);
        assert!(words(&at("markup spec -")).contains(&"--off".into()));
        assert_eq!(at("markup spec --to "), [Want::Panes]);
        assert!(words(&at("markup ")).contains(&"reply".into()));
        assert_eq!(at("markup reply "), [Want::Threads]);
        assert_eq!(at("markup resolve "), [Want::Threads]);
        assert_eq!(at("markup reply c2 "), []);
        assert_eq!(words(&at("resize p1 ")), ["1/2", "1/3", "2/3", "1/4", "3/4"]);
    }

    #[test]
    fn an_optional_value_offers_its_values_and_what_comes_next() {
        let after = at("move p1 --tab ");
        assert!(after.contains(&Want::Tabs), "{after:?}");
        assert!(words(&after).contains(&"new".into()), "{after:?}");
        assert_eq!(words(&at("open --dock ")), ["left", "right", "term", "web", "md", "ios", "code", "diff"]);
        assert_eq!(at("open --dock left md "), [Want::Files(MARKDOWN)]);
    }

    #[test]
    fn status_and_help_have_their_own_words() {
        assert_eq!(words(&at("status ")), ["explain", "update"]);
        assert_eq!(at("status explain "), [Want::Panes]);
        let agents: Vec<String> = AGENTS.iter().map(|a| a.to_string()).collect();
        assert_eq!(words(&at("status explain --file s.txt --agent ")), agents);
        assert!(words(&at("help ")).contains(&"move-tab".into()));
        assert!(words(&at("help ")).contains(&"completions".into()));
        assert_eq!(words(&at("completions ")), ["zsh", "bash"]);
    }

    #[test]
    fn text_after_a_double_dash_is_not_completed() {
        assert_eq!(at("send p1 -- -"), []);
    }

    #[test]
    fn the_agents_are_the_bundled_rules() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../agent-rules");
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(String::from))
            .filter(|n| n != "mock")
            .collect();
        names.sort();
        assert_eq!(names, AGENTS);
    }

    #[test]
    fn panes_are_offered_by_name_with_what_they_show() {
        let list = json!({ "panes": [
            { "id": "term-1", "name": null, "type": "term", "cmd": null, "state": "running", "status": "working" },
            { "id": "file-1", "name": "spec", "type": "md", "path": "/r/docs/specs/desk.md", "state": "running" },
            { "id": "page-1", "name": null, "type": "web", "url": "file:///r/desk.html", "markup": true, "state": "running" },
            { "id": "term-2", "name": "srv", "type": "term", "cmd": "npm run dev", "state": "exited" }
        ] });
        assert_eq!(panes(&list, ""), [
            "term-1\tterm · shell · working",
            "spec\tfile-1 · md · desk.md",
            "page-1\tweb · desk.html · markup",
            "srv\tterm-2 · term · npm run dev · exited",
        ]);
        assert_eq!(panes(&list, "t")[1], "spec\tfile-1 · md · desk.md");
        assert_eq!(panes(&list, "file-1")[1], "file-1\tmd · desk.md");
    }

    #[test]
    fn tabs_and_windows_say_where_they_are() {
        let list = json!({ "windows": [{ "id": "win-1", "key": true, "tabs": [
            { "id": "tab-1", "title": "Tab 1", "active": true }, { "id": "tab-2", "title": "build", "active": false }
        ] }] });
        assert_eq!(tabs(&list), ["tab-1\t\"Tab 1\" in win-1, shown", "tab-2\t\"build\" in win-1"]);
        assert_eq!(windows(&list), ["win-1\tfront, 2 tabs"]);
    }
}
