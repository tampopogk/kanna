use super::{absolute, caller, repo_here, usage, Command, Flow};
use crate::args::Argv;
use crate::output::{ago, tilde};
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "run",
    group: "Information",
    summary: "Build a task's change through a pipeline: start a run from the task's terminal, list runs, show one, read its log, resume a paused one, or stop it.",
    usage: "kanna run start PIPELINE [--request FILE|TEXT]\n       kanna run list [--all]\n       kanna run show [RUN] [--json]\n       kanna run log [RUN]\n       kanna run stop [RUN]\n       kanna run resume [RUN] [TEXT]\n       kanna run done [pass|fail] [--answer FILE]\n       kanna run need TEXT",
    options: &[
        ("--request FILE|TEXT", "start: what the run builds, given to every agent stage: a file's text, or the text itself. Default: the task's first message."),
        ("--all", "list: every repo's runs, not only this repo's."),
        ("--answer FILE", "done: the stage's final answer. Default: the file in $KANNA_ANSWER, which the stage's prompt names."),
    ],
    notes: "A run builds the change of the task whose terminal starts it, on a branch of its own (run/TASK-N) from the task's branch as it is, in a worker: a worktree (.kanna-worktrees/TASK-run-N) and a terminal in the task's workspace's background, in no tab until you show it with kanna move PANE --tab (kanna move PANE --background puts it back). The daemon runs the pipeline's stages there one at a time: an agent stage as a fresh session with its prompt (.kanna/prompts/NAME.md, the request and the last failure's feedback, written to the worker's .kanna/tmp/stage-N.md), a run stage as a command whose exit 0 passes. It picks each arrow by the stage's verdict, as kanna pipeline check describes. The task's own agent and worktree are left alone while it runs. At done the run hands back: it removes its worktree, keeps its branch and tells the task's agent the branch is ready, with how to take it (git merge --ff-only run/TASK-N in the task's worktree), as it tells it when the run is stuck, waits on you or stops. The task's agent decides whether to merge it; from the task's worktree the change goes to main as any task's work. Closing the task stops its live runs.\ndone and need are for a stage's agent, in its worker's terminal, as its prompt says: done ends its visit (with pass or fail for a stage that gives a verdict), and Kanna quits the agent once its turn ends; need pauses the run, tells the task's agent what the stage needs, and quits the agent keeping its session. A stage whose turn ends without either pauses the run the same way, and the task's agent is told what its screen said. resume carries a paused stage on: its session starts again with TEXT (an answer, or what to do) as its next message. A stage's visit ends only when it reports done (or a command exits); an agent that exits without reporting stops the run. Agent stages take any of Kanna's agents (claude, codex, opencode, agy), which report their status (OSC 7501), so Kanna knows when a turn ends. RUN is a run's ID (run-7f3a1c2e, or a unique start of it); without one, show and log take the latest run of the task the terminal is in, and stop and resume its live run, refusing with the list when it has several. A task can have any number of runs at once, each on its own branch and worktree; each run's budget is its own, and a run over it waits and tells the task's agent. Fan-out, ask, guard, emits and on another machine aren't built yet: a pipeline using them is refused. docs/specs/software-factory.md.",
    examples: &[
        ("kanna run start feature", "Build this task's change through .kanna/pipelines/feature.kojo."),
        ("kanna run start bugfix --request notes/bug.md", "Run bugfix.kojo with the bug's write-up as the request."),
        ("kanna run show --json", "This task's latest run: its revision, every visit with its verdict and answer, and the worker running now."),
        ("kanna run log run-7f3a", "What happened in run-7f3a, oldest first."),
        ("kanna run resume \"Use blue.\"", "Answer the stage this task's run paused on; its agent carries on with that."),
        ("kanna run stop", "Stop this task's run where it is; its branch is kept."),
    ],
    parse,
    show,
    flow: Flow::PLAIN,
};

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let request = args.option("--request")?;
    let answer = args.option("--answer")?;
    let all = args.flag("--all");
    let action = args.positional().ok_or_else(|| usage("run", "say what: start, list, show, log, resume or stop"))?;
    let mut out = json!({ "action": action });
    if let Some(by) = caller() {
        out["by"] = json!(by);
    }
    if let Ok(here) = std::env::current_dir() {
        out["cwd"] = json!(here.to_string_lossy());
    }
    if request.is_some() && action != "start" {
        return Err(usage("run", "--request is for run start"));
    }
    if all && action != "list" {
        return Err(usage("run", "--all is for run list"));
    }
    if answer.is_some() && action != "done" {
        return Err(usage("run", "--answer is for run done"));
    }
    match action.as_str() {
        "start" => {
            let pipeline = args.positional().ok_or_else(|| usage("run", "start needs a PIPELINE: a name in .kanna/pipelines or a .kojo file"))?;
            out["pipeline"] = json!(if pipeline.ends_with(".kojo") || pipeline.contains('/') { absolute(&pipeline) } else { pipeline });
            if let Some(request) = request {
                let file = std::path::Path::new(&request);
                out["request"] = json!(if file.is_file() { std::fs::read_to_string(file).map_err(|e| usage("run", &format!("can't read {request}: {e}")))? } else { request });
            }
        }
        "list" => {
            out["all"] = json!(all);
            if let Ok(repo) = repo_here() {
                out["repo"] = json!(repo);
            }
        }
        "done" => {
            if let Some(verdict) = args.positional() {
                if verdict != "pass" && verdict != "fail" {
                    return Err(usage("run", &format!("done takes pass or fail, not \"{verdict}\"")));
                }
                out["verdict"] = json!(verdict);
            }
            let file = answer.or_else(|| std::env::var("KANNA_ANSWER").ok().filter(|f| std::path::Path::new(f).is_file()));
            if let Some(file) = file {
                out["answer"] = json!(std::fs::read_to_string(&file).map_err(|e| usage("run", &format!("can't read the answer {file}: {e}")))?);
            }
        }
        "need" => out["text"] = json!(args.rest().ok_or_else(|| usage("run", "need says what: kanna run need TEXT"))?),
        "resume" => {
            let mut words = args.positionals();
            if words.first().is_some_and(|w| w.starts_with("run-")) {
                out["run"] = json!(words.remove(0));
            }
            if !words.is_empty() {
                out["text"] = json!(words.join(" "));
            }
        }
        "show" | "log" | "stop" => {
            if let Some(run) = args.positional() {
                out["run"] = json!(run);
            }
        }
        other => return Err(usage("run", &format!("unknown action \"{other}\" (start, list, show, log, stop, resume, done or need)"))),
    }
    Ok(out)
}

