use crate::db::{RunEvent, RunRow, Sample, StageRow};
use crate::stats::{section, tile, Commit};
use kanna_mux::json::{json, string, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const CAVEAT: &str = "Bugs traced back are SZZ's guesses: its F1 is about 0.6, so read them across many runs, not for one. A Fixes: trailer makes a trace exact.";

const SERIES: usize = 3;

const RECENT: usize = 8;

const HOUR: f64 = 3600.0;

const NOT_INTERVENTIONS: &[&str] = &["started", "stage", "verdict", "handed_back", "waiting"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quality {
    pub bugs: Option<u64>,
    pub covered: Option<(u64, u64)>,
    pub erosion: Option<f64>,
    pub verbosity: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Numbers {
    pub run: String,
    pub task: String,
    pub name: String,
    pub pipeline: String,
    pub hash: String,
    pub revision: usize,
    pub started: f64,
    pub ended: Option<f64>,
    pub live: bool,
    pub outcome: String,
    pub why: Option<String>,
    pub at: Option<(String, i64)>,
    pub to_hand_back: Option<f64>,
    pub to_land: Option<f64>,
    pub active: f64,
    pub waited: f64,
    pub visits: usize,
    pub sessions: u64,
    pub turns: u64,
    pub tools: u64,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub written: u64,
    pub cost: Option<f64>,
    pub rework: u64,
    pub rework_by: BTreeMap<String, u64>,
    pub findings: Option<BTreeMap<String, u64>>,
    pub failing: u64,
    pub failed_runs: u64,
    pub interventions: BTreeMap<String, u64>,
    pub commits: u64,
    pub files: u64,
    pub added: u64,
    pub deleted: u64,
    pub landing: Option<String>,
    pub quality: Quality,
}

fn median(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut values: Vec<f64> = values.collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 1 { values[middle] } else { (values[middle - 1] + values[middle]) / 2.0 })
}

fn number(doc: &Value, key: &str) -> u64 {
    doc.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn intervention(event: &RunEvent, stages: &[StageRow]) -> Option<String> {
    let by = event.by.as_deref()?;
    if NOT_INTERVENTIONS.contains(&event.what.as_str()) || by == "daemon" || stages.iter().any(|s| s.terminal.as_deref() == Some(by)) {
        return None;
    }
    if event.what == "revision" {
        return (by == "user").then(|| "edited".to_string());
    }
    Some(event.what.clone())
}

pub fn visit_interventions(stage: &StageRow, stages: &[StageRow], events: &[RunEvent]) -> BTreeMap<String, u64> {
    let mut by = BTreeMap::new();
    let holds = |event: &RunEvent| event.at >= stage.started_at && stage.ended_at.is_none_or(|end| event.at <= end);
    for kind in events.iter().filter(|e| holds(e)).filter_map(|e| intervention(e, stages)) {
        *by.entry(kind).or_insert(0) += 1;
    }
    by
}

pub fn numbers(row: &RunRow, stages: &[StageRow], events: &[RunEvent], revision: usize, now: f64) -> Numbers {
    let live = crate::runs::LIVE.contains(&row.state.as_str());
    let handed_back = row.outcome.as_deref() == Some("handed back");
    let landing = string(&row.doc, "landing").map(String::from);
    let landed_at = row.doc.get("landedAt").and_then(Value::as_f64);
    let mut out = Numbers {
        run: row.id.clone(),
        task: row.task.clone(),
        name: string(&row.doc, "taskName").unwrap_or(&row.task).to_string(),
        pipeline: row.pipeline.clone(),
        hash: row.revision_hash.clone(),
        revision,
        started: row.started_at,
        ended: row.ended_at,
        live,
        outcome: if landing.is_some() { "landed".to_string() } else { row.outcome.clone().unwrap_or_else(|| row.state.clone()) },
        why: string(&row.doc, "why").map(String::from),
        to_hand_back: row.ended_at.filter(|_| handed_back).map(|end| end - row.started_at),
        to_land: landed_at.map(|at| at - row.started_at),
        landing,
        visits: stages.len(),
        ..Numbers::default()
    };
    let mut seen: BTreeMap<&str, u64> = BTreeMap::new();
    for stage in stages {
        let doc = &stage.doc;
        let waited = doc.get("waited").and_then(Value::as_f64).unwrap_or(0.0);
        out.waited += waited;
        out.active += (stage.ended_at.unwrap_or(now) - stage.started_at - waited).max(0.0);
        let visits = seen.entry(&stage.stage).or_insert(0);
        *visits += 1;
        if *visits > 1 {
            out.rework += 1;
            *out.rework_by.entry(stage.stage.clone()).or_insert(0) += 1;
        }
        if stage.kind == "agent" {
            out.sessions += 1;
        }
        if let Some(usage) = doc.get("usage").filter(|u| u.is_object()) {
            out.turns += number(usage, "turns");
            out.tools += number(usage, "tools");
            out.input += number(usage, "in");
            out.output += number(usage, "out");
            out.cached += number(usage, "cached");
            out.written += number(usage, "written");
            if let Some(cost) = usage.get("cost").and_then(Value::as_f64) {
                out.cost = Some(out.cost.unwrap_or(0.0) + cost);
            }
        }
        if let Some(findings) = doc.get("findings").and_then(Value::as_object) {
            let sum = out.findings.get_or_insert_with(BTreeMap::new);
            for (severity, count) in findings {
                *sum.entry(severity.clone()).or_insert(0) += count.as_u64().unwrap_or(0);
            }
        }
        out.failing += number(doc, "failing");
        if stage.kind == "run" && stage.verdict.as_deref() == Some("fail") {
            out.failed_runs += 1;
        }
        out.commits += number(doc, "commits");
        out.files += number(doc, "files");
        out.added += number(doc, "added");
        out.deleted += number(doc, "deleted");
    }
    out.at = stages.last().filter(|s| s.ended_at.is_none()).map(|s| (s.stage.clone(), seen.get(s.stage.as_str()).copied().unwrap_or(1) as i64));
    let kept: BTreeMap<String, u64> = row.doc.get("interventions").and_then(Value::as_object).map_or_else(BTreeMap::new, |m| m.iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0))).filter(|(_, n)| *n > 0).collect());
    if kept.is_empty() {
        for kind in events.iter().filter_map(|e| intervention(e, stages)) {
            *out.interventions.entry(kind).or_insert(0) += 1;
        }
    } else {
        out.interventions = kept;
    }
    out
}

