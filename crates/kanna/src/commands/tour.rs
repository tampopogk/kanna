use super::{absolute, file_lines, from_caller, is_markdown, usage, Command, Flow};
use crate::args::Argv;
use crate::output::label;
use kanna_client::Failure;
use serde_json::{json, Map, Value};

pub const COMMAND: Command = Command {
    name: "tour",
    group: "Panes",
    summary: "Queue a tour of comments the user steps through with Next and Back, without you.",
    usage: "kanna tour [add [--next]] FILE:LINE[-LAST] TEXT [FILE:LINE TEXT ...] [--name NAME] [--split right|down|auto] [--size FRACTION] [--tab] [--window ID|new]\n       kanna tour go next|back|N\n       kanna tour [show]\n       kanna tour end",
    options: &[
        ("--next", "With add: put the stops right after the one the user is on, and show the first of them."),
        ("--name NAME", "A name for the code pane, if the stop shown opens one."),
        ("--split right|down|auto", "Open the stop shown in a new pane split from the focused one."),
        ("--size FRACTION", "Share of the focused pane's space the new pane takes."),
        ("--tab", "Open the stop shown in a new tab, even if its file is already open."),
        ("--window ID|new", "Open the stop shown in that window, or a new one."),
    ],
    notes: "A tour is yours, the agent's: a list of stops, each a comment as `kanna comment` makes it, given as pairs of FILE:LINE and its text (one argument each; quote it). A new tour replaces your last and shows its first stop; every stop is checked first, and nothing is queued if one fails. The user steps through it with Back and Next on the comment, and nothing is sent to you as they do: when they ask you something, `kanna tour` says which stop they're on. Answer a question with `kanna tour add --next` on the code that answers it. `add` without --next adds stops at the end while the user reads. Next on the last stop ends the tour; its comment stays.",
    examples: &[
        ("kanna tour src/server.rs:92 \"Queues the message with its terminals.\" src/fdpass.rs:20 \"Sends them across.\"", "A tour of two stops; shows the first."),
        ("kanna tour", "Which stop the user is on, and the rest."),
        ("kanna tour add --next src/client.rs:244 \"Each line says how many terminals are its.\"", "Answer a question with a stop after the current one, shown now."),
    ],
    parse,
    show,
    flow: Flow::PLAIN,
};

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
    let next = args.flag("--next");
    let mut words = args.positionals();
    let action = match words.first().map(String::as_str) {
        None | Some("show") => "show",
        Some("add") => "add",
        Some("go") => "go",
        Some("end") => "end",
        Some(_) => "new",
    };
    if action != "new" && !words.is_empty() {
        words.remove(0);
    }
    if next && action != "add" {
        return Err(usage("tour", "--next goes with add: kanna tour add --next FILE:LINE \"text\""));
    }
    out.insert("action".into(), json!(action));
    match action {
        "new" | "add" => {
            if words.is_empty() {
                return Err(usage("tour", "say where and what: FILE:LINE \"text\" ..."));
            }
            if words.len() % 2 == 1 {
                let last = words.last().cloned().unwrap_or_default();
                return Err(usage("tour", &match file_lines(&last) {
                    Some(_) => format!("{last} has no text: give each stop as FILE:LINE \"text\""),
                    None => format!("\"{last}\" has no FILE:LINE before it: give each stop as FILE:LINE \"text\", the text quoted as one argument"),
                }));
            }
            let mut stops = vec![];
            for pair in words.chunks(2) {
                let (spot, text) = (&pair[0], pair[1].trim());
                let (file, line, last) = file_lines(spot).ok_or_else(|| usage("tour", &format!("{spot} needs a line: FILE:LINE or FILE:LINE-LAST, then its text quoted as one argument")))?;
                if text.is_empty() {
                    return Err(usage("tour", &format!("{spot} has no text")));
                }
                let mut stop = json!({ "path": absolute(&file), "line": line, "text": text });
                if let Some(last) = last {
                    if last < line {
                        return Err(usage("tour", &format!("{spot}: lines {line}-{last} run backwards")));
                    }
                    if is_markdown(&file) {
                        return Err(usage("tour", &format!("{spot}: a comment on a markdown file is on the block at one line: FILE.md:LINE")));
                    }
                    stop["last"] = json!(last);
                }
                stops.push(stop);
            }
            out.insert("stops".into(), Value::Array(stops));
            if next {
                out.insert("next".into(), json!(true));
            }
        }
        "go" => {
            let to = match words.as_slice() {
                [to] if to == "next" || to == "back" => json!(to),
                [to] => json!(to.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(|| usage("tour", &format!("\"{to}\" isn't a stop: next, back or a stop's number")))?),
                _ => return Err(usage("tour", "say where: kanna tour go next|back|N")),
            };
            out.insert("to".into(), to);
        }
        _ => {
            if let Some(extra) = words.first() {
                return Err(usage("tour", &format!("unexpected argument \"{extra}\"")));
            }
        }
    }
    from_caller(&mut out);
    Ok(Value::Object(out))
}

