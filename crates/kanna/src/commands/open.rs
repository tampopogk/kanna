use super::{absolute, caller, diagrams, file_line, file_url, from_caller, git_line, is_markdown, no_dock, usage, Call, Command, Flow};
use crate::args::Argv;
use crate::output::{label, state, whose_text};
use crate::target::Session;
use kanna_client::Failure;
use serde_json::{json, Map, Value};

pub const COMMAND: Command = Command {
    name: "open",
    group: "Panes",
    summary: "Open a terminal, web, iOS, markdown, code, diff, log, serial or run pane and wait until it is running.",
    usage: "kanna open [term] [--cmd CMD] [--cwd DIR] [--name NAME] [--split right|down|auto] [--size FRACTION] [--tab] [--window ID|new] [--no-wait]\n       kanna open web URL [--history] [same placement options]\n       kanna open md FILE [same placement options]\n       kanna open ios --app APP [--device DEVICE] [same placement options]\n       kanna open code FILE[:LINE] [same placement options]\n       kanna open diff [GIT-DIFF-ARGS] [same placement options]\n       kanna open log [REV...] [-- PATH...] [same placement options]\n       kanna open md FILE --diff [REF|A..B] [same placement options]\n       kanna open serial PORT [--baud N] [--line-ending lf|crlf|cr] [same placement options]\n       kanna open run [RUN] [same placement options]\n       kanna open run [RUN] --watch [WORKER]",
    options: &[
        ("--cmd CMD", "Shell command line to run in your login shell, e.g. \"npm run dev\". Put it in single quotes if it uses $VARS or ;, so your own shell leaves it alone. Default: an interactive shell."),
        ("--cwd DIR", "Working directory."),
        ("--name NAME", "Optional. A name to refer to the pane by in later commands; without one, use the ID that open prints (e.g. term-3)."),
        ("--split right|down|auto", "Where the new pane goes relative to the focused pane. auto (default) splits along the longer side."),
        ("--size FRACTION", "Share of the focused pane's space the new pane takes: 1/3, 25% or 0.25. Default 1/2."),
        ("--tab", "Open in a new tab instead of splitting."),
        ("--window ID|new", "Open in that window, or a new one. Default: the window of the terminal you run it in."),
        ("--no-wait", "Return as soon as the pane exists instead of waiting until it is running."),
        ("--app APP", "iOS panes: a .app built for the simulator (installed, then launched) or the bundle ID of an installed app, e.g. com.apple.Preferences."),
        ("--device DEVICE", "iOS panes: a simulator device name or UDID, e.g. \"iPhone 16\". Default: a booted iPhone, else the newest one. The device is booted if needed (this can take a minute) and stays booted after the pane closes."),
        ("--history", "Web panes only: keep back/forward history (off by default), for `kanna navigate PANE --back` and the Pane menu's Back and Forward."),
        ("--watch [WORKER]", "Run panes only: show the run's current worker terminal (or WORKER) in a tab of the task's workspace instead; the tab follows the run to each next stage's worker, and what you type there is kept as an intervention."),
    ],
    notes: "Prints the new pane's ID and name. A pane whose command exits stays on screen as `exited`. A docked pane closes only with its window.\nA terminal opened without --split, --size, --tab or --window opens even with Kanna.app closed, and takes its place when the app next opens; anything else starts the app.\nRun from a pane without --window, it opens in that pane's window, not the key one; from a task kept off-screen (on the Desk but not shown, or on the Shelf), in a new tab of the task's window. Run from outside a Kanna terminal, it needs --window: there's no window of yours to open in.\nA docked pane is a task's agent, so open docks none: make a task with `kanna task new`, then `kanna task pick` or `kanna task start`.\n--diff [REF] marks what changed in a markdown doc since REF, inline: added text in green, removed text struck through in red, a bar beside each changed block; ]c and [c jump to the next and previous change. REF is any commit or branch, compared as `git diff REF...` compares: from where HEAD and REF meet, uncommitted changes included. Default: main (or master), so a task's doc shows everything the task changed. --diff A..B shows the doc as it is in commit B, marked against commit A (compared directly, not from where they meet); it's read-only, doesn't follow the file and takes no comments.",
    examples: &[
        ("kanna open --name server --cmd \"npm run dev\"", "Run a dev server in a pane named server."),
        ("kanna open --name logs --split right --size 1/3 --cmd \"tail -f app.log\"", "Put a log tail in the right third."),
        ("kanna open --tab", "Open a shell in a new tab."),
        ("kanna open --name build --split down --size 1/4 --cmd 'for i in 1 2 3; do echo step $i; done'", "A quarter-height pane below, running a loop."),
        ("kanna open --window new --name scratch", "Open a shell in a new window."),
        ("kanna open ios --name phone --app build/MyApp.app --split right --size 1/3", "Run your app in the simulator, in the right third."),
        ("kanna open md docs/spec.md --name spec --split right", "Show a markdown file (with mermaid diagrams) beside this pane; it reloads when the file changes."),
        ("kanna open md docs/specs/presenting.md --diff main --tab", "Show a spec with what this branch changed in it marked."),
        ("kanna open code src/main.rs:42", "Show a source file, highlighted, at line 42; it follows the file on disk."),
        ("kanna open diff --merge-base main", "Show everything this branch changed since it left main, committed or not, with untracked files; it follows the worktree."),
        ("kanna open log", "Show the history as a graph: from a task, its branch and main; from the main checkout, main and the open tasks' branches, each labelled with its task. It follows the repo."),
        ("kanna open serial usbmodem1101 --tab", "Show what the device on /dev/cu.usbmodem1101 sends, at 115200; `kanna serial` reads and writes the same port."),
        ("kanna open run", "Show the latest run of this terminal's task in a Run tab: its pipeline, numbers and visits, with buttons to steer it."),
        ("kanna open run --watch", "Show the stage running now in a tab that follows the run from stage to stage."),
    ],
    parse,
    show,
    flow: Flow { wire: Some(wire), unplaced: Some(unplaced), after: Some(after), ..Flow::PLAIN },
};