impl Numbers {
    pub fn tokens(&self) -> u64 {
        self.input + self.written + self.output
    }

    fn found(&self) -> Option<u64> {
        self.findings.as_ref().map(|f| f.values().sum())
    }

    fn intervened(&self) -> u64 {
        self.interventions.values().sum()
    }

    fn covered(&self) -> Option<f64> {
        self.quality.covered.filter(|(_, found)| *found > 0).map(|(hit, found)| hit as f64 * 100.0 / found as f64)
    }

    pub fn value(&self) -> Value {
        json!({
            "run": self.run, "task": self.task, "name": self.name, "pipeline": self.pipeline, "hash": self.hash, "revision": self.revision,
            "startedAt": self.started, "endedAt": self.ended, "outcome": self.outcome, "why": self.why, "landing": self.landing,
            "timeToHandBack": self.to_hand_back, "timeToLand": self.to_land, "activeTime": self.active, "waited": self.waited,
            "visits": self.visits, "sessions": self.sessions, "turns": self.turns, "tools": self.tools,
            "tokens": { "total": self.tokens(), "in": self.input, "out": self.output, "cached": self.cached, "written": self.written }, "cost": self.cost,
            "reworkLoops": self.rework, "reworkByStage": self.rework_by, "findings": self.findings, "failingTests": self.failing, "failedRuns": self.failed_runs,
            "interventions": self.interventions, "commits": self.commits, "files": self.files, "added": self.added, "deleted": self.deleted,
            "bugsTracedBack": self.quality.bugs, "changedLinesCovered": self.covered(), "qualityDelta": { "erosion": self.quality.erosion, "verbosity": self.quality.verbosity },
        })
    }
}

pub fn join_quality(samples: &[Sample]) -> impl Fn(&str) -> Quality {
    let of = |metric: &str| -> Vec<(String, f64)> { samples.iter().filter(|s| s.metric == metric).map(|s| (s.commit.clone(), s.value)).collect() };
    let deltas = |metric: &str| -> HashMap<String, f64> {
        let rows = of(metric);
        rows.windows(2).map(|pair| (pair[1].0.clone(), ((pair[1].1 - pair[0].1) * 10.0).round() / 10.0)).collect()
    };
    let (erosion, verbosity) = (deltas("erosion"), deltas("verbosity"));
    let mut bugs: HashMap<String, u64> = HashMap::new();
    for fix in samples.iter().filter(|s| s.metric == "fixes") {
        if let Some(blamed) = string(&fix.doc, "blamed") {
            *bugs.entry(blamed.to_string()).or_insert(0) += 1;
        }
    }
    let sampled: BTreeSet<String> = samples.iter().filter(|s| s.metric == "fixes").map(|s| s.commit.clone()).collect();
    let mut covered: HashMap<String, (u64, u64)> = HashMap::new();
    if let Some(last) = samples.iter().rfind(|s| s.metric == "coverage") {
        for landing in last.doc.get("landings").and_then(Value::as_array).into_iter().flatten() {
            if let Some(commit) = string(landing, "commit") {
                covered.insert(commit.to_string(), (number(landing, "hit"), number(landing, "found")));
            }
        }
    }
    move |landing| Quality {
        bugs: sampled.contains(landing).then(|| bugs.get(landing).copied().unwrap_or(0)),
        covered: covered.get(landing).copied(),
        erosion: erosion.get(landing).copied(),
        verbosity: verbosity.get(landing).copied(),
    }
}

