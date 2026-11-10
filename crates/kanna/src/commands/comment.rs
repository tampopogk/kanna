use super::{absolute, caller, file_lines, from_caller, is_markdown, usage, Command, Flow};
use crate::args::Argv;
use crate::output::{label, whose_text};
use crate::target::Session;
use kanna_client::Failure;
use serde_json::{json, Map, Value};

pub const COMMAND: Command = Command {
    name: "comment",
    group: "Panes",
    summary: "Comment on some lines of a file, shown beside them, to explain the code.",
    usage: "kanna comment FILE:LINE[-LAST] TEXT... [--name NAME] [--split right|down|auto] [--size FRACTION] [--tab] [--window ID|new] [--no-wait]",
    options: &[
        ("--name NAME", "A name for the code pane, if it opens one."),
        ("--split right|down|auto", "Open the file in a new pane split from the focused one."),
        ("--size FRACTION", "Share of the focused pane's space the new pane takes."),
        ("--tab", "Open the file in a new tab, even if it's already open."),
        ("--window ID|new", "Open the file in that window, or a new one."),
    ],
    notes: "On a markdown file, starts a comment thread (as `kanna markup`'s) on the block at LINE (a paragraph, a list item, a heading), written by you, in the doc's pane with comments on; the user's replies in it come to you as comments do.\nOn a source file, opens FILE in a code pane at LINE, or uses the code pane in this window that already shows it, tints the lines and dims the rest, with the comment in a box above them; it replaces the pane's last comment. LINE takes the whole scope it opens or sits in (a function, a loop, a block); LINE-LAST takes those lines as they are. The pane keeps the comment until it closes. Prints the pane and the lines commented on.",
    examples: &[
        ("kanna comment src/fdpass.rs:50 \"Waits for a message and takes the terminals that came with it.\"", "Comment on the function that starts at line 50."),
        ("kanna comment src/client.rs:244-246 \"Takes as many terminals as the message says.\"", "Comment on lines 244 to 246 exactly."),
    ],
    parse,
    show,
    flow: Flow { unplaced: Some(unplaced), online: Some(online), ..Flow::PLAIN },
};

fn online(request: &Value, mux: &mut Session, json_output: bool) -> Option<Result<(), Failure>> {
    (request["doc"] == true).then(|| crate::threads::comment(request, mux, json_output))
}

fn unplaced(request: &Value) -> Option<&'static str> {
    (request["window"].is_null() && request["from"].is_null()).then_some("add --window WINDOW (or --window new)")
}

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let mut out = Map::new();
    for (flag, key) in [("--name", "name"), ("--split", "split"), ("--size", "size"), ("--window", "window")] {
        if let Some(value) = args.option(flag)? {
            out.insert(key.into(), json!(value));
        }
    }
    if args.flag("--tab") {
        out.insert("tab".into(), json!(true));
    }
    if args.flag("--no-wait") {
        out.insert("wait".into(), json!(false));
    }
    let spot = args.positional().ok_or_else(|| usage("comment", "say where: kanna comment FILE:LINE \"text\""))?;
    let (file, line, last) = file_lines(&spot).ok_or_else(|| usage("comment", &format!("{spot} needs a line: FILE:LINE or FILE:LINE-LAST")))?;
    let text = args.rest().filter(|t| !t.trim().is_empty()).ok_or_else(|| usage("comment", "say what: kanna comment FILE:LINE \"text\""))?;
    out.insert("path".into(), json!(absolute(&file)));
    out.insert("line".into(), json!(line));
    if let Some(last) = last {
        if last < line {
            return Err(usage("comment", &format!("lines {line}-{last} run backwards")));
        }
        out.insert("last".into(), json!(last));
    }
    out.insert("text".into(), json!(text));
    if is_markdown(&file) {
        if last.is_some() {
            return Err(usage("comment", "a comment on a markdown file is on the block at one line: FILE.md:LINE"));
        }
        out.insert("doc".into(), json!(true));
        if let Some(agent) = caller() {
            out.insert("agent".into(), json!(agent));
        }
    }
    from_caller(&mut out);
    Ok(Value::Object(out))
}

pub fn show(args: &Value, reply: &Value) -> String {
    let file = args["path"].as_str().and_then(|p| p.rsplit('/').next()).unwrap_or("?");
    let lines = match (reply["first"].as_u64(), reply["last"].as_u64()) {
        (Some(first), Some(last)) if last > first => format!("{first}–{last}"),
        (Some(first), _) => first.to_string(),
        _ => "?".into(),
    };
    let opened = if reply["opened"] == true { "opened " } else { "" };
    let place = whose_text(reply).map(|w| format!(" in {w}")).unwrap_or_default();
    format!("{opened}{}{place} · {file}:{lines}", label(&reply["pane"]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::parse;

    #[test]
    fn a_comment_takes_a_file_a_line_or_lines_and_the_rest_as_its_text() {
        let (name, request) = parse("kanna comment /tmp/a.rs:50 Waits for a message.").unwrap();
        assert_eq!(name, "comment");
        assert_eq!(request, json!({ "path": "/tmp/a.rs", "line": 50, "text": "Waits for a message." }));
        let (_, request) = parse("kanna comment /tmp/a.rs:20-48 'Sends it.' --tab").unwrap();
        assert_eq!((request["line"].clone(), request["last"].clone(), request["tab"].clone()), (json!(20), json!(48), json!(true)));
        assert!(parse("kanna comment /tmp/a.rs Sends it.").is_err());
        assert!(parse("kanna comment /tmp/a.rs:5").is_err());
        assert!(parse("kanna comment /tmp/a.rs:9-3 Backwards.").is_err());
        assert!(parse("kanna comment /tmp/a.rs:0 Nothing.").is_err());
    }
}
