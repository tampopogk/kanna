use super::{caller, file_lines, git_line, repo_here, usage, Call, Command, Flow};
use crate::args::Argv;
use crate::target::Session;
use kanna_client::Failure;
use qrcode::render::unicode;
use qrcode::QrCode;
use serde_json::{json, Value};

const MAX_LINES: u64 = 200;

pub const COMMAND: Command = Command {
    name: "phone",
    group: "Information",
    summary: "Talk to the phone: as the repo's planner or a task's agent, answer the user there; or show the code that pairs a phone.",
    usage: "kanna phone say TEXT\n       kanna phone ask TEXT --choice 'LABEL|WHY' [--choice ...]\n       kanna phone show FILE:LINE[-LAST] [--commit C] [--hot N-M]\n       kanna phone history [--last N]\n       kanna phone pair",
    options: &[
        ("--choice 'LABEL|WHY'", "ask: one choice, repeated; a tap arrives as a new message, and typing answers too."),
        ("--commit C", "show: read the lines from this commit (default: your HEAD)."),
        ("--hot N-M", "show: the lines to mark."),
        ("--last N", "history: only the last N items."),
    ],
    notes: "say, ask, show and history are for the repo's planner and its tasks' agents, run in their own terminal: any other is refused (not_an_agent). The planner's go to the phone's chat, a task's to that task's page. A message starting [message from phone] came from the user on their phone, who can't see the terminal. ask returns at once. show keeps the lines and their commit with the message, so it stays true after main moves. history is the conversation so far, for an agent that resumed. pair shows a QR for the phone to scan; adding it takes a yes in Kanna.app.",
    examples: &[
        ("kanna phone say \"Three merges on main; nothing needs you.\"", "Answer on the phone."),
        ("kanna phone ask \"Merge it?\" --choice 'Merge|tests pass' --choice 'Wait'", "Ask with choices."),
        ("kanna phone show crates/kanna-net/src/node.rs:161-175", "Show code on the phone."),
        ("kanna phone pair", "Show a QR for the phone to scan."),
    ],
    parse,
    show,
    flow: Flow { online: Some(online), before: Some(before), ..Flow::PLAIN },
};

fn before(request: &mut Value, _: &mut Session) -> Result<Call, Failure> {
    if request["action"] == "show" {
        let (repo, file, line, commit) = (request["repo"].as_str().unwrap_or_default().to_string(), request["file"].as_str().unwrap_or_default().to_string(), request["line"].as_u64().unwrap_or(1), request["commit"].as_str().unwrap_or("main").to_string());
        let end = request["end"].as_u64().unwrap_or(line);
        let full = git_line(&repo, &["rev-parse", "--verify", &format!("{commit}^{{commit}}")]).ok_or_else(|| usage("phone", &format!("no commit {commit}")))?;
        request["lines"] = json!(lines_of(&repo, &file, &full, line, end)?);
        request["commit"] = json!(full);
        if let Some(fields) = request.as_object_mut() {
            fields.remove("end");
        }
    }
    Ok(Call::default())
}

fn online(request: &Value, mux: &mut Session, json_output: bool) -> Option<Result<(), Failure>> {
    (request["action"] == "offer").then(|| pair(mux, json_output))
}

pub fn qr(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    Some(code.render::<unicode::Dense1x2>().dark_color(unicode::Dense1x2::Light).light_color(unicode::Dense1x2::Dark).quiet_zone(true).build())
}

fn pair(mux: &mut Session, json_output: bool) -> Result<(), Failure> {
    let offer = mux.call("phone", json!({ "action": "offer" }))?;
    if json_output {
        println!("{offer}");
    } else {
        let scanned = json!({ "code": offer["code"], "key": offer["key"], "addrs": offer["addrs"] }).to_string();
        println!("{}", qr(&scanned).unwrap_or_default());
        println!("Scan this in Kanna on your phone. It works once, for {} minutes. Waiting…", offer["expires_in"].as_u64().unwrap_or(300).div_ceil(60));
    }
    let paired = mux.call("phone", json!({ "action": "wait" }))?;
    if json_output {
        println!("{paired}");
    } else {
        println!("paired: {} is one of your phones", paired["name"].as_str().unwrap_or("?"));
    }
    Ok(())
}