fn wire(request: &Value) -> &'static str {
    if request["action"] == "watch" {
        "run"
    } else {
        "open"
    }
}

fn after(request: &Value, reply: &mut Value, _: &mut Session, call: &mut Call) -> Result<(), Failure> {
    if let (Some(path), "md") = (request["path"].as_str(), request["type"].as_str().unwrap_or_default()) {
        let undrawn = diagrams::check(path).map(|r| r.failures).unwrap_or_default();
        call.warnings = undrawn.iter().map(diagrams::line).collect();
        reply["diagrams"] = Value::Array(undrawn);
    }
    Ok(())
}

fn unplaced(request: &Value) -> Option<&'static str> {
    if request["type"] == "run" || request["action"] == "watch" {
        return None;
    }
    (!["window", "workspace", "from"].iter().any(|key| !request[*key].is_null())).then_some("add --window WINDOW (or --window new)")
}

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let mut out = Map::new();
    for (flag, key) in [("--cmd", "cmd"), ("--cwd", "cwd"), ("--name", "name"), ("--split", "split"), ("--size", "size"), ("--window", "window"), ("--app", "app"), ("--device", "device")] {
        if let Some(value) = args.option(flag)? {
            out.insert(key.into(), json!(value));
        }
    }
    if let Some(Value::String(cwd)) = out.get("cwd") {
        let cwd = absolute(cwd);
        out.insert("cwd".into(), json!(cwd));
    }
    if let Some(Value::String(app)) = out.get("app") {
        let path = std::path::Path::new(app);
        if path.is_relative() && path.exists() {
            if let Ok(full) = std::fs::canonicalize(path) {
                out.insert("app".into(), json!(full.to_string_lossy()));
            }
        }
    }
    no_dock(args, "open")?;
    if args.flag("--tab") {
        out.insert("tab".into(), json!(true));
    }
    if args.flag("--no-wait") {
        out.insert("wait".into(), json!(false));
    }
    let history = args.flag("--history");
    let watch = args.optional("--watch", |word| !word.starts_with('-'));
    let diff = args.optional("--diff", |word| !word.starts_with('-') && !is_markdown(word));
    let kind = match args.positional() {
        None => "term".to_string(),
        Some(kind) if ["term", "web", "ios", "md", "code", "diff", "log", "serial", "run"].contains(&kind.as_str()) => kind,
        Some(other) if is_markdown(&other) => {
            return Err(usage("open", &format!("to show a markdown file use: kanna open md {other}")));
        }
        Some(other) => {
            return Err(usage("open", &format!("unknown pane type \"{other}\" (term, web, ios, md, code, diff, log, serial or run). To run a command use --cmd \"{other}\"")));
        }
    };
    if kind == "web" {
        let url = args.positional().ok_or_else(|| usage("open", "web panes need a URL: kanna open web URL"))?;
        out.insert("url".into(), json!(local_page(&url).unwrap_or(url)));
    }
    if kind == "md" {
        let file = args.positional().ok_or_else(|| usage("open", "markdown panes need a file: kanna open md FILE"))?;
        out.insert("path".into(), json!(absolute(&file)));
    }
    if kind == "code" {
        let file = args.positional().ok_or_else(|| usage("open", "code panes need a file: kanna open code FILE[:LINE]"))?;
        let (file, line) = file_line(&file);
        out.insert("path".into(), json!(absolute(&file)));
        if let Some(line) = line {
            out.insert("line".into(), json!(line));
        }
    }
    if kind == "diff" || kind == "log" {
        let folder = out.remove("cwd").and_then(|cwd| cwd.as_str().map(String::from)).unwrap_or_else(|| absolute("."));
        out.insert("path".into(), json!(folder));
        out.insert("args".into(), json!(args.remaining()));
    }
    let (baud, ending) = (args.option("--baud")?, args.option("--line-ending")?);
    if kind == "serial" {
        let port = args.positional().ok_or_else(|| usage("open", "serial panes need a port: kanna open serial PORT (kanna serial list shows them)"))?;
        out.insert("path".into(), json!(port));
        if let Some(baud) = baud {
            out.insert("baud".into(), json!(baud.parse::<u32>().map_err(|_| usage("open", "--baud takes a number, like 115200"))?));
        }
        if let Some(ending) = ending {
            if !["lf", "crlf", "cr"].contains(&ending.as_str()) {
                return Err(usage("open", "--line-ending is lf, crlf or cr"));
            }
            out.insert("lineEnding".into(), json!(ending));
        }
    } else if baud.is_some() || ending.is_some() {
        return Err(usage("open", "--baud and --line-ending are only for serial panes: kanna open serial PORT --baud N"));
    }
    if kind == "run" {
        let run = args.positional().filter(|r| !r.is_empty());
        if let Some(watch) = watch {
            let mut request = json!({ "action": "watch" });
            if let Some(run) = run {
                request["run"] = json!(run);
            }
            if let Some(worker) = watch {
                request["worker"] = json!(worker);
            }
            if let Some(by) = caller() {
                request["by"] = json!(by);
            }
            if let Ok(here) = std::env::current_dir() {
                request["cwd"] = json!(here.to_string_lossy());
            }
            return Ok(request);
        }
        if let Some(run) = run {
            out.insert("run".into(), json!(run));
        }
        if let Some(by) = caller() {
            out.insert("by".into(), json!(by));
        }
        if let Ok(here) = std::env::current_dir() {
            out.insert("cwd".into(), json!(here.to_string_lossy()));
        }
    } else if watch.is_some() {
        return Err(usage("open", "--watch is only for the Run tab: kanna open run --watch [WORKER]"));
    }
    if kind == "ios" && !out.contains_key("app") {
        return Err(usage("open", "iOS panes need an app: kanna open ios --app MyApp.app (or a bundle ID)"));
    }
    if kind != "ios" && (out.contains_key("app") || out.contains_key("device")) {
        return Err(usage("open", "--app and --device are only for iOS panes: kanna open ios --app APP"));
    }
    if let Some(reference) = diff {
        if kind != "md" {
            return Err(usage("open", "--diff is only for markdown panes: kanna open md FILE --diff REF"));
        }
        let path = out["path"].as_str().unwrap_or_default().to_string();
        out.insert("diff".into(), json!(diff_base(&path, reference)?));
    }
    if history {
        if kind != "web" {
            return Err(usage("open", "--history is only for web panes: kanna open web URL --history"));
        }
        out.insert("history".into(), json!(true));
    }
    out.insert("type".into(), json!(kind));
    from_caller(&mut out);
    Ok(Value::Object(out))
}

