use crate::diagrams::shown;
use crate::target::Target;
use kanna_client::{exit, fail, Failure};
use kanna_pipe::{Context, Error, Pipeline, Rule, Sizes, ASSUMED_PARTS};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Checked {
    pub file: String,
    pub name: String,
    pub pipeline: Option<Pipeline>,
    pub errors: Vec<Error>,
    pub warnings: Vec<Error>,
}

pub fn run(request: &Value, json_output: bool, target: &Target) -> Result<(), Failure> {
    let places = places(target);
    match request["action"].as_str() {
        Some("list") => list(json_output, &places),
        _ => check(request["pipeline"].as_str().unwrap_or_default(), json_output, &places),
    }
}

fn check(pipeline: &str, json_output: bool, places: &Context) -> Result<(), Failure> {
    let path = locate(pipeline)?;
    let checked = check_file(&path, places)?;
    if json_output {
        println!("{}", describe(&checked));
    } else {
        for problem in &checked.errors {
            println!("{}", problem.at(&checked.file));
        }
        for problem in &checked.warnings {
            println!("{}:{}: warning: {}", checked.file, problem.line, problem.message);
        }
        if checked.errors.is_empty() {
            println!("{}: valid: {}", checked.file, summary(&checked));
        }
    }
    match checked.errors.len() {
        0 => Ok(()),
        1 => Err(fail(exit::FAILED, format!("{}: 1 error", checked.file))),
        n => Err(fail(exit::FAILED, format!("{}: {n} errors", checked.file))),
    }
}

fn list(json_output: bool, places: &Context) -> Result<(), Failure> {
    let dir = repo()?.join(".kanna/pipelines");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).map(|d| d.filter_map(|e| Some(e.ok()?.path())).filter(|p| p.extension().is_some_and(|x| x == "pipe")).collect()).unwrap_or_default();
    files.sort();
    let checked = files.iter().map(|path| check_file(path, places)).collect::<Result<Vec<_>, _>>()?;
    if json_output {
        println!("{}", Value::Array(checked.iter().map(describe).collect()));
        return Ok(());
    }
    if checked.is_empty() {
        println!("no pipelines: add one as {}/NAME.pipe", shown(&dir.to_string_lossy()));
        return Ok(());
    }
    let width = checked.iter().map(|c| c.name.len()).max().unwrap_or(0);
    for one in &checked {
        let state = match one.errors.len() {
            0 => summary(one),
            1 => format!("invalid: 1 error (kanna pipeline check {})", one.name),
            n => format!("invalid: {n} errors (kanna pipeline check {})", one.name),
        };
        let warned = match one.warnings.len() {
            0 => String::new(),
            1 => format!(" · 1 warning (kanna pipeline check {})", one.name),
            n => format!(" · {n} warnings (kanna pipeline check {})", one.name),
        };
        println!("{:<width$}  {state}{warned}", one.name);
    }
    Ok(())
}

pub fn check_file(path: &Path, places: &Context) -> Result<Checked, Failure> {
    let file = shown(&path.to_string_lossy());
    let text = std::fs::read_to_string(path).map_err(|e| fail(exit::NOT_FOUND, format!("pipeline: can't read {file}: {e}")))?;
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let context = Context { prompts: prompts(path), ..places.clone() };
    let (pipeline, found) = match kanna_pipe::parse(&text) {
        Err(errors) => (None, errors),
        Ok(pipeline) => {
            let errors = pipeline.check(&context).err().unwrap_or_default();
            (Some(pipeline), errors)
        }
    };
    let (warnings, errors) = found.into_iter().partition(|e| e.rule == Rule::Placement);
    Ok(Checked { file, name, pipeline, errors, warnings })
}

fn summary(checked: &Checked) -> String {
    let Some(pipeline) = &checked.pipeline else { return String::new() };
    let worst = pipeline.max_sessions(&Sizes::new());
    let budget = pipeline.budget.as_ref();
    let mut out = format!("{} stages, worst case {worst} sessions", pipeline.stages.len());
    if let Some(sessions) = budget.and_then(|b| b.sessions) {
        out += &format!(" of {sessions}");
    }
    if let Some(time) = budget.and_then(|b| b.time) {
        out += &format!(", {}", duration(time));
    }
    let fanouts: Vec<&str> = pipeline.fanouts().iter().map(|s| s.name.as_str()).collect();
    if !fanouts.is_empty() {
        out += &format!(" (counting {ASSUMED_PARTS} parts for each fan-out: {})", fanouts.join(", "));
    }
    out
}

fn duration(time: Duration) -> String {
    let (h, m) = (time.as_secs() / 3600, time.as_secs() % 3600 / 60);
    match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
}

fn describe(checked: &Checked) -> Value {
    let problems = |list: &[Error]| list.iter().map(|e| json!({ "line": e.line, "message": e.message })).collect::<Vec<_>>();
    let pipeline = checked.pipeline.as_ref().filter(|_| checked.errors.is_empty());
    json!({
        "name": checked.name,
        "file": checked.file,
        "valid": checked.errors.is_empty(),
        "errors": problems(&checked.errors),
        "warnings": problems(&checked.warnings),
        "stages": pipeline.map(|p| p.stages.len()),
        "worst": pipeline.map(|p| p.max_sessions(&Sizes::new())),
        "sessions": pipeline.and_then(|p| p.budget.as_ref()?.sessions),
        "time": pipeline.and_then(|p| p.budget.as_ref()?.time).map(|t| t.as_secs()),
        "fanouts": pipeline.map(|p| p.fanouts().iter().map(|s| s.name.clone()).collect::<Vec<_>>()),
    })
}

fn locate(pipeline: &str) -> Result<PathBuf, Failure> {
    if pipeline.ends_with(".pipe") || pipeline.contains('/') {
        return Ok(PathBuf::from(pipeline));
    }
    let path = repo()?.join(format!(".kanna/pipelines/{pipeline}.pipe"));
    if !path.is_file() {
        return Err(fail(exit::NOT_FOUND, format!("no pipeline {pipeline}: no {} (kanna pipeline list shows the repo's)", shown(&path.to_string_lossy()))));
    }
    Ok(path)
}

fn repo() -> Result<PathBuf, Failure> {
    let out = std::process::Command::new("git").args(["rev-parse", "--show-toplevel"]).output().ok().filter(|o| o.status.success());
    out.map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .ok_or_else(|| fail(exit::NOT_FOUND, "pipeline: not in a git repo; a repo's pipelines are its .kanna/pipelines/*.pipe"))
}

fn prompts(pipeline: &Path) -> std::collections::BTreeMap<String, String> {
    let dir = pipeline.parent().and_then(Path::parent).map(|kanna| kanna.join("prompts"));
    let entries = dir.and_then(|dir| std::fs::read_dir(dir).ok());
    entries
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), std::fs::read_to_string(&p).ok()?)))
        .collect()
}

fn places(target: &Target) -> Context {
    let status = target.try_connect().and_then(|mut mux| mux.call("machine", json!({ "action": "status" })).ok());
    let Some(status) = status else { return Context::default() };
    let peers = status["peers"].as_array().into_iter().flatten().filter(|p| p["revoked"] != true);
    Context {
        machine: status["name"].as_str().map(String::from),
        machines: peers.filter_map(|p| p["name"].as_str().map(String::from)).collect(),
        ..Context::default()
    }
}
