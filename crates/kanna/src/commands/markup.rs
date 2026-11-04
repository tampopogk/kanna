use super::{absolute, caller, file_url, from_caller, is_markdown, no_dock, usage, Command, Flow};
use crate::args::Argv;
use crate::output::label;
use kanna_client::Failure;
use serde_json::{json, Map, Value};

pub const COMMAND: Command = Command {
    name: "markup",
    group: "Panes",
    summary: "Open a markdown file or a local page to comment on, turn comments on or off in a pane, and answer them; the comments come to this pane.",
    usage: "kanna markup FILE.md|FILE.html [--to PANE] [--name NAME] [--split right|down|auto] [--size FRACTION] [--tab] [--window ID|new] [--no-wait]\n       kanna markup PANE [--to PANE | --off]\n       kanna markup comments [--all] [--doc FILE]\n       kanna markup show|resolve|reopen THREAD [--doc FILE]\n       kanna markup reply THREAD TEXT... [--doc FILE]",
    options: &[
        ("--to PANE", "The terminal comments go to, e.g. the pane an agent runs in. Default: this one."),
        ("--name NAME", "A name to refer to the pane by in later commands."),
        ("--split right|down|auto", "Where the pane goes relative to the focused pane, as for open."),
        ("--size FRACTION", "Share of the focused pane's space it takes, as for open."),
        ("--tab", "Open in a new tab instead of splitting."),
        ("--window ID|new", "Open in that window, or a new one."),
        ("--no-wait", "Return as soon as the pane exists."),
        ("--off", "Turn comments off in PANE: it shows the doc alone again. Its comments are kept."),
        ("--all", "comments: resolved ones too."),
        ("--doc FILE", "The doc a comment is on. Default: found from the comment's ID, the docs whose comments come to this pane first."),
    ],
    notes: "A markdown or web pane like `kanna open md FILE` or `kanna open web FILE`, with a thread list and a comment box: select text and press ⌘↵, ⌘-click a part, or ⌘-drag a box. Each comment is sent to the terminal that ran markup (or the one --to names), with the part or box and, for a page or an image, a screenshot with it marked up: to a task's or planner's Claude or Codex as a message once its turn is over (`kanna task messages` lists it), and typed into any other agent at once. When a pane in your workspace already shows FILE (the focused one, else the one focused last), comments come on there and it's focused, unless placement options ask for a new pane. `kanna open` shows the doc without comments; `kanna markup PANE` turns them on there.",
    examples: &[
        ("kanna markup references/mockups/desk.html --split right", "Review a mockup beside this pane."),
        ("kanna markup docs/specs/desk.md --tab", "Review a spec in a new tab."),
        ("kanna markup docs/specs/desk.md --to agent --tab", "Open a spec for comments in a new tab and send them to the pane named agent."),
        ("kanna markup spec", "Turn on comments in the pane named spec; they come to this pane."),
        ("kanna markup spec --to agent", "Send spec's comments to the pane named agent instead."),
        ("kanna markup spec --off", "Show spec without comments again."),
        ("kanna markup comments", "The open comments on the docs whose comments come to this pane."),
        ("kanna markup reply c4 \"Renamed it.\"", "Answer comment c4 beside the doc; leave it open for its author to read and resolve."),
    ],
    parse,
    show,
    flow: Flow { wire: Some(wire), unplaced: Some(unplaced), offline: Some(crate::threads::offline), ..Flow::PLAIN },
};

fn wire(request: &Value) -> &'static str {
    if request.get("type").is_some() { "open" } else { "markup" }
}

fn unplaced(request: &Value) -> Option<&'static str> {
    (!request["type"].is_null() && request["window"].is_null() && request["from"].is_null()).then_some("add --window WINDOW (or --window new)")
}