fn watching(reply: &Value) -> String {
    let place = whose_text(reply).unwrap_or_else(|| reply["window"].as_str().unwrap_or("its task's workspace").to_string());
    let tab = reply["tab"].as_str().map(|t| format!(", {t}")).unwrap_or_default();
    format!("{} shows {}'s worker in {place}{tab}; the tab follows the run to the next stage", reply["terminal"].as_str().unwrap_or("?"), reply["run"].as_str().unwrap_or("?"))
}

pub fn show(request: &Value, reply: &Value) -> String {
    if request["action"] == "watch" {
        return watching(reply);
    }
    let place = whose_text(reply).unwrap_or_else(|| reply["window"].as_str().unwrap_or("?").to_string());
    if reply["background"] == true {
        return format!("opened {} in the background of {place}: {}", label(&reply["pane"]), state(&reply["pane"]));
    }
    if let Some(edge) = reply["dock"].as_str() {
        let frozen = if request["frozen"] == true { ", frozen," } else { "" };
        return format!("opened {} docked {edge}{frozen} in {place}: {}", label(&reply["pane"]), state(&reply["pane"]));
    }
    format!("opened {} in {place}, {}: {}", label(&reply["pane"]), reply["tab"].as_str().unwrap_or("?"), state(&reply["pane"]))
}

