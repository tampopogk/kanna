use crate::db::{Revision, RunEvent, RunRow, StageRow};
use kanna_mux::json::{json, num, opt, string, Value};
use kanna_pipe::{Kind, Pipeline, Verdict, DONE};

pub const INTERVENTIONS: &[&str] = &["answered", "paused", "resumed", "skipped", "retried", "typed", "stopped", "revision"];

pub struct Inputs<'a> {
    pub row: &'a RunRow,
    pub pipeline: &'a Pipeline,
    pub stages: &'a [StageRow],
    pub events: &'a [RunEvent],
    pub revision: Option<&'a Revision>,
    pub task_name: Option<&'a str>,
    pub lines: Option<(u64, u64, u64)>,
    pub now: f64,
}

fn counted(stage: &StageRow) -> bool {
    stage.verdict.as_deref() != Some("interrupted")
}

fn node_state(stage: Option<&StageRow>, run_state: &str) -> Option<&'static str> {
    let stage = stage?;
    if stage.ended_at.is_none() {
        return Some(if run_state == "waiting" || run_state == "paused" { "wait" } else { "now" });
    }
    match stage.verdict.as_deref() {
        Some("pass" | "done" | "yes" | "skipped") => Some("pass"),
        Some(_) => Some("fail"),
        None => None,
    }
}

pub fn who(pipeline: &Pipeline, stage: &StageRow) -> String {
    match pipeline.stage(&stage.stage).map(|s| &s.kind) {
        Some(Kind::Agent(agent)) => match agent.effort.as_deref().or(agent.model.as_deref()) {
            Some(more) => format!("{}:{more}", agent.agent),
            None => agent.agent.clone(),
        },
        Some(_) => pipeline.stage(&stage.stage).map(kanna_pipe::who).unwrap_or_default(),
        None => stage.agent.clone().unwrap_or_else(|| stage.kind.clone()),
    }
}

fn first_line(text: &str, most: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("```")).unwrap_or("");
    let line = line.trim_start_matches('#').trim();
    if line.chars().count() <= most {
        return line.to_string();
    }
    format!("{}…", line.chars().take(most).collect::<String>().trim_end())
}

fn note(stage: &StageRow) -> String {
    let doc = &stage.doc;
    if let Some(note) = string(doc, "note").filter(|n| !n.is_empty()) {
        return note.to_string();
    }
    if stage.verdict.as_deref() == Some("fail") {
        let failures: Vec<&str> = doc.get("failures").and_then(Value::as_array).map_or(vec![], |f| f.iter().filter_map(Value::as_str).collect());
        if !failures.is_empty() {
            return first_line(&failures.iter().take(2).copied().collect::<Vec<_>>().join("; "), 120);
        }
    }
    first_line(string(doc, "answer").unwrap_or(""), 120)
}

fn sum(stages: &[StageRow], key: &str) -> Value {
    let agents: Vec<&StageRow> = stages.iter().filter(|s| s.kind == "agent" && counted(s)).collect();
    if agents.is_empty() || agents.iter().any(|s| s.doc.get(key).and_then(Value::as_f64).is_none()) {
        return Value::Null;
    }
    num(agents.iter().filter_map(|s| s.doc.get(key).and_then(Value::as_f64)).sum())
}

pub fn place(pipeline: &Pipeline, row: &RunRow, stages: &[StageRow]) -> (String, usize, usize) {
    let of = pipeline.stages.len();
    if row.state == "done" {
        return (DONE.into(), of, of);
    }
    let open = stages.iter().rfind(|s| s.ended_at.is_none()).map(|s| s.stage.clone());
    let stage = open.or_else(|| string(&row.doc, "next").map(String::from)).or_else(|| stages.last().map(|s| s.stage.clone())).unwrap_or_else(|| pipeline.start.clone());
    let at = pipeline.stages.iter().position(|s| s.name == stage).map_or(0, |i| i + 1);
    (stage, at, of)
}