pub fn show(request: &Value, reply: &Value) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    match request["action"].as_str() {
        Some("start") => format!(
            "{} runs {} for {} on {} (worst case {} sessions); `kanna run show {}` follows it",
            reply["run"].as_str().unwrap_or("?"),
            reply["pipeline"].as_str().unwrap_or("?"),
            reply["task"].as_str().unwrap_or("?"),
            reply["branch"].as_str().unwrap_or("?"),
            reply["worst"],
            reply["run"].as_str().unwrap_or("?")
        ),
        Some("stop") => format!("{} is stopped; its branch is kept", reply["run"].as_str().unwrap_or("?")),
        Some("done") => format!("{}: {} reported {}; Kanna ends this session now", reply["run"].as_str().unwrap_or("?"), reply["stage"].as_str().unwrap_or("?"), reply["verdict"].as_str().unwrap_or("done")),
        Some("need") => format!("{}: {} is paused; the task's agent is told what it needs, and Kanna ends this session once your turn ends, keeping it to resume", reply["run"].as_str().unwrap_or("?"), reply["stage"].as_str().unwrap_or("?")),
        Some("resume") => format!("{}: {} carries on", reply["run"].as_str().unwrap_or("?"), reply["stage"].as_str().unwrap_or("?")),
        Some("list") => {
            let runs = reply["runs"].as_array().cloned().unwrap_or_default();
            if runs.is_empty() {
                return "no runs".into();
            }
            runs.iter().map(|r| run_line(r, now)).collect::<Vec<_>>().join("\n")
        }
        Some("log") => reply["events"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| {
                let at = e["at"].as_f64().unwrap_or(0.0);
                let doc = &e["doc"];
                let detail = match e["what"].as_str() {
                    Some("stage") => format!("{} (visit {}) in {}", doc["stage"].as_str().unwrap_or("?"), doc["visit"], doc["terminal"].as_str().unwrap_or("-")),
                    Some("verdict") => format!("{} {} → {}", doc["stage"].as_str().unwrap_or("?"), doc["verdict"].as_str().unwrap_or("?"), doc["next"].as_str().unwrap_or("no arrow")),
                    Some("started") => format!("{} on {}", doc["pipeline"].as_str().unwrap_or("?"), doc["branch"].as_str().unwrap_or("?")),
                    Some("handed_back") => doc["note"].as_str().unwrap_or("").to_string(),
                    _ => doc["why"].as_str().unwrap_or("").to_string(),
                };
                format!("{}  {:<11} {detail}", ago(now - at), e["what"].as_str().unwrap_or("?"))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => {
            let run = &reply["run"];
            let mut out = vec![run_line(run, now)];
            out.push(format!("branch {}  worktree {}", run["branch"].as_str().unwrap_or("?"), tilde(run["worktree"].as_str().unwrap_or("?"))));
            if let Some(note) = run["note"].as_str() {
                out.push(format!("handed back: {note}"));
            }
            for visit in reply["visits"].as_array().into_iter().flatten() {
                let ended = visit["endedAt"].as_f64().unwrap_or(now);
                let took = span(ended - visit["startedAt"].as_f64().unwrap_or(ended));
                let verdict = visit["verdict"].as_str().unwrap_or("running");
                let answer = visit["doc"]["answer"].as_str().or(visit["doc"]["feedback"].as_str()).unwrap_or("").lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(72).collect::<String>();
                out.push(format!("  {:<10} {:<2} {:<8} {:<11} {took:>8}  {answer}", visit["stage"].as_str().unwrap_or("?"), visit["visit"], visit["kind"].as_str().unwrap_or("?"), verdict));
            }
            for worker in reply["workers"].as_array().into_iter().flatten() {
                out.push(format!("worker: {} in {}", worker["stage"].as_str().unwrap_or("?"), worker["terminal"].as_str().unwrap_or("-")));
            }
            out.join("\n")
        }
    }
}

fn run_line(run: &Value, now: f64) -> String {
    let id = run["run"].as_str().unwrap_or("?");
    let task: String = run["task"].as_str().unwrap_or("?").chars().take(9).collect();
    let state = run["state"].as_str().unwrap_or("?");
    let stage = match (run["stageOpen"].as_bool(), run["stage"].as_str(), run["next"].as_str()) {
        (Some(true), Some(stage), _) => stage.to_string(),
        (_, _, Some(next)) => format!("next {next}"),
        (_, Some(stage), _) => format!("last {stage}"),
        _ => "-".into(),
    };
    let ended = run["endedAt"].as_f64().unwrap_or(now);
    let took = span(ended - run["startedAt"].as_f64().unwrap_or(ended));
    let outcome = run["outcome"].as_str().map_or(String::new(), |o| format!("  {o}"));
    let why = run["why"].as_str().filter(|_| state != "done").map_or(String::new(), |w| format!(": {w}"));
    format!("{id}  {task}  {:<10} {:<8} {stage}  {} visits  {took}{outcome}{why}", run["pipeline"].as_str().unwrap_or("?"), state, run["visits"].as_u64().unwrap_or(0))
}

fn span(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    match (s / 3600, s % 3600 / 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s:02}s"),
        (h, m, _) => format!("{h}h {m:02}m"),
    }
}