pub fn revisions(rows: &[RunRow]) -> HashMap<(String, String), usize> {
    let mut order: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut sorted: Vec<&RunRow> = rows.iter().collect();
    sorted.sort_by(|a, b| a.started_at.total_cmp(&b.started_at));
    for row in sorted {
        let hashes = order.entry(&row.pipeline).or_default();
        if !hashes.contains(&row.revision_hash.as_str()) {
            hashes.push(&row.revision_hash);
        }
    }
    order.into_iter().flat_map(|(pipeline, hashes)| hashes.into_iter().enumerate().map(move |(i, hash)| ((pipeline.to_string(), hash.to_string()), i + 1))).collect()
}

pub fn landing_of(commits: &[Commit], reaches: impl Fn(&str) -> bool) -> Option<(String, f64)> {
    let oldest_first: Vec<&Commit> = commits.iter().rev().collect();
    let first = oldest_first.partition_point(|commit| !reaches(&commit.id));
    oldest_first.get(first).map(|commit| (commit.id.clone(), commit.at))
}

fn hours(seconds: f64) -> String {
    match seconds {
        s if s < 60.0 => format!("{}s", s.round()),
        s if s < HOUR => format!("{}m", (s / 60.0).round()),
        s => format!("{:.1}h", s / HOUR),
    }
}

fn amount(tokens: f64) -> String {
    match tokens {
        t if t >= 1e6 => format!("{:.1}M", t / 1e6),
        t if t >= 1e3 => format!("{:.0}k", t / 1e3),
        t => format!("{t:.0}"),
    }
}

fn figure(value: Option<f64>, shown: impl Fn(f64) -> String) -> String {
    value.map_or("—".to_string(), shown)
}

fn tenth(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn signed(value: f64) -> String {
    format!("{value:+.1}").replace('-', "−")
}

fn delta_text(erosion: Option<f64>, verbosity: Option<f64>) -> String {
    match (erosion, verbosity) {
        (None, None) => "—".to_string(),
        (e, v) => format!("{} / {}", figure(e, signed), figure(v, signed)),
    }
}

pub struct Group<'a> {
    pub pipeline: String,
    pub revision: usize,
    pub hash: String,
    pub series: Option<usize>,
    pub runs: Vec<&'a Numbers>,
}

impl Group<'_> {
    fn label(&self) -> String {
        format!("{} rev {}", self.pipeline, self.revision)
    }

    fn ended(&self) -> impl Iterator<Item = &&Numbers> {
        self.runs.iter().filter(|r| !r.live)
    }

    fn landed(&self) -> usize {
        self.runs.iter().filter(|r| r.landing.is_some()).count()
    }

    fn med(&self, pick: impl Fn(&Numbers) -> Option<f64>) -> Option<f64> {
        median(self.ended().filter_map(|r| pick(r)))
    }

    fn value(&self) -> Value {
        let ended = self.ended().count();
        json!({
            "pipeline": self.pipeline, "revision": self.revision, "hash": self.hash, "series": self.series, "runs": self.runs.len(), "landed": self.landed(),
            "landedShare": (ended > 0).then(|| self.landed() as f64 / ended as f64),
            "medians": {
                "timeToLand": self.med(|r| r.to_land), "timeToHandBack": self.med(|r| r.to_hand_back), "activeTime": self.med(|r| Some(r.active)), "sessions": self.med(|r| Some(r.sessions as f64)),
                "turns": self.med(|r| Some(r.turns as f64)), "tokens": self.med(|r| Some(r.tokens() as f64)), "cost": self.med(|r| r.cost), "reworkLoops": self.med(|r| Some(r.rework as f64)),
                "findings": self.med(|r| r.found().map(|f| f as f64)), "interventions": self.med(|r| Some(r.intervened() as f64)), "bugsTracedBack": self.med(|r| r.quality.bugs.map(|b| b as f64)),
                "changedLinesCovered": self.med(Numbers::covered), "erosionDelta": self.med(|r| r.quality.erosion), "verbosityDelta": self.med(|r| r.quality.verbosity),
            },
        })
    }
}

pub fn groups(runs: &[Numbers]) -> Vec<Group<'_>> {
    let mut found: Vec<Group> = vec![];
    let mut ordered: Vec<&Numbers> = runs.iter().collect();
    ordered.sort_by(|a, b| a.started.total_cmp(&b.started));
    for run in ordered {
        if let Some(group) = found.iter_mut().find(|g| g.pipeline == run.pipeline && g.hash == run.hash) {
            group.runs.push(run);
        } else {
            let series = (found.len() < SERIES).then_some(found.len());
            found.push(Group { pipeline: run.pipeline.clone(), revision: run.revision, hash: run.hash.clone(), series, runs: vec![run] });
        }
    }
    found
}

fn cell(text: impl Into<String>) -> Value {
    json!({ "text": text.into() })
}