fn parse_thread_action(args: &mut Argv, action: &str) -> Result<Value, Failure> {
    let all = args.flag("--all");
    let doc = args.option("--doc")?;
    args.positional();
    let mut out = json!({ "local": action });
    if let Some(doc) = doc {
        out["doc"] = json!(doc);
    }
    if action == "comments" {
        out["all"] = json!(all);
        return Ok(out);
    }
    let id = args.positional().ok_or_else(|| usage("markup", &format!("{action} which comment? e.g. kanna markup {action} c2")))?;
    out["id"] = json!(id);
    if action == "reply" {
        let text = args.rest().ok_or_else(|| usage("markup", "reply with what? e.g. kanna markup reply c2 \"Fixed.\""))?;
        out["text"] = json!(text);
    }
    Ok(out)
}

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let mut out = Map::new();
    for (flag, key) in [("--name", "name"), ("--split", "split"), ("--size", "size"), ("--window", "window")] {
        if let Some(value) = args.option(flag)? {
            out.insert(key.into(), json!(value));
        }
    }
    no_dock(args, "markup")?;
    if args.flag("--tab") {
        out.insert("tab".into(), json!(true));
    }
    if args.flag("--no-wait") {
        out.insert("wait".into(), json!(false));
    }
    if let Some(action) = args.first().filter(|a| crate::threads::ACTIONS.contains(a)).map(String::from) {
        return parse_thread_action(args, &action);
    }
    let off = args.flag("--off");
    let to = args.option("--to")?;
    if off && to.is_some() {
        return Err(usage("markup", "--off sends comments nowhere, so it takes no --to"));
    }
    let target = args.positional().ok_or_else(|| usage("markup", "say what: kanna markup FILE.md, kanna markup FILE.html or kanna markup PANE"))?;
    let lower = target.to_lowercase();
    let agent = to.or_else(|| caller());
    let page = lower.ends_with(".html") || lower.ends_with(".htm");
    if !is_markdown(&target) && !page {
        if !out.is_empty() {
            return Err(usage("markup", &format!("{target} is a pane, so it stays where it is: no placement options")));
        }
        let mut pane = json!({ "pane": target });
        if off {
            pane["off"] = json!(true);
        } else if let Some(agent) = agent {
            pane["agent"] = json!(agent);
        }
        return Ok(pane);
    }
    if off {
        return Err(usage("markup", "--off is for a pane: kanna markup PANE --off"));
    }
    if page {
        out.insert("type".into(), json!("web"));
        out.insert("url".into(), json!(file_url(&absolute(&target))));
    } else {
        out.insert("type".into(), json!("md"));
        out.insert("path".into(), json!(absolute(&target)));
    }
    out.insert("markup".into(), json!(true));
    if let Some(agent) = agent {
        out.insert("agent".into(), json!(agent));
    }
    from_caller(&mut out);
    Ok(Value::Object(out))
}

pub fn show(args: &Value, reply: &Value) -> String {
    if args.get("type").is_some() && reply["reused"] == true {
        let full = args["path"].as_str().map(String::from).or_else(|| args["url"].as_str().map(|u| u.trim_start_matches("file://").to_string())).unwrap_or_default();
        let here = std::env::current_dir().map(|d| format!("{}/", d.display())).unwrap_or_default();
        let shown = full.strip_prefix(&here).unwrap_or(&full);
        let tab = reply["tab"].as_str().map(|t| format!(" ({t})")).unwrap_or_default();
        return format!("comments on in {}, which already shows {shown}{tab}; focused", label(&reply["pane"]));
    }
    if args.get("type").is_some() {
        return crate::output::opened(args, reply);
    }
    let pane = &reply["pane"];
    let to = pane["agent"].as_str().map(|agent| format!(", sent to {agent}")).unwrap_or_default();
    match pane["markup"] == true {
        true => format!("comments on in {}{to}", label(pane)),
        false => format!("comments off in {}", label(pane)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::parse;

    #[test]
    fn a_local_page_opens_as_a_file_url_and_markup_opens_or_switches_comments() {
        let dir = std::env::temp_dir().join(format!("kanna-page-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let page = dir.join("a page.html");
        std::fs::write(&page, "<p>hi</p>").unwrap();
        let (_, request) = parse(&format!("kanna open web '{}'", page.display())).unwrap();
        assert_eq!(request["url"], json!(format!("file://{}", page.display().to_string().replace(' ', "%20"))));
        let (_, request) = parse("kanna open web a.test").unwrap();
        assert_eq!(request["url"], json!("a.test"));
        assert!(request.get("markup").is_none());
        let (name, request) = parse(&format!("kanna markup '{}' --tab", page.display())).unwrap();
        assert_eq!(name, "markup");
        assert_eq!(request["type"], json!("web"));
        assert_eq!(request["markup"], json!(true));
        assert_eq!(request["tab"], json!(true));
        let (_, request) = parse("kanna markup /tmp/notes.md --split right").unwrap();
        assert_eq!((request["type"].clone(), request["path"].clone(), request["markup"].clone()), (json!("md"), json!("/tmp/notes.md"), json!(true)));
        assert_eq!(parse("kanna markup spec --off").unwrap().1, json!({ "pane": "spec", "off": true }));
        assert_eq!(parse("kanna markup spec").unwrap().1["pane"], json!("spec"));
        assert!(parse("kanna markup spec --tab").is_err());
        assert_eq!(parse("kanna markup spec --to agent").unwrap().1, json!({ "pane": "spec", "agent": "agent" }));
        assert_eq!(parse("kanna markup /tmp/notes.md --to p4").unwrap().1["agent"], json!("p4"));
        assert!(parse("kanna markup spec --to agent --off").is_err());
        assert!(parse("kanna markup /tmp/notes.md --off").is_err());
        let (_, request) = parse("kanna open md /tmp/notes.md").unwrap();
        assert!(request.get("markup").is_none() && request.get("agent").is_none());
        assert_eq!(parse("kanna markup /tmp/m.html").unwrap().1["url"], json!("file:///tmp/m.html"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