fn lines_of(repo: &str, file: &str, commit: &str, line: u64, last: u64) -> Result<Vec<String>, Failure> {
    let spec = format!("{commit}:{file}");
    let out = std::process::Command::new("git").args(["show", &spec]).current_dir(repo).output().map_err(|e| usage("phone", &format!("can't run git: {e}")))?;
    if !out.status.success() {
        return Err(usage("phone", &format!("{file} isn't in {commit}")));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let found: Vec<String> = text.lines().skip((line - 1) as usize).take((last - line + 1) as usize).map(String::from).collect();
    if found.is_empty() {
        return Err(usage("phone", &format!("{file} has no line {line} in {commit}")));
    }
    Ok(found)
}

fn range(text: &str) -> Option<(u64, u64)> {
    let (first, last) = text.split_once('-')?;
    Some((first.parse().ok()?, last.parse().ok()?))
}

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let mut choices = vec![];
    while let Some(choice) = args.option("--choice")? {
        let (label, why) = choice.split_once('|').map_or((choice.as_str(), None), |(l, w)| (l, Some(w)));
        choices.push(json!({ "label": label.trim(), "why": why.map(str::trim) }));
    }
    let commit = args.option("--commit")?;
    let hot = args.option("--hot")?;
    let last = args.option("--last")?;
    let action = args.positional().ok_or_else(|| usage("phone", "say what: say, ask, show, history or pair"))?;
    if action == "pair" {
        return Ok(json!({ "action": "offer" }));
    }
    let repo = repo_here().map_err(|_| usage("phone", "run it in the repo"))?;
    let mut out = json!({ "action": action, "repo": repo, "pane": caller() });
    match action.as_str() {
        "say" | "ask" => {
            let mut words = vec![args.positional().ok_or_else(|| usage("phone", &format!("{action} needs TEXT")))?];
            while let Some(word) = args.positional() {
                words.push(word);
            }
            out["text"] = json!(words.join(" "));
            if action == "ask" {
                out["choices"] = json!(choices);
            }
        }
        "show" => {
            let spot = args.positional().ok_or_else(|| usage("phone", "show needs FILE:LINE[-LAST]"))?;
            let (file, line, end) = file_lines(&spot).ok_or_else(|| usage("phone", &format!("{spot} needs a line: FILE:LINE or FILE:LINE-LAST")))?;
            let end = end.unwrap_or(line);
            if end < line || end - line >= MAX_LINES {
                return Err(usage("phone", &format!("a code card holds 1 to {MAX_LINES} lines")));
            }
            out["commit"] = json!(commit.or_else(|| git_line(".", &["rev-parse", "HEAD"])).unwrap_or_else(|| "main".into()));
            out["file"] = json!(crate::commands::quiz::in_repo(&file));
            out["line"] = json!(line);
            out["end"] = json!(end);
            if let Some(hot) = hot {
                let (first, last) = range(&hot).ok_or_else(|| usage("phone", "--hot takes N-M"))?;
                out["hot"] = json!([first, last]);
            }
        }
        "history" => {
            if let Some(last) = last {
                out["last"] = json!(last.parse::<u64>().map_err(|_| usage("phone", "--last takes a number"))?);
            }
        }
        other => return Err(usage("phone", &format!("unknown action \"{other}\" (say, ask, show, history or pair)"))),
    }
    Ok(out)
}

fn show(request: &Value, reply: &Value) -> String {
    match request["action"].as_str() {
        Some("history") => reply["items"].as_array().into_iter().flatten().map(|item| {
            let doc = &item["doc"];
            let text = doc["text"].as_str().map(String::from).unwrap_or_else(|| format!("{}:{}", doc["file"].as_str().unwrap_or("?"), doc["line"]));
            format!("{}  {}", item["kind"].as_str().unwrap_or("?"), text)
        }).collect::<Vec<_>>().join("\n"),
        _ => "on the phone".into(),
    }
}