fn barred(text: String, share: f64) -> Value {
    json!({ "text": text, "bar": share })
}

fn columns(names: &[&str], left: usize) -> Value {
    Value::Array(names.iter().enumerate().map(|(i, name)| json!({ "title": name, "right": i >= left })).collect())
}

fn summary_table(groups: &[Group]) -> Value {
    let top = |pick: &dyn Fn(&Group) -> Option<f64>| groups.iter().filter_map(pick).fold(0.0, f64::max);
    let land = |g: &Group| g.med(|r| r.to_land);
    let tokens = |g: &Group| g.med(|r| Some(r.tokens() as f64));
    let (most_land, most_tokens) = (top(&land), top(&tokens));
    let bar = |value: Option<f64>, top: f64, text: String| value.filter(|_| top > 0.0).map_or_else(|| cell(text.clone()), |v| barred(text.clone(), v / top));
    let rows = groups.iter().map(|g| {
        let (landed, ended) = (g.landed(), g.ended().count());
        let share = if ended > 0 { format!("{landed} ({:.0}%)", landed as f64 * 100.0 / ended as f64) } else { format!("{landed}") };
        let mean = |pick: &dyn Fn(&Numbers) -> Option<f64>| figure(g.med(pick), tenth);
        json!({ "series": g.series, "pipeline": g.pipeline, "cells": [
            json!({ "text": g.label(), "swatch": true }), cell(g.runs.len().to_string()), cell(share),
            bar(land(g), most_land, figure(land(g), hours)),
            cell(mean(&|r| Some(r.sessions as f64))),
            bar(tokens(g), most_tokens, figure(tokens(g), amount)),
            cell(mean(&|r| Some(r.rework as f64))), cell(mean(&|r| r.found().map(|f| f as f64))), cell(mean(&|r| Some(r.intervened() as f64))),
            cell(mean(&|r| r.quality.bugs.map(|b| b as f64))), cell(figure(g.med(Numbers::covered), |c| format!("{c:.0}%"))),
            cell(delta_text(g.med(|r| r.quality.erosion), g.med(|r| r.quality.verbosity))),
        ]})
    }).collect::<Vec<_>>();
    json!({ "kind": "table", "columns": columns(&["Pipeline", "Runs", "Landed", "Time to land,\nmedian", "Sessions", "Tokens", "Rework\nloops", "Review\nfindings", "Your\ninterventions", "Bugs traced\nback", "Changed lines\ncovered", "Quality\ndelta"], 1), "rows": rows, "what": "pipelines" })
}

fn seen_text(ended: f64, today: i64) -> String {
    let days = today - crate::stats::local_day(ended);
    match days {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        n => format!("{n} days ago"),
    }
}

fn outcome_text(run: &Numbers) -> (String, &'static str) {
    if let Some((stage, visit)) = &run.at {
        return (if *visit > 1 { format!("{stage}, visit {visit}") } else { stage.clone() }, "now");
    }
    match run.outcome.as_str() {
        "landed" => ("landed".to_string(), "pass"),
        "handed back" => ("handed back".to_string(), "now"),
        other => (run.why.clone().map_or_else(|| other.to_string(), |why| format!("{other}: {why}")), "fail"),
    }
}

fn runs_table(groups: &[Group], runs: &[&Numbers], today: i64) -> Value {
    let group_of = |run: &Numbers| groups.iter().find(|g| g.pipeline == run.pipeline && g.hash == run.hash);
    let rows = runs.iter().map(|run| {
        let group = group_of(run);
        let (outcome, tone) = outcome_text(run);
        json!({ "series": group.and_then(|g| g.series), "pipeline": run.pipeline, "dim": !run.live && run.landing.is_none() && run.outcome != "handed back", "cells": [
            json!({ "text": run.run, "mono": true }), cell(run.name.clone()), json!({ "text": group.map_or(String::new(), Group::label), "swatch": true }),
            cell(run.ended.map_or("running".to_string(), |at| seen_text(at, today))), cell(figure(run.to_land, hours)), cell(run.sessions.to_string()), cell(amount(run.tokens() as f64)), cell(run.rework.to_string()), cell(run.intervened().to_string()),
            json!({ "text": outcome, "tone": tone }),
        ]})
    }).collect::<Vec<_>>();
    json!({ "kind": "table", "columns": [
        json!({ "title": "Run" }), json!({ "title": "Task", "fill": true }), json!({ "title": "Pipeline" }), json!({ "title": "Ended" }), json!({ "title": "Time to land", "right": true }), json!({ "title": "Sessions", "right": true }),
        json!({ "title": "Tokens", "right": true }), json!({ "title": "Rework", "right": true }), json!({ "title": "Interventions", "right": true }), json!({ "title": "Outcome" }),
    ], "rows": rows, "what": "recent runs" })
}

struct StageTime {
    pipeline: String,
    label: String,
    series: Option<usize>,
    stage: String,
    machine: String,
    seconds: Vec<f64>,
}