pub fn show(args: &Value, reply: &Value) -> String {
    if reply["done"] == true {
        return format!("tour done · {} stops", reply["of"].as_u64().unwrap_or(0));
    }
    match args["action"].as_str().unwrap_or("show") {
        "end" => (if reply["ended"] == true { "tour ended" } else { "no tour" }).to_string(),
        "show" => {
            let stops = reply["stops"].as_array().cloned().unwrap_or_default();
            if stops.is_empty() {
                return "no tour".into();
            }
            let at = reply["at"].as_u64().unwrap_or(1) as usize;
            let place = |stop: &Value| format!("{}:{}", shown_path(stop["path"].as_str().unwrap_or("?")), stop_lines(stop["first"].as_u64(), stop["last"].as_u64()));
            let mut out = format!("at {at} of {} · {}", stops.len(), place(&stops[at - 1]));
            for (i, stop) in stops.iter().enumerate() {
                let marker = if i + 1 == at { "▸" } else { " " };
                let text: String = stop["text"].as_str().unwrap_or("").chars().take(60).collect();
                out += &format!("\n{marker} {:>2}  {}  {text}", i + 1, place(stop));
            }
            out
        }
        action => {
            let (at, of) = (reply["tour"]["at"].as_u64().unwrap_or(0), reply["tour"]["of"].as_u64().unwrap_or(0));
            if reply.get("pane").is_none() {
                return format!("added {} · at {at} of {of}", reply["added"].as_u64().unwrap_or(0));
            }
            let opened = if reply["opened"] == true { "opened " } else { "" };
            let place = format!("{}:{}", shown_path(reply["path"].as_str().unwrap_or("?")).rsplit('/').next().unwrap_or("?"), stop_lines(reply["first"].as_u64(), reply["last"].as_u64()));
            let thread = reply["thread"].as_str().map(|t| format!(" · {t}")).unwrap_or_default();
            let step = format!("{at} of {of} · {opened}{} · {place}{thread}", label(&reply["pane"]));
            match action {
                "new" => format!("tour of {of} · {step}"),
                "add" => format!("added {} · {step}", args["stops"].as_array().map(Vec::len).unwrap_or(0)),
                _ => step,
            }
        }
    }
}

fn stop_lines(first: Option<u64>, last: Option<u64>) -> String {
    match (first, last) {
        (Some(first), Some(last)) if last > first => format!("{first}–{last}"),
        (Some(first), _) => first.to_string(),
        _ => "?".into(),
    }
}

fn shown_path(path: &str) -> String {
    let here = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    match path.strip_prefix(&format!("{}/", here.trim_end_matches('/'))) {
        Some(inside) if !here.is_empty() => inside.to_string(),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::parse;

    #[test]
    fn a_tour_takes_pairs_of_a_spot_and_its_text() {
        let (name, request) = parse("kanna tour /tmp/a.rs:50 'Waits for a message.' /tmp/b.md:3 'The big picture.' --tab").unwrap();
        assert_eq!(name, "tour");
        assert_eq!((request["action"].clone(), request["tab"].clone()), (json!("new"), json!(true)));
        assert_eq!(request["stops"], json!([{ "path": "/tmp/a.rs", "line": 50, "text": "Waits for a message." }, { "path": "/tmp/b.md", "line": 3, "text": "The big picture." }]));
        let (_, request) = parse("kanna tour add --next /tmp/a.rs:20-48 'Sends it.'").unwrap();
        assert_eq!((request["action"].clone(), request["next"].clone(), request["stops"][0]["last"].clone()), (json!("add"), json!(true), json!(48)));
        assert_eq!(parse("kanna tour").unwrap().1["action"], "show");
        assert_eq!(parse("kanna tour go back").unwrap().1["to"], "back");
        assert_eq!(parse("kanna tour go 3").unwrap().1["to"], 3);
        assert_eq!(parse("kanna tour end").unwrap().1["action"], "end");
        for bad in [
            "kanna tour /tmp/a.rs:5 Waits for a message.",
            "kanna tour /tmp/a.rs:5",
            "kanna tour /tmp/a.rs 'No line.'",
            "kanna tour /tmp/a.rs:9-3 'Backwards.'",
            "kanna tour /tmp/a.md:3-5 'A doc block.'",
            "kanna tour add",
            "kanna tour go",
            "kanna tour go sideways",
            "kanna tour --next /tmp/a.rs:5 'Next of nothing.'",
            "kanna tour end now",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