pub fn local_page(target: &str) -> Option<String> {
    if target.contains("://") {
        return None;
    }
    let path = absolute(target);
    std::path::Path::new(&path).is_file().then(|| file_url(&path))
}

pub fn diff_base(path: &str, reference: Option<String>) -> Result<String, Failure> {
    if let Some(reference) = reference {
        return Ok(reference);
    }
    let folder = std::path::Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into());
    ["main", "master"]
        .iter()
        .find(|branch| git_line(&folder, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_some())
        .map(|branch| branch.to_string())
        .ok_or_else(|| usage("open", "--diff compares with main (or master), and there's none here: say what to compare with, e.g. --diff HEAD"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::parse;

    #[test]
    fn a_markdown_pane_marks_changes_since_a_ref_and_a_bare_diff_means_main() {
        assert_eq!(parse("kanna open md /tmp/notes.md --diff HEAD").unwrap().1["diff"], json!("HEAD"));
        let (_, request) = parse("kanna open md --diff abc123 /tmp/notes.md --tab").unwrap();
        assert_eq!((request["path"].clone(), request["diff"].clone()), (json!("/tmp/notes.md"), json!("abc123")));
        assert_eq!(parse("kanna open md /tmp/notes.md --diff=origin/main").unwrap().1["diff"], json!("origin/main"));
        assert_eq!(parse("kanna open md /tmp/notes.md --diff v1..main").unwrap().1["diff"], json!("v1..main"));
        let here = env!("CARGO_MANIFEST_DIR");
        let main = ["main", "master"].into_iter().find(|b| std::process::Command::new("git").args(["-C", here, "rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")]).output().is_ok_and(|o| o.status.success()));
        if let Some(main) = main {
            assert_eq!(parse(&format!("kanna open md {here}/README.md --diff")).unwrap().1["diff"], json!(main));
        }
        assert!(parse("kanna open md /no-repo-here/notes.md --diff").is_err());
        assert!(parse("kanna open web a.test --diff main").is_err());
        assert!(parse("kanna open md /tmp/notes.md").unwrap().1.get("diff").is_none());
    }
}