fn stage_times(groups: &[Group], stages: &HashMap<String, Vec<StageRow>>) -> Vec<StageTime> {
    let mut times: Vec<StageTime> = vec![];
    for group in groups {
        for run in &group.runs {
            for visit in stages.get(&run.run).into_iter().flatten() {
                let Some(end) = visit.ended_at else { continue };
                let waited = visit.doc.get("waited").and_then(Value::as_f64).unwrap_or(0.0);
                let took = (end - visit.started_at - waited).max(0.0);
                let same = times.iter_mut().find(|t| t.label == group.label() && t.stage == visit.stage && t.machine == visit.machine);
                match same {
                    Some(time) => time.seconds.push(took),
                    None => times.push(StageTime { pipeline: group.pipeline.clone(), label: group.label(), series: group.series, stage: visit.stage.clone(), machine: visit.machine.clone(), seconds: vec![took] }),
                }
            }
        }
    }
    times
}

fn machines_table(times: &[StageTime]) -> Value {
    let machines: BTreeSet<&str> = times.iter().map(|t| t.machine.as_str()).collect();
    let mut places: Vec<(&str, &str)> = vec![];
    for time in times {
        if !places.contains(&(&time.label, &time.stage)) {
            places.push((&time.label, &time.stage));
        }
    }
    let rows = places.iter().map(|(label, stage)| {
        let here: Vec<&StageTime> = times.iter().filter(|t| t.label == *label && t.stage == *stage).collect();
        let mut cells = vec![json!({ "text": label, "swatch": true }), cell(*stage)];
        for machine in &machines {
            let time = here.iter().find(|t| t.machine == *machine);
            cells.push(cell(time.map_or("—".to_string(), |t| format!("{} · {}", figure(median(t.seconds.iter().copied()), hours), t.seconds.len()))));
        }
        json!({ "series": here[0].series, "pipeline": here[0].pipeline, "cells": cells })
    }).collect::<Vec<_>>();
    let mut titles = vec![json!({ "title": "Pipeline" }), json!({ "title": "Stage", "fill": true })];
    titles.extend(machines.iter().map(|m| json!({ "title": m, "right": true })));
    json!({ "kind": "table", "columns": titles, "rows": rows, "what": "a stage's median time and visits, by machine" })
}

fn nice(top: f64) -> f64 {
    if top <= 0.0 {
        return 1.0;
    }
    let power = 10f64.powf(top.log10().floor());
    [1.0, 2.0, 4.0, 5.0, 10.0].iter().map(|m| m * power).find(|v| *v >= top).unwrap_or(top)
}

fn dots(id: &str, label: &str, unit: &str, groups: &[Group], from: f64, to: f64, pick: impl Fn(&Numbers) -> Option<f64>, shown: impl Fn(f64) -> String) -> Value {
    let mut points = vec![];
    let mut lines = vec![];
    let mut top: f64 = 0.0;
    for group in groups {
        for run in group.ended() {
            if let Some(value) = pick(run) {
                top = top.max(value);
                points.push(json!({ "at": run.ended.unwrap_or(run.started), "value": value, "series": group.series, "pipeline": group.pipeline, "label": format!("{} · {} · {}", group.label(), run.run, shown(value)) }));
            }
        }
        if let Some(value) = group.med(&pick) {
            lines.push(json!({ "value": value, "series": group.series, "pipeline": group.pipeline, "label": format!("{} median {}", group.label(), shown(value)) }));
        }
    }
    let note = format!("{unit} · one dot per run, dashed: each pipeline's median");
    let max = nice(top);
    tile(id, label, Value::Null, None, note, 2, json!({ "kind": "runs", "points": points, "medians": lines, "max": max, "ticks": [shown(max / 2.0), shown(max)], "from": from, "to": to, "what": label }))
}