fn card(input: &Inputs, open: Option<&StageRow>) -> Value {
    let row = input.row;
    let why = string(&row.doc, "why").unwrap_or("");
    if let Some(open) = open {
        if let Some(Kind::Ask { question }) = input.pipeline.stage(&open.stage).map(|s| &s.kind) {
            return json!({ "kind": "ask", "stage": open.stage, "visit": open.visit, "title": question, "detail": format!("{} · visit {} · your answer picks the arrow", open.stage, open.visit) });
        }
    }
    match row.state.as_str() {
        "paused" => {
            let stage = open.map_or_else(String::new, |s| s.stage.clone());
            let title = match why.strip_prefix(&format!("{stage} needs: ")) {
                Some(rest) => format!("{stage} needs you: {rest}"),
                None => why.to_string(),
            };
            let visit = open.map_or(0, |s| s.visit);
            json!({ "kind": "need", "stage": stage, "visit": visit, "title": title, "detail": format!("{stage} · visit {visit} · your answer resumes it") })
        }
        "waiting" => json!({ "kind": "wait", "stage": opt(open.map(|s| s.stage.as_str())), "title": why, "detail": "the run waits for you: stop it, or change its budget" }),
        _ => Value::Null,
    }
}

pub fn notice(input: &Inputs) -> Value {
    let (row, pipeline) = (input.row, input.pipeline);
    let stages: Vec<&StageRow> = input.stages.iter().filter(|s| counted(s)).collect();
    let open = input.stages.iter().rfind(|s| s.ended_at.is_none());
    let graph = kanna_pipe::layout(pipeline);
    let taken: Vec<u64> = row.doc.get("taken").and_then(Value::as_array).map_or(vec![], |t| t.iter().map(|n| n.as_u64().unwrap_or(0)).collect());
    let nodes: Vec<Value> = graph
        .nodes
        .iter()
        .map(|n| {
            let mine: Vec<&&StageRow> = stages.iter().filter(|s| s.stage == n.id).collect();
            let state = if n.id == DONE { (row.state == "done").then_some("pass") } else { node_state(mine.last().map(|s| **s), &row.state) };
            json!({ "id": n.id, "kind": n.kind, "who": n.who, "col": n.col, "row": n.row, "state": opt(state), "visits": mine.len() })
        })
        .collect();
    let edges: Vec<Value> = graph
        .edges
        .iter()
        .map(|e| json!({ "from": e.from, "to": e.to, "route": e.route.name(), "label": e.label, "fail": e.fail, "lane": e.lane, "lanes": e.lanes, "taken": taken.get(e.arrow).copied().unwrap_or(0) > 0 }))
        .collect();
    let visits: Vec<Value> = stages
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "n": i + 1, "stage": s.stage, "visit": s.visit, "kind": s.kind, "who": who(pipeline, s),
                "startedAt": num(s.started_at), "endedAt": s.ended_at.map_or(Value::Null, num),
                "verdict": opt(s.verdict.as_deref()), "turns": s.doc.get("turns").cloned().unwrap_or(Value::Null),
                "tokens": s.doc.get("tokens").cloned().unwrap_or(Value::Null), "note": note(s), "terminal": opt(s.terminal.as_deref()),
            })
        })
        .collect();
    let rework: u64 = pipeline.arrows.iter().enumerate().filter(|(_, a)| a.when == Some(Verdict::Fail)).map(|(i, _)| taken.get(i).copied().unwrap_or(0)).sum();
    let interventions = input.events.iter().filter(|e| INTERVENTIONS.contains(&e.what.as_str()) && e.by.as_deref().is_some_and(|by| by != "daemon")).count();
    let waited: f64 = input.stages.iter().map(|s| s.doc.get("waited").and_then(Value::as_f64).unwrap_or(0.0)).sum();
    let waiting_since = match row.state.as_str() {
        "paused" | "waiting" => input.events.iter().rfind(|e| e.what == "paused" || e.what == "waiting").map_or(Value::Null, |e| num(e.at)),
        _ => Value::Null,
    };
    let (stage, at, of) = place(pipeline, row, input.stages);
    let by_agent = input.events.iter().find(|e| e.what == "started").is_some_and(|e| e.doc.get("agent") == Some(&Value::Bool(true)));
    let budget = row.doc.get("budget").cloned().unwrap_or(Value::Null);
    json!({
        "content": row.id,
        "run": row.id,
        "task": row.task,
        "name": opt(input.task_name),
        "pipeline": row.pipeline,
        "branch": row.branch,
        "state": row.state,
        "outcome": opt(row.outcome.as_deref()),
        "why": opt(string(&row.doc, "why")),
        "startedAt": num(row.started_at),
        "endedAt": row.ended_at.map_or(Value::Null, num),
        "now": num(input.now),
        "startedBy": if by_agent { "the task's agent" } else { "you" },
        "revision": input.revision.map_or(Value::Null, |r| json!({ "n": r.n, "by": r.by, "at": num(r.at), "hash": r.hash })),
        "place": { "stage": stage, "at": at, "of": of },
        "graph": { "columns": graph.columns, "rows": graph.rows, "nodes": nodes, "edges": edges },
        "numbers": {
            "visits": stages.len(),
            "sessions": row.doc.get("sessions").cloned().unwrap_or(json!(0)),
            "budget": budget.get("sessions").cloned().unwrap_or(Value::Null),
            "waited": num(waited),
            "waitingSince": waiting_since,
            "turns": sum(input.stages, "turns"),
            "tokens": sum(input.stages, "tokens"),
            "cost": sum(input.stages, "cost"),
            "rework": rework,
            "interventions": interventions,
            "lines": input.lines.map_or(Value::Null, |(added, removed, files)| json!({ "added": added, "removed": removed, "files": files })),
        },
        "visits": visits,
        "workers": input.stages.iter().filter(|s| s.ended_at.is_none() && !s.doc.get("paused").is_some_and(Value::is_object) && s.terminal.is_some()).map(|s| json!({ "stage": s.stage, "visit": s.visit, "instance": opt(s.instance.as_deref()), "terminal": opt(s.terminal.as_deref()), "machine": s.machine, "worktree": opt(s.worktree.as_deref()), "who": who(pipeline, s), "startedAt": num(s.started_at) })).collect::<Vec<_>>(),
        "card": card(input, open),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature() -> Pipeline {
        kanna_pipe::parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.kanna/pipelines/feature.kojo")).unwrap()).unwrap()
    }

    fn visit(stage: &str, n: i64, verdict: Option<&str>, kind: &str) -> StageRow {
        StageRow { id: 0, run: "run-1".into(), stage: stage.into(), visit: n, instance: None, kind: kind.into(), agent: None, machine: "mbp".into(), started_at: 100.0, ended_at: verdict.map(|_| 160.0), verdict: verdict.map(String::from), session: None, terminal: Some("term-9".into()), worktree: None, branch: None, doc: json!({ "answer": "## Plan\n3 parts: tokens, split view\n" }) }
    }

    fn row(state: &str, taken: Vec<u32>) -> RunRow {
        RunRow { id: "run-7f3a1c2e".into(), task: "task-7f3a".into(), repo: "/r".into(), pipeline: "feature".into(), revision_hash: "h".into(), branch: "run/task-7f3a-1".into(), state: state.into(), started_at: 0.0, ended_at: None, outcome: None, doc: json!({ "taken": taken, "sessions": 8, "budget": { "sessions": 40 }, "why": "fix needs: Which theme?" }) }
    }

    fn event(what: &str, by: &str) -> RunEvent {
        RunEvent { at: 50.0, what: what.into(), by: Some(by.into()), doc: Value::Null }
    }

    #[test]
    fn the_run_scene_marks_each_stage_counts_its_visits_and_places_the_badge() {
        let pipeline = feature();
        let taken = vec![1, 1, 1, 1, 2, 2, 1, 0, 0, 0];
        let stages = [
            visit("plan", 1, Some("done"), "agent"),
            visit("build", 1, Some("done"), "agent"),
            visit("join", 1, Some("pass"), "merge"),
            visit("check", 1, Some("fail"), "run"),
            visit("fix", 1, Some("done"), "agent"),
            visit("check", 2, Some("pass"), "run"),
            visit("review", 1, Some("fail"), "agent"),
            visit("fix", 2, Some("interrupted"), "agent"),
            visit("fix", 2, Some("done"), "agent"),
            visit("check", 3, Some("pass"), "run"),
            visit("review", 2, None, "agent"),
        ];
        let row = row("running", taken);
        let mut started = event("started", "term-1");
        started.doc = json!({ "agent": true });
        let events = [started, event("typed", "user"), event("stage", "daemon")];
        let out = notice(&Inputs { row: &row, pipeline: &pipeline, stages: &stages, events: &events, revision: None, task_name: Some("Dark mode"), lines: Some((212, 48, 3)), now: 200.0 });
        let marks: Vec<(String, Value, u64)> = out["graph"]["nodes"].as_array().unwrap().iter().map(|n| (n["id"].as_str().unwrap().to_string(), n["state"].clone(), n["visits"].as_u64().unwrap())).collect();
        assert_eq!(
            marks,
            [
                ("plan".into(), json!("pass"), 1),
                ("build".into(), json!("pass"), 1),
                ("join".into(), json!("pass"), 1),
                ("check".into(), json!("pass"), 3),
                ("review".into(), json!("now"), 2),
                ("e2e".into(), Value::Null, 0),
                ("done".into(), Value::Null, 0),
                ("fix".into(), json!("pass"), 2),
            ]
        );
        let taken: Vec<String> = out["graph"]["edges"].as_array().unwrap().iter().filter(|e| e["taken"] == true).map(|e| format!("{}>{}", e["from"].as_str().unwrap(), e["to"].as_str().unwrap())).collect();
        assert_eq!(taken, ["plan>build", "build>join", "join>check", "check>fix", "fix>check", "check>review", "review>fix"]);
        assert_eq!(out["place"], json!({ "stage": "review", "at": 5, "of": 7 }));
        assert_eq!(out["startedBy"], "the task's agent");
        let numbers = &out["numbers"];
        assert_eq!((numbers["visits"].as_u64(), numbers["rework"].as_u64(), numbers["interventions"].as_u64()), (Some(10), Some(2), Some(1)));
        assert_eq!((numbers["sessions"].clone(), numbers["budget"].clone(), numbers["tokens"].clone(), numbers["turns"].clone()), (json!(8), json!(40), Value::Null, Value::Null));
        assert_eq!(numbers["lines"], json!({ "added": 212, "removed": 48, "files": 3 }));
        let rows = out["visits"].as_array().unwrap();
        assert_eq!(rows.len(), 10);
        assert_eq!((rows[0]["n"].as_u64(), rows[0]["who"].as_str(), rows[0]["note"].as_str()), (Some(1), Some("claude"), Some("Plan")));
        assert_eq!(rows[6]["who"], "codex:high");
        assert_eq!(out["workers"][0]["stage"], "review");
        assert_eq!(out["card"], Value::Null);
    }

    #[test]
    fn a_paused_run_shows_the_need_as_a_card_and_its_stage_waiting() {
        let pipeline = feature();
        let stages = [visit("plan", 1, Some("done"), "agent"), visit("fix", 1, None, "agent")];
        let mut row = row("paused", vec![1]);
        row.doc["why"] = json!("fix needs: Which theme?");
        let out = notice(&Inputs { row: &row, pipeline: &pipeline, stages: &stages, events: &[event("started", "user"), event("paused", "term-9")], revision: None, task_name: None, lines: None, now: 200.0 });
        assert_eq!(out["card"]["title"], "fix needs you: Which theme?");
        assert_eq!(out["card"]["kind"], "need");
        assert_eq!(out["numbers"]["waitingSince"], 50.0);
        assert_eq!(out["numbers"]["lines"], Value::Null);
        assert_eq!(out["startedBy"], "you");
        let fix = out["graph"]["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "fix").unwrap().clone();
        assert_eq!(fix["state"], "wait");
        assert_eq!(out["place"], json!({ "stage": "fix", "at": 6, "of": 7 }));
    }

    #[test]
    fn a_handed_back_run_has_reached_done() {
        let pipeline = feature();
        let out = notice(&Inputs { row: &row("done", vec![]), pipeline: &pipeline, stages: &[], events: &[], revision: None, task_name: None, lines: None, now: 0.0 });
        assert_eq!(out["place"], json!({ "stage": "done", "at": 7, "of": 7 }));
        assert_eq!(out["graph"]["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "done").unwrap()["state"], "pass");
    }
}
