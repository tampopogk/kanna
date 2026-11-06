use super::{absolute, caller, file_lines, git_line, repo_here, usage, Command, Flow};
use crate::args::Argv;
use crate::output::ago;
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "quiz",
    group: "Information",
    summary: "Open this repo's Quiz window, and keep a quiz's rounds and answers, so misses come back in a later round and a round can cover what's new since the last.",
    usage: "kanna quiz open [--agent CMD]\n       kanna quiz start [--json DOC]\n       kanna quiz answer ROUND --kind KIND --file FILE:LINE[-LAST] --right|--wrong [--again ANSWER] [--json DOC]\n       kanna quiz history [--misses]",
    options: &[
        ("--agent CMD", "open: the quiz agent to start (default: claude)."),
        ("--kind KIND", "answer: architecture, drawing, behaviour, where, why or bug."),
        ("--file FILE:LINE[-LAST]", "answer: the lines the question is about, kept relative to the repo."),
        ("--right, --wrong", "answer: whether the user picked the right choice."),
        ("--again ANSWER", "answer: the earlier miss this question asks again (a7); it stops being a miss."),
        ("--json DOC", "start, answer: a JSON object kept with the round (commits covered, agent) or the answer (question, choices, the pick, the right choice, explanation)."),
        ("--misses", "history: only the misses not asked again since."),
    ],
    notes: "For the kanna-quiz skill's agents, run in the repo. open, for the planning agent, brings the repo's Quiz window forward, or opens one: a window with the quiz agent docked on the left, in the repo's main checkout, told it's the quiz agent, and what it opens as tabs beside it; it isn't a task. start begins a round on this repo at main's commit and prints its ID (r12); answer records one answer to it and prints the answer's ID (a7); history lists this repo's rounds, newest first, each with its commit and answers. history is for picking the next round's questions; your totals over time are on the Desk view's Stats tab (kanna stats).",
    examples: &[
        ("kanna quiz open --agent codex", "Open the Quiz window with Codex as the quiz agent, or bring it forward."),
        ("kanna quiz start --json '{\"commits\": [\"67b287c\"]}'", "Start a round on main's commit; prints its ID, e.g. r12."),
        ("kanna quiz answer r12 --kind why --file docs/specs/database.md:40 --wrong --json '{\"question\": \"Why one SQLite file?\", \"pick\": 2, \"answer\": 1}'", "Record a miss in round r12."),
        ("kanna quiz answer r13 --kind why --file docs/specs/database.md:40 --right --again a7", "Ask miss a7 again, and record that it was right this time."),
        ("kanna quiz history --misses --json", "The misses to bring back, as JSON."),
    ],
    parse,
    show,
    flow: Flow::PLAIN,
};

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let misses = args.flag("--misses");
    let (right, wrong) = (args.flag("--right"), args.flag("--wrong"));
    let kind = args.option("--kind")?;
    let file = args.option("--file")?;
    let again = args.option("--again")?;
    let agent = args.option("--agent")?;
    let doc = match args.doc() {
        Some(text) => Some(serde_json::from_str::<Value>(&text).ok().filter(Value::is_object).ok_or_else(|| usage("quiz", "--json takes a JSON object"))?),
        None => None,
    };
    let action = args.positional().ok_or_else(|| usage("quiz", "say what: open, start, answer or history"))?;
    let only = |given: bool, flag: &str, action: &str| if given { Err(usage("quiz", &format!("{flag} is for quiz {action}"))) } else { Ok(()) };
    if action != "answer" {
        for (given, flag) in [(kind.is_some(), "--kind"), (file.is_some(), "--file"), (right || wrong, "--right and --wrong"), (again.is_some(), "--again")] {
            only(given, flag, "answer")?;
        }
    }
    if action != "history" {
        only(misses, "--misses", "history")?;
    }
    if action != "open" {
        only(agent.is_some(), "--agent", "open")?;
    }
    let repo = repo_here().map_err(|_| usage("quiz", "a quiz is about a git repo: run this in one"))?;
    let mut out = json!({ "action": action, "repo": repo });
    match action.as_str() {
        "open" => {
            if doc.is_some() {
                return Err(usage("quiz", "--json DOC is for start and answer"));
            }
            if let Some(agent) = agent {
                out["agent"] = json!(agent);
            }
        }
        "start" => {
            out["head"] = json!(main_commit(&repo)?);
            if let Some(by) = caller() {
                out["by"] = json!(by);
            }
        }
        "answer" => {
            out["round"] = json!(args.positional().ok_or_else(|| usage("quiz", "answer needs a ROUND, the ID quiz start printed"))?);
            out["kind"] = json!(kind.ok_or_else(|| usage("quiz", "answer needs --kind: architecture, drawing, behaviour, where, why or bug"))?);
            let spot = file.ok_or_else(|| usage("quiz", "answer needs --file FILE:LINE"))?;
            let (file, line, last) = file_lines(&spot).ok_or_else(|| usage("quiz", &format!("{spot} needs a line: FILE:LINE or FILE:LINE-LAST")))?;
            out["file"] = json!(in_repo(&file));
            out["line"] = json!(line);
            if let Some(last) = last {
                out["last"] = json!(last);
            }
            out["right"] = match (right, wrong) {
                (true, false) => json!(true),
                (false, true) => json!(false),
                _ => return Err(usage("quiz", "say --right or --wrong")),
            };
            if let Some(again) = again {
                out["again"] = json!(again);
            }
        }
        "history" => {
            out["misses"] = json!(misses);
            if doc.is_some() {
                return Err(usage("quiz", "--json DOC is for start and answer; history --json prints JSON"));
            }
        }
        other => return Err(usage("quiz", &format!("unknown action \"{other}\" (open, start, answer or history)"))),
    }
    if let Some(doc) = doc {
        out["doc"] = doc;
    }
    Ok(out)
}