pub fn section_of(runs: &[Numbers], stages: &HashMap<String, Vec<StageRow>>, days: Option<i64>, from: f64, now: f64, today: i64) -> Value {
    let note = "by the pipeline and revision each run ran";
    if runs.is_empty() {
        let mut empty = section("pipelines", "Pipelines", note, vec![]);
        empty["empty"] = json!("No pipeline runs yet. `kanna run start PIPELINE` from a task runs one; each run shows here, compared by pipeline and revision.");
        empty["filters"] = json!({ "days": days, "pipelines": [] });
        return empty;
    }
    let found = groups(runs);
    let mut recent: Vec<&Numbers> = runs.iter().collect();
    recent.sort_by(|a, b| b.started.total_cmp(&a.started));
    recent.truncate(RECENT);
    let mut every: Vec<&Numbers> = runs.iter().collect();
    every.sort_by(|a, b| b.started.total_cmp(&a.started));
    let times = stage_times(&found, stages);
    let legend: Vec<Value> = found.iter().map(|g| json!({ "label": g.label(), "series": g.series, "pipeline": g.pipeline })).collect();
    let names: Vec<String> = found.iter().map(|g| g.pipeline.clone()).fold(vec![], |mut names, name| {
        if !names.contains(&name) {
            names.push(name);
        }
        names
    });
    let summary_note = format!("Per run unless said otherwise: medians. Tokens: in, cache writes and out; cache reads are in the JSON. Bugs traced back: fix commits on main blamed to the run's merge (SZZ). Changed lines covered: lines the run's merge changed that a test later ran. Quality delta: the points erosion / verbosity moved at the merge. {CAVEAT}");
    let mut body = section("pipelines", "Pipelines", note, vec![
        tile("pipelines", "", Value::Null, None, summary_note, 6, summary_table(&found)),
        dots("run-land", "Time to land", "hours", &found, from, now, |r| r.to_land.map(|s| s / HOUR), |v| hours(v * HOUR)),
        dots("run-tokens", "Tokens", "millions", &found, from, now, |r| Some(r.tokens() as f64 / 1e6), |v| amount(v * 1e6)),
        dots("run-rework", "Rework loops", "fix visits", &found, from, now, |r| Some(r.rework as f64), tenth),
        tile("recent", "Recent runs", Value::Null, None, String::new(), 6, runs_table(&found, &recent, today)),
        tile("machines", "Stage times by machine", Value::Null, None, "median · visits".to_string(), 6, machines_table(&times)),
    ]);
    body["filters"] = json!({ "days": days, "pipelines": names });
    body["legend"] = Value::Array(legend);
    body["allRuns"] = runs_table(&found, &every, today);
    body
}

pub fn report(project: &str, runs: &[Numbers], stages: &HashMap<String, Vec<StageRow>>, days: Option<i64>, from: f64, now: f64, today: i64) -> Value {
    let found = groups(runs);
    let times = stage_times(&found, stages);
    json!({
        "project": project,
        "days": days,
        "pipelines": found.iter().map(Group::value).collect::<Vec<_>>(),
        "runs": runs.iter().map(Numbers::value).collect::<Vec<_>>(),
        "stages": times.iter().map(|t| json!({ "pipeline": t.pipeline, "label": t.label, "stage": t.stage, "machine": t.machine, "visits": t.seconds.len(), "medianSeconds": median(t.seconds.iter().copied()) })).collect::<Vec<_>>(),
        "caveat": CAVEAT,
        "sections": [section_of(runs, stages, days, from, now, today)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, pipeline: &str, hash: &str, started: f64, outcome: &str, doc: Value) -> RunRow {
        RunRow { id: id.into(), task: "t".into(), repo: "/r".into(), pipeline: pipeline.into(), revision_hash: hash.into(), branch: "b".into(), state: "done".into(), started_at: started, ended_at: Some(started + 100.0), outcome: Some(outcome.into()), doc }
    }

    fn visit(stage: &str, start: f64, end: f64, kind: &str, verdict: &str, doc: Value) -> StageRow {
        StageRow { id: 1, run: "r".into(), stage: stage.into(), visit: 1, instance: None, kind: kind.into(), agent: None, machine: "mac".into(), started_at: start, ended_at: Some(end), verdict: Some(verdict.into()), session: None, terminal: Some(format!("term-{stage}")), worktree: None, branch: None, doc }
    }

    fn event(at: f64, what: &str, by: Option<&str>) -> RunEvent {
        RunEvent { at, what: what.into(), by: by.map(String::from), doc: json!({}) }
    }

    #[test]
    fn a_run_sums_its_visits_and_counts_rework_and_interventions() {
        let usage = |turns: u64, cost: Option<f64>| json!({ "turns": turns, "tools": 2, "in": 100, "out": 50, "cached": 1000, "written": 25, "cost": cost });
        let stages = [
            visit("build", 0.0, 10.0, "agent", "done", json!({ "usage": usage(3, Some(0.5)), "commits": 2, "files": 3, "added": 40, "deleted": 4 })),
            visit("review", 10.0, 30.0, "agent", "fail", json!({ "usage": usage(1, None), "findings": { "bug": 1, "nit": 2 }, "waited": 5.0 })),
            visit("check", 30.0, 35.0, "run", "fail", json!({ "failing": 2 })),
            visit("build", 35.0, 40.0, "agent", "done", json!({ "usage": usage(2, Some(0.25)), "commits": 1 })),
        ];
        let events = [event(1.0, "started", Some("term-1")), event(12.0, "paused", Some("term-review")), event(20.0, "resumed", Some("term-9")), event(21.0, "resumed", None), event(39.0, "stopped", Some("user")), event(39.5, "paused", Some("daemon"))];
        let n = numbers(&run("r", "p", "h", 0.0, "handed back", json!({})), &stages, &events, 1, 50.0);
        assert_eq!((n.turns, n.tools, n.input, n.output, n.cached, n.written, n.tokens()), (6, 6, 300, 150, 3000, 75, 525));
        assert_eq!(n.cost, Some(0.75));
        assert_eq!(n.active, 35.0, "the sum of the visits less what they waited");
        assert_eq!((n.rework, n.rework_by.get("build").copied()), (1, Some(1)));
        assert_eq!(n.findings, Some(BTreeMap::from([("bug".to_string(), 1), ("nit".to_string(), 2)])));
        assert_eq!((n.failing, n.failed_runs, n.sessions, n.commits, n.files, n.added), (2, 1, 3, 3, 3, 40));
        assert_eq!(n.interventions, BTreeMap::from([("resumed".to_string(), 1), ("stopped".to_string(), 1)]), "a stage's own terminal and the daemon don't intervene");
        assert_eq!(n.to_hand_back, Some(100.0));
        assert_eq!((n.to_land, n.outcome.as_str()), (None, "handed back"));
        assert_eq!(visit_interventions(&stages[1], &stages, &events), BTreeMap::from([("resumed".to_string(), 1)]));
    }

    #[test]
    fn the_run_s_own_count_of_interventions_wins_over_its_events() {
        let events = [event(1.0, "stopped", Some("user"))];
        let n = numbers(&run("r", "p", "h", 0.0, "stopped", json!({ "interventions": { "paused": 2, "skipped": 1 } })), &[], &events, 1, 10.0);
        assert_eq!(n.interventions, BTreeMap::from([("paused".to_string(), 2), ("skipped".to_string(), 1)]));
    }

    #[test]
    fn a_landed_run_is_timed_to_its_merge() {
        let n = numbers(&run("r", "p", "h", 100.0, "handed back", json!({ "landing": "abc", "landedAt": 700.0 })), &[], &[], 1, 800.0);
        assert_eq!((n.to_land, n.outcome.as_str(), n.to_hand_back), (Some(600.0), "landed", Some(100.0)));
    }

    #[test]
    fn revisions_count_a_pipeline_s_texts_in_the_order_they_first_ran() {
        let rows = [run("a", "feature", "h2", 2.0, "stopped", json!({})), run("b", "feature", "h1", 1.0, "stopped", json!({})), run("c", "feature", "h2", 3.0, "stopped", json!({})), run("d", "bugfix", "h9", 4.0, "stopped", json!({}))];
        let found = revisions(&rows);
        assert_eq!(found[&("feature".to_string(), "h1".to_string())], 1);
        assert_eq!(found[&("feature".to_string(), "h2".to_string())], 2);
        assert_eq!(found[&("bugfix".to_string(), "h9".to_string())], 1);
    }

    #[test]
    fn the_landing_is_main_s_first_commit_that_holds_the_tip() {
        let commit = |id: &str, at: f64| Commit { id: id.into(), at, parents: vec![], subject: String::new() };
        let main = [commit("d", 4.0), commit("c", 3.0), commit("b", 2.0), commit("a", 1.0)];
        assert_eq!(landing_of(&main, |c| c >= "c"), Some(("c".to_string(), 3.0)));
        assert_eq!(landing_of(&main, |_| false), None);
    }

    fn sample(commit: &str, metric: &str, at: f64, value: f64, doc: Value) -> Sample {
        Sample { commit: commit.into(), metric: metric.into(), at, value, doc, revision: 0 }
    }

    #[test]
    fn quality_joins_by_the_merge_commit() {
        let samples = [
            sample("m1", "fixes", 1.0, 0.0, json!({})),
            sample("m2", "fixes", 2.0, 0.0, json!({})),
            sample("m3", "fixes", 3.0, 1.0, json!({ "blamed": "m2" })),
            sample("m4", "fixes", 4.0, 1.0, json!({ "blamed": "m2" })),
            sample("m1", "erosion", 1.0, 40.0, json!({})),
            sample("m2", "erosion", 2.0, 42.5, json!({})),
            sample("m1", "verbosity", 1.0, 10.0, json!({})),
            sample("m2", "verbosity", 2.0, 9.5, json!({})),
            sample("m4", "coverage", 4.0, 70.0, json!({ "landings": [{ "commit": "m2", "hit": 9, "found": 12 }] })),
        ];
        let quality = join_quality(&samples);
        assert_eq!(quality("m2"), Quality { bugs: Some(2), covered: Some((9, 12)), erosion: Some(2.5), verbosity: Some(-0.5) });
        assert_eq!(quality("m1"), Quality { bugs: Some(0), covered: None, erosion: None, verbosity: None });
        assert_eq!(quality("unsampled"), Quality::default(), "not sampled yet is unknown, not zero");
    }

    fn numbered(id: &str, pipeline: &str, hash: &str, started: f64, to_land: Option<f64>, tokens: u64, rework: u64) -> Numbers {
        Numbers { run: id.into(), name: id.into(), pipeline: pipeline.into(), hash: hash.into(), revision: 1, started, ended: Some(started + 10.0), outcome: if to_land.is_some() { "landed" } else { "stopped" }.into(), to_land, input: tokens, rework, landing: to_land.map(|_| "m".to_string()), ..Numbers::default() }
    }

    #[test]
    fn runs_group_by_pipeline_and_revision_and_only_ended_runs_make_the_medians() {
        let live = Numbers { live: true, ended: None, ..numbered("e", "feature", "h1", 5.0, None, 9_000_000, 9) };
        let runs = [numbered("a", "feature", "h1", 1.0, Some(3600.0), 1000, 1), numbered("b", "feature", "h1", 2.0, Some(7200.0), 3000, 3), numbered("c", "feature", "h2", 3.0, None, 500, 0), numbered("d", "bugfix", "h3", 4.0, Some(600.0), 100, 0), live];
        let found = groups(&runs);
        assert_eq!(found.iter().map(|g| (g.label(), g.series, g.runs.len())).collect::<Vec<_>>(), [("feature rev 1".to_string(), Some(0), 3), ("feature rev 1".to_string(), Some(1), 1), ("bugfix rev 1".to_string(), Some(2), 1)]);
        let first = found[0].value();
        assert_eq!((first["runs"].clone(), first["landed"].clone(), first["landedShare"].clone()), (json!(3), json!(2), json!(1.0)));
        assert_eq!(first["medians"]["timeToLand"], json!(5400.0));
        assert_eq!(first["medians"]["tokens"], json!(2000.0));
        assert_eq!(found[1].value()["landedShare"], json!(0.0));
    }

    #[test]
    fn a_fourth_pipeline_has_no_hue_of_its_own() {
        let runs: Vec<Numbers> = ["a", "b", "c", "d"].iter().enumerate().map(|(i, p)| numbered(p, p, "h", i as f64, None, 1, 0)).collect();
        assert_eq!(groups(&runs).iter().map(|g| g.series).collect::<Vec<_>>(), [Some(0), Some(1), Some(2), None]);
    }

    #[test]
    fn the_section_has_the_mockup_s_table_charts_and_runs_with_the_caveat() {
        let runs = [numbered("run-1", "feature", "h1", 86400.0, Some(5400.0), 2_500_000, 1), numbered("run-2", "bugfix", "h2", 90000.0, None, 900, 0)];
        let stages = HashMap::from([("run-1".to_string(), vec![visit("build", 0.0, 60.0, "agent", "done", json!({}))])]);
        let body = section_of(&runs, &stages, Some(30), 0.0, 100000.0, 1);
        let tiles = body["tiles"].as_array().unwrap();
        assert_eq!(tiles.iter().map(|t| t["id"].as_str().unwrap()).collect::<Vec<_>>(), ["pipelines", "run-land", "run-tokens", "run-rework", "recent", "machines"]);
        assert!(tiles[0]["note"].as_str().unwrap().contains("F1 is about 0.6"));
        let row = &tiles[0]["chart"]["rows"][0];
        assert_eq!(row["cells"][0]["text"], json!("feature rev 1"));
        assert_eq!(row["cells"][2]["text"], json!("1 (100%)"));
        assert_eq!(row["cells"][3]["text"], json!("1.5h"));
        assert_eq!(row["cells"][3]["bar"], json!(1.0));
        assert_eq!(row["cells"][5]["text"], json!("2.5M"));
        assert_eq!(row["cells"][11]["text"], json!("—"));
        assert_eq!(tiles[1]["chart"]["points"].as_array().unwrap().len(), 1, "a run that didn't land has no time to land");
        assert_eq!(tiles[2]["chart"]["points"].as_array().unwrap().len(), 2);
        assert_eq!(tiles[1]["chart"]["max"], json!(2.0));
        assert_eq!(tiles[4]["chart"]["rows"][0]["cells"][0]["text"], json!("run-2"));
        assert_eq!(tiles[4]["chart"]["rows"][0]["dim"], json!(true));
        assert_eq!(tiles[5]["chart"]["columns"][2]["title"], json!("mac"));
        assert_eq!(tiles[5]["chart"]["rows"][0]["cells"][2]["text"], json!("1m · 1"));
        assert_eq!(body["filters"], json!({ "days": 30, "pipelines": ["feature", "bugfix"] }));
    }

    #[test]
    fn no_runs_says_how_to_get_one() {
        let body = section_of(&[], &HashMap::new(), Some(30), 0.0, 1.0, 1);
        assert!(body["empty"].as_str().unwrap().starts_with("No pipeline runs yet"));
    }

    #[test]
    fn amounts_read_like_the_mockup_s() {
        assert_eq!((hours(45.0), hours(600.0), hours(12240.0)), ("45s".to_string(), "10m".to_string(), "3.4h".to_string()));
        assert_eq!((amount(900.0), amount(71_000.0), amount(3_100_000.0)), ("900".to_string(), "71k".to_string(), "3.1M".to_string()));
        assert_eq!(signed(-0.5), "−0.5");
        assert_eq!(delta_text(Some(0.4), None), "+0.4 / —");
    }
}