pub fn show(request: &Value, reply: &Value) -> String {
    match request["action"].as_str() {
        Some("open") => format!("{} {} in {}", if reply["opened"] == true { "opened" } else { "brought forward" }, reply["pane"].as_str().unwrap_or("?"), reply["window"].as_str().unwrap_or("?")),
        Some("start") => reply["round"].as_str().unwrap_or("?").to_string(),
        Some("answer") => format!("{} in {}", reply["answer"].as_str().unwrap_or("?"), reply["round"].as_str().unwrap_or("?")),
        _ => {
            let rounds = reply["rounds"].as_array().cloned().unwrap_or_default();
            if rounds.is_empty() {
                return if reply["misses"] == true { "no misses to bring back".into() } else { "no quiz rounds in this repo yet".into() };
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            let mut out = vec![];
            for round in &rounds {
                let head: String = round["head"].as_str().unwrap_or("?").chars().take(7).collect();
                out.push(format!("{}  {head}  {}", round["round"].as_str().unwrap_or("?"), round["at"].as_f64().map_or(String::new(), |at| ago(now - at))));
                for answer in round["answers"].as_array().into_iter().flatten() {
                    let mark = if answer["right"] == true { "✓" } else { "✗" };
                    let line = answer["doc"]["line"].as_u64().map_or(String::new(), |l| format!(":{l}"));
                    let again = answer["again"].as_str().map_or(String::new(), |a| format!("  (again: {a})"));
                    let question = answer["doc"]["question"].as_str().map_or(String::new(), |q| format!("  {q}"));
                    out.push(format!("  {:<4} {mark} {:<12} {}{line}{question}{again}", answer["answer"].as_str().unwrap_or("?"), answer["kind"].as_str().unwrap_or("?"), answer["file"].as_str().unwrap_or("?")));
                }
            }
            out.join("\n")
        }
    }
}

pub fn in_repo(file: &str) -> String {
    let path = absolute(file);
    let path = std::fs::canonicalize(&path).unwrap_or_else(|_| path.into());
    let top = git_line(".", &["rev-parse", "--show-toplevel"]);
    match top.as_deref().and_then(|top| path.strip_prefix(top).ok()) {
        Some(inside) => inside.to_string_lossy().into_owned(),
        None => file.to_string(),
    }
}

pub fn main_commit(repo: &str) -> Result<String, Failure> {
    ["main", "master"]
        .iter()
        .find_map(|branch| git_line(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}^{{commit}}")]))
        .ok_or_else(|| usage("quiz", "a quiz asks about main, and this repo has no main (or master) branch"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::parse;

    #[test]
    fn a_quiz_answer_is_sent_with_its_repo_relative_file_and_a_round_on_main_s_commit() {
        let git = |args: &[&str]| String::from_utf8(std::process::Command::new("git").args(args).output().unwrap().stdout).unwrap().trim().to_string();
        let (top, repo) = (git(&["rev-parse", "--show-toplevel"]), git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]));
        let (_, answer) = parse("kanna quiz answer r12 --kind why --file src/lib.rs:40-42 --wrong --again a7 --json '{\"pick\": 2}'").unwrap();
        let file = std::path::Path::new(&std::env::current_dir().unwrap().canonicalize().unwrap()).join("src/lib.rs").strip_prefix(&top).unwrap().to_string_lossy().into_owned();
        assert_eq!(answer, json!({ "action": "answer", "repo": repo.trim_end_matches("/.git"), "round": "r12", "kind": "why", "file": file, "line": 40, "last": 42, "right": false, "again": "a7", "doc": { "pick": 2 } }));
        let (_, start) = parse("kanna quiz start").unwrap();
        assert_eq!(start["head"], json!(git(&["rev-parse", "main"])));
        assert_eq!(parse("kanna quiz history --misses").unwrap().1["misses"], json!(true));
        let refused = |line: &str| parse(line).err().unwrap_or_else(|| panic!("{line} parsed")).message;
        assert!(refused("kanna quiz answer r1 --kind why --file a.rs:1 --right --wrong").contains("--right or --wrong"));
        assert!(refused("kanna quiz answer r1 --kind why --file a.rs --right").contains("needs a line"));
        assert!(refused("kanna quiz start --misses").contains("--misses is for quiz history"));
        assert!(refused("kanna quiz start --right").contains("for quiz answer"));
        assert!(refused("kanna quiz start --json '{oops'").contains("JSON object"));
        assert!(refused("kanna quiz score").contains("unknown action"));
        assert_eq!(parse("kanna quiz open --agent codex").unwrap().1, json!({ "action": "open", "repo": repo.trim_end_matches("/.git"), "agent": "codex" }));
        assert_eq!(parse("kanna quiz open").unwrap().1, json!({ "action": "open", "repo": repo.trim_end_matches("/.git") }));
        assert!(refused("kanna quiz start --agent codex").contains("--agent is for quiz open"));
        assert!(refused("kanna capabilities --json '{}'").contains("--json DOC"));
    }

    #[test]
    fn quiz_history_lists_rounds_with_their_answers_and_no_score() {
        let reply = json!({ "misses": false, "rounds": [
            { "round": "r2", "at": 0.0, "head": "67b287cabc", "answers": [] },
            { "round": "r1", "at": 0.0, "head": "90d98b9def", "answers": [
                { "answer": "a1", "kind": "where", "file": "src/a.rs", "right": false, "again": null, "doc": { "line": 4, "question": "Where?" } },
                { "answer": "a2", "kind": "why", "file": "docs/b.md", "right": true, "again": "a0", "doc": {} },
            ] },
        ]});
        let text = show(&json!({ "action": "history" }), &reply);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("r2  67b287c  "), "{text}");
        assert_eq!(lines[2], "  a1   ✗ where        src/a.rs:4  Where?");
        assert_eq!(lines[3], "  a2   ✓ why          docs/b.md  (again: a0)");
        assert!(!text.contains("of"), "{text}");
        assert_eq!(show(&json!({ "action": "history" }), &json!({ "misses": true, "rounds": [] })), "no misses to bring back");
        assert_eq!(show(&json!({ "action": "start" }), &json!({ "round": "r3" })), "r3");
        assert_eq!(show(&json!({ "action": "open" }), &json!({ "pane": "p9", "window": "win-4", "opened": true })), "opened p9 in win-4");
        assert_eq!(show(&json!({ "action": "open" }), &json!({ "pane": "p9", "window": "win-4", "opened": false })), "brought forward p9 in win-4");
    }
}
