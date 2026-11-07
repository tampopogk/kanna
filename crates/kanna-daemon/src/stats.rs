use crate::agent_stats;
use crate::db::{self, AgentRow, Answer, Db, Life, Sample};
use crate::quality;
use crate::tasks::git;
use crate::Event;
use kanna_quality::git::Blobs;
use kanna_quality::tree::{Cache, Tree};
use kanna_mux::json::{json, string, Map, Value};
use kanna_mux::{project, CoreHandle, KmuxError, Responder};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const DAYS: i64 = 30;

const AREAS: usize = 5;

const MEDIAN_DAYS: i64 = 7;

const BATCH_SECONDS: f64 = 2.0;

pub struct Commit {
    pub id: String,
    pub at: f64,
    pub parents: Vec<String>,
    pub subject: String,
}

pub struct Context<'a> {
    pub repo: &'a Path,
    pub tasks: &'a HashMap<String, String>,
}

pub enum Sampler {
    Commit(fn(&Context, &Commit) -> Result<(f64, Value), String>),
    Tree(fn(&Tree) -> (f64, Value)),
}

pub struct Metric {
    pub name: &'static str,
    pub revision: i64,
    pub sample: Sampler,
}

pub type Revisions = HashMap<(String, String), i64>;

pub const METRICS: &[Metric] = &[
    Metric { name: "commits", revision: 0, sample: Sampler::Commit(brought) },
    Metric { name: "merged", revision: 0, sample: Sampler::Commit(merge) },
    Metric { name: "churn", revision: 0, sample: Sampler::Commit(quality::churn) },
    Metric { name: "fixes", revision: 0, sample: Sampler::Commit(quality::fixes) },
    Metric { name: "erosion", revision: 1, sample: Sampler::Tree(quality::erosion) },
    Metric { name: "verbosity", revision: 1, sample: Sampler::Tree(quality::verbosity) },
    Metric { name: "high", revision: 2, sample: Sampler::Tree(quality::high) },
    Metric { name: "dead", revision: 2, sample: Sampler::Tree(quality::dead) },
    Metric { name: "wrappers", revision: 2, sample: Sampler::Tree(quality::wrappers) },
    Metric { name: "lines", revision: 1, sample: Sampler::Tree(quality::lines) },
];

fn brought(cx: &Context, commit: &Commit) -> Result<(f64, Value), String> {
    match commit.parents.first() {
        Some(first) if commit.parents.len() > 1 => {
            let count = git(cx.repo, &["rev-list", "--count", "--no-merges", &format!("{first}..{}", commit.id)])?;
            Ok((count.trim().parse().map_err(|_| format!("git counted {count:?}"))?, json!({})))
        }
        _ => Ok((1.0, json!({}))),
    }
}

fn merge(_: &Context, commit: &Commit) -> Result<(f64, Value), String> {
    Ok((if commit.parents.len() > 1 { 1.0 } else { 0.0 }, json!({})))
}

pub fn main_ref(repo: &Path) -> String {
    ["refs/heads/main", "refs/heads/master"]
        .into_iter()
        .find(|name| git(repo, &["rev-parse", "--verify", "--quiet", name]).is_ok())
        .unwrap_or("HEAD")
        .to_string()
}

pub fn first_parents(repo: &Path, since: f64) -> Result<Vec<Commit>, String> {
    let log = git(repo, &["log", "--first-parent", &format!("--since=@{}", since as i64), "--format=%H%x09%ct%x09%P%x09%s", &main_ref(repo)])?;
    let mut commits: Vec<Commit> = log
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, '\t');
            let id = fields.next()?.to_string();
            let at: f64 = fields.next()?.parse().ok()?;
            let parents = fields.next()?.split_whitespace().map(String::from).collect();
            Some(Commit { id, at, parents, subject: fields.next().unwrap_or("").to_string() })
        })
        .collect();
    let mut previous = f64::NEG_INFINITY;
    for commit in commits.iter_mut().rev() {
        commit.at = commit.at.max(previous + 0.001);
        previous = commit.at;
    }
    commits.retain(|commit| commit.at >= since);
    Ok(commits)
}

pub struct Progress {
    pub sampled: usize,
    pub of: usize,
}

pub fn catch_up(repo: &Path, since: f64, sampled: &Revisions, tasks: &HashMap<String, String>, deliver: &mut dyn FnMut(Vec<Sample>, Option<Progress>)) -> Result<(), String> {
    let cx = Context { repo, tasks };
    let commits = first_parents(repo, since)?;
    let missing = |commit: &Commit, metric: &Metric| sampled.get(&(commit.id.clone(), metric.name.to_string())) != Some(&metric.revision);
    let mut samples = Vec::new();
    for commit in commits.iter().rev() {
        for metric in METRICS {
            let Sampler::Commit(sample) = &metric.sample else { continue };
            if missing(commit, metric) {
                let (value, doc) = sample(&cx, commit)?;
                samples.push(Sample { commit: commit.id.clone(), metric: metric.name.to_string(), at: commit.at, value, doc, revision: metric.revision });
            }
        }
    }
    let trees: Vec<&Commit> = commits.iter().filter(|c| METRICS.iter().any(|m| matches!(m.sample, Sampler::Tree(_)) && missing(c, m))).collect();
    let progress = |done: usize| (!trees.is_empty() && done < trees.len()).then_some(Progress { sampled: commits.len() - trees.len() + done, of: commits.len() });
    deliver(std::mem::take(&mut samples), progress(0));
    if trees.is_empty() {
        return Ok(());
    }
    let mut blobs = Blobs::open(repo)?;
    let mut cache = Cache::default();
    let mut batch = now();
    for (done, commit) in trees.iter().enumerate() {
        let tree = cache.tree(repo, &mut blobs, &commit.id)?;
        for metric in METRICS {
            let Sampler::Tree(sample) = &metric.sample else { continue };
            if missing(commit, metric) {
                let (value, doc) = sample(&tree);
                samples.push(Sample { commit: commit.id.clone(), metric: metric.name.to_string(), at: commit.at, value, doc, revision: metric.revision });
            }
        }
        if now() - batch > BATCH_SECONDS && done + 1 < trees.len() {
            deliver(std::mem::take(&mut samples), progress(done + 1));
            batch = now();
        }
    }
    deliver(samples, None);
    Ok(())
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146097 + of_cycle - 719468
}

fn civil(days: i64) -> (i64, i64, i64) {
    let days = days + 719468;
    let era = days.div_euclid(146097);
    let of_era = days - era * 146097;
    let of_cycle = (of_era - of_era / 1460 + of_era / 36524 - of_era / 146096) / 365;
    let of_year = of_era - (365 * of_cycle + of_cycle / 4 - of_cycle / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 { shifted + 3 } else { shifted - 9 };
    (of_cycle + era * 400 + if month <= 2 { 1 } else { 0 }, month, day)
}

pub fn local_day(at: f64) -> i64 {
    let seconds = at.floor() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    days_from_civil(tm.tm_year as i64 + 1900, tm.tm_mon as i64 + 1, tm.tm_mday as i64)
}

pub fn day_start(day: i64) -> f64 {
    let (year, month, date) = civil(day);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = (year - 1900) as libc::c_int;
    tm.tm_mon = (month - 1) as libc::c_int;
    tm.tm_mday = date as libc::c_int;
    tm.tm_isdst = -1;
    unsafe { libc::mktime(&mut tm) as f64 }
}

fn date(day: i64) -> String {
    let (year, month, date) = civil(day);
    format!("{year:04}-{month:02}-{date:02}")
}

pub struct Facts {
    pub today: i64,
    pub answers: Vec<Answer>,
    pub samples: Vec<Sample>,
    pub lives: Vec<Life>,
    pub agents: Vec<AgentRow>,
    pub now: f64,
    pub sampling: Option<String>,
    pub coverage: Option<String>,
}

pub fn tile(id: &str, label: &str, value: Value, unit: Option<&str>, note: String, span: u8, chart: Value) -> Value {
    json!({ "id": id, "label": label, "value": value, "unit": unit, "note": note, "span": span, "chart": chart })
}

pub fn section(id: &str, title: &str, note: &str, tiles: Vec<Value>) -> Value {
    json!({ "id": id, "title": title, "note": note, "tiles": tiles })
}

fn percent(right: usize, all: usize) -> Value {
    if all == 0 {
        Value::Null
    } else {
        json!((right as f64 * 1000.0 / all as f64).round() / 10.0)
    }
}

pub fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 1 { values[middle] } else { (values[middle - 1] + values[middle]) / 2.0 })
}

pub fn area(file: &str) -> String {
    let parts: Vec<&str> = file.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    match parts.len() {
        0 => "Other".to_string(),
        1 => parts[0].to_string(),
        2 => parts[0].to_string(),
        _ => format!("{}/{}", parts[0], parts[1]),
    }
}

fn ranked(groups: BTreeMap<String, (usize, usize)>, limit: Option<usize>) -> Value {
    let mut rows: Vec<(String, usize, usize)> = groups.into_iter().map(|(name, (right, all))| (name, right, all)).collect();
    let mut other = None;
    if let Some(limit) = limit.filter(|limit| rows.len() > *limit) {
        rows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        let rest = rows.split_off(limit);
        other = Some(("Other".to_string(), rest.iter().map(|r| r.1).sum(), rest.iter().map(|r| r.2).sum()));
    }
    rows.sort_by(|a, b| (b.1 * a.2).cmp(&(a.1 * b.2)).then_with(|| b.2.cmp(&a.2)).then_with(|| a.0.cmp(&b.0)));
    rows.extend(other);
    json!({ "kind": "ranked", "max": 100, "rows": rows.iter().map(|(name, right, all)| json!({ "label": name, "value": percent(*right, *all), "note": format!("{right}/{all}") })).collect::<Vec<_>>() })
}

fn quiz(facts: &Facts) -> Value {
    let note = "what you know of the code, from your kept answers";
    let first = facts.today - DAYS + 1;
    let answers: Vec<(&Answer, i64)> = facts.answers.iter().map(|a| (a, local_day(a.at))).filter(|(_, day)| *day <= facts.today).collect();
    if answers.is_empty() {
        let mut empty = section("quiz", "Quiz", note, vec![]);
        empty["empty"] = json!("No quiz answers yet. The planning agent offers a quiz once something lands on main; your answers show here.");
        return empty;
    }
    let earliest = answers.iter().map(|(_, day)| *day).min().unwrap_or(facts.today);
    let asked_again: HashMap<i64, i64> = answers.iter().filter_map(|(a, day)| a.again.map(|again| (again, *day))).fold(HashMap::new(), |mut seen, (again, day)| {
        let at = seen.entry(again).or_insert(day);
        *at = (*at).min(day);
        seen
    });
    let mut right = Vec::new();
    let mut misses = Vec::new();
    for day in first..=facts.today {
        let window: Vec<&Answer> = answers.iter().filter(|(_, d)| *d > day - DAYS && *d <= day).map(|(a, _)| *a).collect();
        right.push(percent(window.iter().filter(|a| a.right).count(), window.len()));
        misses.push(if day < earliest {
            Value::Null
        } else {
            json!(answers.iter().filter(|(a, d)| !a.right && *d <= day && asked_again.get(&a.id).is_none_or(|again| *again > day)).count())
        });
    }
    let recent: Vec<&Answer> = answers.iter().filter(|(_, d)| *d >= first).map(|(a, _)| *a).collect();
    let share = right.last().cloned().unwrap_or(Value::Null);
    let shown = share.as_f64().map_or(Value::Null, |s| json!(s.round()));
    let count = recent.len();
    let mut kinds: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut areas: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for answer in &recent {
        for (groups, key) in [(&mut kinds, answer.kind.clone()), (&mut areas, area(&answer.file))] {
            let entry = groups.entry(key).or_default();
            entry.0 += answer.right as usize;
            entry.1 += 1;
        }
    }
    let outstanding = misses.last().cloned().unwrap_or(json!(0));
    section(
        "quiz",
        "Quiz",
        note,
        vec![
            tile("right", "Answers right, last 30 days", shown, Some("%"), if count == 0 { "no answers in 30 days".to_string() } else { format!("of {count} answer{}", if count == 1 { "" } else { "s" }) }, 3, json!({ "kind": "line", "values": right, "max": 100, "what": "of the answers in the 30 days before" })),
            tile("misses", "Misses to come back", outstanding, None, String::new(), 3, json!({ "kind": "line", "values": misses, "what": "misses to come back" })),
            tile("kinds", "By kind", Value::Null, None, String::new(), 3, ranked(kinds, None)),
            tile("areas", "By area", Value::Null, None, String::new(), 3, ranked(areas, Some(AREAS))),
        ],
    )
}

fn per_day(first: i64, items: impl Iterator<Item = (f64, f64)>) -> Vec<f64> {
    let mut days = vec![0.0; DAYS as usize];
    for (at, value) in items {
        let index = local_day(at) - first;
        if (0..DAYS).contains(&index) {
            days[index as usize] += value;
        }
    }
    days
}

fn count_tile(id: &str, label: &str, days: Vec<f64>, what: &str) -> Value {
    let today = days.last().copied().unwrap_or(0.0);
    let total: f64 = days.iter().sum();
    tile(id, label, json!(today), None, format!("today · {total} in 30 days"), 2, json!({ "kind": "columns", "values": days, "what": what }))
}

fn main_section(facts: &Facts) -> Value {
    let first = facts.today - DAYS + 1;
    let of = |metric: &str| per_day(first, facts.samples.iter().filter(|s| s.metric == metric).map(|s| (s.at, s.value)));
    section("main", "Main", "what landed", vec![count_tile("commits", "Commits", of("commits"), "commits"), count_tile("landed", "Tasks landed", of("merged"), "merges into main")])
}

fn open_at(lives: &[Life], at: f64) -> f64 {
    lives.iter().filter(|life| life.made < at && life.done.is_none_or(|done| done >= at)).count() as f64
}

fn open_tile(facts: &Facts, made: &[f64], done: &[f64]) -> Value {
    let first = facts.today - DAYS + 1;
    let open: Vec<f64> = (first..=facts.today).map(|day| open_at(&facts.lives, day_start(day + 1))).collect();
    let today = open.last().copied().unwrap_or(0.0);
    let change = today - open_at(&facts.lives, day_start(first));
    let note = if change > 0.0 {
        format!("+{change} in 30 days")
    } else if change < 0.0 {
        format!("\u{2212}{} in 30 days", -change)
    } else {
        "no change in 30 days".to_string()
    };
    tile("open", "Open", json!(today), None, note, 6, json!({ "kind": "flow", "values": open, "made": made, "done": done, "what": "open at the end of the day" }))
}

pub fn median_line(points: &[(i64, f64, &str)]) -> Vec<Value> {
    (0..DAYS)
        .map(|day| {
            let mut near: Vec<f64> = points.iter().filter(|(d, _, _)| *d > day - MEDIAN_DAYS && *d <= day).map(|(_, s, _)| *s).collect();
            median(&mut near).map_or(Value::Null, |m| json!(m.round()))
        })
        .collect()
}

pub fn dots(points: &[(i64, f64, &str)], line: Vec<Value>, what: &str) -> Value {
    json!({ "kind": "dots", "points": points.iter().map(|(day, seconds, name)| json!({ "day": day, "value": seconds.round(), "label": name })).collect::<Vec<_>>(), "line": line, "what": what })
}

fn tasks_section(facts: &Facts) -> Value {
    let first = facts.today - DAYS + 1;
    let at = |pick: fn(&Life) -> Option<f64>| per_day(first, facts.lives.iter().filter_map(pick).map(|at| (at, 1.0)));
    let points: Vec<(i64, f64, &str)> = facts
        .lives
        .iter()
        .filter_map(|life| {
            let done = life.done?;
            let day = local_day(done) - first;
            (0..DAYS).contains(&day).then(|| (day, (done - life.made).max(0.0), life.name.as_str()))
        })
        .collect();
    let made = at(|l| Some(l.made));
    let ended = at(|l| l.done);
    let mut all: Vec<f64> = points.iter().map(|(_, s, _)| *s).collect();
    let middle = median(&mut all);
    let lead = tile(
        "lead",
        "Made to done",
        middle.map_or(Value::Null, |m| json!(m.round())),
        Some("s"),
        if points.is_empty() { String::new() } else { format!("median of {}", points.len()) },
        4,
        dots(&points, median_line(&points), "7-day median"),
    );
    section(
        "tasks",
        "Tasks",
        "from the Desk's and the Shelf's events",
        vec![open_tile(facts, &made, &ended), count_tile("made", "Made", made, "tasks made"), count_tile("started", "Started", at(|l| l.started), "tasks started"), count_tile("ended", "Ended", ended, "tasks ended"), lead],
    )
}

pub fn report(project: &str, facts: &Facts) -> Map {
    let days: Vec<String> = (facts.today - DAYS + 1..=facts.today).map(date).collect();
    Map::from_iter([
        ("project".to_string(), json!(project)),
        ("days".to_string(), json!(days)),
        ("sections".to_string(), json!([quiz(facts), main_section(facts), tasks_section(facts), agent_stats::section_of(facts), quality::code_section(facts)])),
    ])
}

pub struct Stats {
    db: Rc<Db>,
    events: crate::runtime::Events,
    waiting: RefCell<HashMap<String, Vec<Responder>>>,
    running: RefCell<HashMap<String, Option<String>>>,
    coverage: Rc<crate::coverage::Coverage>,
}

fn failed(what: &str, error: rusqlite::Error) -> KmuxError {
    log!("kanna-daemon: couldn't {what} in {}: {error}", db::FILE);
    KmuxError::new("failed", format!("couldn't {what}: {error}"))
}

pub struct Sampled {
    pub repo: String,
    pub samples: Result<Vec<Sample>, String>,
    pub progress: Option<String>,
    pub done: bool,
}

impl Stats {
    pub fn new(db: Rc<Db>, events: crate::runtime::Events, state: &Path) -> Stats {
        let coverage = Rc::new(crate::coverage::Coverage::new(db.clone(), events.clone(), state.to_path_buf()));
        Stats { db, events, waiting: RefCell::default(), running: RefCell::default(), coverage }
    }

    pub fn coverage(&self) -> &Rc<crate::coverage::Coverage> {
        &self.coverage
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let this = Rc::downgrade(self);
        core.add_command(
            "stats",
            Rc::new(move |args, responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                this.ask(&args, responder)
            }),
        );
    }

    fn ask(&self, args: &Value, responder: Responder) {
        let Some(repo) = string(args, "repo").filter(|r| Path::new(r).is_absolute()).and_then(|r| project(Some(r))) else {
            return responder.reply(Err(KmuxError::new("bad_request", "say which repo: an absolute path to a folder in a git repo")));
        };
        let repo = repo.to_string_lossy().to_string();
        self.coverage.consider(&repo);
        if let Some(waiting) = self.waiting.borrow_mut().get_mut(&repo) {
            return waiting.push(responder);
        }
        if self.running.borrow().contains_key(&repo) {
            return responder.reply(self.read(&repo));
        }
        let since = day_start(local_day(now()) - DAYS + 1);
        let sampled = match self.db.sampled(&repo, since) {
            Ok(sampled) => sampled,
            Err(error) => return responder.reply(Err(failed("read the samples", error))),
        };
        let tasks = self.db.branches(&repo).unwrap_or_else(|error| {
            log!("kanna-daemon: couldn't read {repo}'s tasks' branches in {}: {error}", db::FILE);
            HashMap::new()
        });
        self.waiting.borrow_mut().insert(repo.clone(), vec![responder]);
        self.running.borrow_mut().insert(repo.clone(), None);
        let events = self.events.clone();
        std::thread::spawn(move || {
            let mut deliver = |samples: Vec<Sample>, progress: Option<Progress>| {
                let progress = progress.map(|p| format!("sampling {} of {} commits", p.sampled, p.of));
                let _ = events.send(Event::Sampled(Sampled { repo: repo.clone(), samples: Ok(samples), progress, done: false }));
            };
            let result = catch_up(&PathBuf::from(&repo), since, &sampled, &tasks, &mut deliver);
            let _ = events.send(Event::Sampled(Sampled { repo, samples: result.map(|_| vec![]), progress: None, done: true }));
        });
    }

    pub fn sampled(&self, sampled: Sampled) {
        let Sampled { repo, samples, progress, done } = sampled;
        match samples {
            Ok(samples) if !samples.is_empty() => {
                if let Err(error) = self.db.put_samples(&repo, &samples) {
                    log!("kanna-daemon: couldn't save {} samples of {repo} in {}: {error}", samples.len(), db::FILE);
                }
            }
            Ok(_) => {}
            Err(error) => log!("kanna-daemon: couldn't sample {repo}'s main: {error}"),
        }
        if done {
            self.running.borrow_mut().remove(&repo);
        } else if let Some(running) = self.running.borrow_mut().get_mut(&repo) {
            *running = progress;
        }
        let responders = self.waiting.borrow_mut().remove(&repo).unwrap_or_default();
        if responders.is_empty() {
            return;
        }
        let reply = self.read(&repo);
        for responder in responders {
            responder.reply(reply.clone());
        }
    }

    fn read(&self, repo: &str) -> Result<Map, KmuxError> {
        let today = local_day(now());
        let since = day_start(today - DAYS + 1);
        let facts = Facts {
            today,
            answers: self.db.answers(repo, false).map_err(|e| failed("read the quiz answers", e))?,
            samples: self.db.samples(repo, since).map_err(|e| failed("read the samples", e))?,
            lives: self.db.lives(repo, since).map_err(|e| failed("read the task events", e))?,
            agents: self.db.agent_rows(repo, since).map_err(|e| failed("read the agents' status changes", e))?,
            now: now(),
            sampling: self.running.borrow().get(repo).cloned().flatten(),
            coverage: self.coverage.status(repo),
        };
        Ok(report(repo, &facts))
    }
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    pub fn noon(day: i64) -> f64 {
        day_start(day) + 12.0 * 3600.0
    }

    fn answer(id: i64, day: i64, kind: &str, file: &str, right: bool, again: Option<i64>) -> Answer {
        Answer { id, round: 1, at: noon(day), kind: kind.into(), file: file.into(), right, again, doc: json!({}) }
    }

    fn life(task: &str, made: f64, started: Option<f64>, done: Option<f64>) -> Life {
        Life { task: task.into(), name: format!("{task} name"), made, started, done }
    }

    pub fn facts(today: i64, answers: Vec<Answer>, samples: Vec<Sample>, lives: Vec<Life>) -> Facts {
        Facts { today, answers, samples, lives, agents: vec![], now: day_start(today + 1), sampling: None, coverage: None }
    }

    pub fn collect(repo: &Path, since: f64, sampled: &Revisions, tasks: &HashMap<String, String>) -> Vec<Sample> {
        let mut all = Vec::new();
        catch_up(repo, since, sampled, tasks, &mut |samples, _| all.extend(samples)).unwrap();
        all
    }

    fn tiles(report: &Map, section: &str) -> Vec<Value> {
        report["sections"].as_array().unwrap().iter().find(|s| s["id"] == section).unwrap()["tiles"].as_array().unwrap().clone()
    }

    fn tile_of(report: &Map, section: &str, id: &str) -> Value {
        tiles(report, section).into_iter().find(|t| t["id"] == id).unwrap()
    }

    fn row(task: &str, at: f64, what: &str, status: Option<&str>) -> AgentRow {
        AgentRow { task: task.into(), name: format!("{task} name"), at, what: what.into(), status: status.map(String::from) }
    }

    fn said(task: &str, at: f64, status: &str) -> AgentRow {
        row(task, at, "status", Some(status))
    }

    fn agents(today: i64, rows: Vec<AgentRow>) -> Map {
        let mut facts = facts(today, vec![], vec![], vec![]);
        facts.agents = rows;
        facts.now = noon(today) + 24.0 * 60.0;
        report("/r", &facts)
    }

    #[test]
    fn the_agents_section_comes_from_a_fixed_series_of_status_changes() {
        let today = local_day(now());
        let (a, b) = (noon(today - 1), noon(today));
        const MIN: f64 = 60.0;
        let report = agents(
            today,
            vec![
                said("t1", a, "working"),
                said("t1", a + 10.0 * MIN, "needs-you"),
                said("t1", a + 20.0 * MIN, "working"),
                said("t1", a + 50.0 * MIN, "idle"),
                row("t1", a + 80.0 * MIN, "looked", None),
                said("t2", b, "working"),
                said("t2", b + 6.0 * MIN, "idle"),
                row("t2", b + 6.0 * MIN + 1.0, "looked", None),
                row("t2", b + 7.0 * MIN, "stopped", None),
                row("t1", b, "planner", None),
                row("t2", b + 7.0 * MIN, "done", None),
            ],
        );
        let tile = |id: &str| tile_of(&report, "agents", id);
        assert_eq!(tile("agent-time")["value"], json!(0.77), "10m + 30m + 6m");
        assert_eq!(tile("agent-time")["unit"], json!("h"));
        assert_eq!(tile("agent-time")["note"], json!("in 30 days · 6m today"));
        assert_eq!(tile("agent-time")["chart"]["values"].as_array().unwrap()[29], json!(0.1));
        assert_eq!(tile("waiting")["value"], json!(0.17), "10m");
        assert_eq!(tile("answer")["value"], json!(600.0));
        assert_eq!(tile("answer")["note"], json!("median of 1"));
        assert_eq!(tile("unread")["value"], json!(1800.0), "the one looked at a second after it finished was being watched");
        assert_eq!(tile("turns")["value"], json!(1.5));
        assert_eq!(tile("turns")["unit"], json!("x"));
        assert_eq!(tile("turns")["note"], json!("3 turns · 2 tasks"));
        assert_eq!(tile("turn-length")["value"], json!(600.0), "6m, 10m, 30m");
        assert_eq!(tile("turn-length")["note"], json!("median of 3"));
    }

    #[test]
    fn a_restart_in_the_middle_of_a_turn_leaves_no_gap() {
        let today = local_day(now());
        let at = noon(today);
        let report = agents(today, vec![said("t1", at, "working"), said("t1", at + 600.0, "working"), said("t1", at + 1200.0, "idle")]);
        assert_eq!(tile_of(&report, "agents", "agent-time")["value"], json!(0.33), "the repeated working is one span");
        assert_eq!(tile_of(&report, "agents", "turns")["note"], json!("1 turn · 1 task"));
    }

    #[test]
    fn a_stopped_task_at_needs_you_waits_until_it_is_picked_or_started() {
        let today = local_day(now());
        let at = noon(today - 2);
        const HOUR: f64 = 3600.0;
        let picked = agents(today, vec![said("t1", at, "working"), said("t1", at + HOUR, "needs-you"), said("t1", at + HOUR + 1.0, "exited"), row("t1", at + HOUR + 1.0, "stopped", None), row("t1", at + 4.0 * HOUR, "picked", None)]);
        assert_eq!(tile_of(&picked, "agents", "waiting")["value"], json!(3.0));
        assert_eq!(tile_of(&picked, "agents", "answer")["value"], json!(10800.0));
        let started = agents(today, vec![said("t1", at, "needs-you"), row("t1", at + 1.0, "stopped", None), row("t1", at + HOUR, "started", None)]);
        assert_eq!(tile_of(&started, "agents", "answer")["note"], json!("median of 1"));
        let waiting = agents(today, vec![said("t1", at, "needs-you"), said("t1", at + 1.0, "exited")]);
        assert_eq!(tile_of(&waiting, "agents", "waiting")["value"], json!((facts_end(today) - at).div_euclid(36.0) / 100.0));
        assert_eq!(tile_of(&waiting, "agents", "answer")["value"], Value::Null);
        assert_eq!(tile_of(&waiting, "agents", "answer")["note"], json!("1 waiting now"));
        let running = agents(today, vec![said("t1", at, "needs-you"), row("t1", at + HOUR, "picked", None)]);
        assert_eq!(tile_of(&running, "agents", "answer")["note"], json!("1 waiting now"), "picking a task that's still running isn't an answer");
        let done = agents(today, vec![said("t1", at, "needs-you"), said("t1", at + 1.0, "exited"), row("t1", at + HOUR, "done", None)]);
        assert_eq!(tile_of(&done, "agents", "waiting")["value"], json!(1.0));
        assert_eq!(tile_of(&done, "agents", "answer")["note"], json!(""), "done without an answer: it counts as time, never as an answer");
    }

    fn facts_end(today: i64) -> f64 {
        noon(today) + 24.0 * 60.0
    }

    #[test]
    fn finished_work_a_stopped_agent_left_stays_unread_until_someone_looks() {
        let today = local_day(now());
        let at = noon(today);
        let rows = vec![said("t1", at, "working"), said("t1", at + 60.0, "idle"), said("t1", at + 90.0, "exited"), row("t1", at + 90.0, "stopped", None)];
        let open = agents(today, rows.clone());
        assert_eq!(tile_of(&open, "agents", "unread")["note"], json!("1 not looked at yet"));
        let mut picked = rows;
        picked.push(row("t1", at + 660.0, "picked", None));
        assert_eq!(tile_of(&agents(today, picked), "agents", "unread")["value"], json!(600.0));
    }

    #[test]
    fn agent_time_splits_at_midnight_and_a_project_with_no_status_says_so() {
        let today = local_day(now());
        let midnight = day_start(today);
        let report = agents(today, vec![said("t1", midnight - 1800.0, "working"), said("t1", midnight + 3600.0, "idle")]);
        let values = tile_of(&report, "agents", "agent-time")["chart"]["values"].as_array().unwrap().clone();
        assert_eq!((values[28].clone(), values[29].clone()), (json!(0.5), json!(1.0)));
        let empty = agents(today, vec![row("t1", midnight, "started", None)]);
        let section = empty["sections"].as_array().unwrap().iter().find(|s| s["id"] == "agents").unwrap().clone();
        assert_eq!(section["tiles"], json!([]));
        assert!(section["empty"].as_str().unwrap().starts_with("No agent status"));
    }

    #[test]
    fn days_round_trip_through_the_calendar() {
        for day in [-1, 0, 59, 365, 19000, 20393, 20758] {
            let (y, m, d) = civil(day);
            assert_eq!(days_from_civil(y, m, d), day);
        }
        assert_eq!(date(20393), "2025-11-01");
        let today = local_day(now());
        assert_eq!(local_day(day_start(today)), today);
        assert_eq!(local_day(day_start(today) - 1.0), today - 1);
    }

    #[test]
    fn the_grade_is_the_share_right_in_the_30_days_before_each_day() {
        let today = 20393;
        let facts = facts(today, vec![answer(1, today - 40, "why", "docs/specs/quiz.md", false, None), answer(2, today - 5, "why", "docs/specs/quiz.md", true, None), answer(3, today - 5, "bug", "crates/kanna-daemon/src/db.rs", false, None), answer(4, today, "bug", "crates/kanna-daemon/src/db.rs", true, None), answer(5, today, "where", "README.md", true, None)], vec![], vec![]);
        let report = report("/r", &facts);
        let right = tile_of(&report, "quiz", "right");
        assert_eq!(right["value"], json!(75.0));
        assert_eq!(right["note"], json!("of 4 answers"));
        let values = right["chart"]["values"].as_array().unwrap();
        assert_eq!(values.len(), 30);
        assert_eq!(values[0], json!(0.0), "the miss 40 days ago is still in day 0's window");
        assert_eq!(values[20], Value::Null, "no answers in the 30 days before");
        assert_eq!(values[24], json!(50.0));
        assert_eq!(values[29], json!(75.0));
    }

    #[test]
    fn a_miss_counts_until_it_is_asked_again() {
        let today = 20393;
        let facts = facts(today, vec![answer(1, today - 10, "why", "a/b/c.rs", false, None), answer(2, today - 8, "why", "a/b/c.rs", false, None), answer(3, today - 3, "why", "a/b/c.rs", true, Some(1)), answer(4, today - 1, "why", "a/b/c.rs", false, Some(2))], vec![], vec![]);
        let report = report("/r", &facts);
        let misses = tile_of(&report, "quiz", "misses");
        let values = misses["chart"]["values"].as_array().unwrap();
        assert_eq!(values[18], Value::Null);
        assert_eq!(values[19], json!(1));
        assert_eq!(values[21], json!(2));
        assert_eq!(values[26], json!(1));
        assert_eq!(values[28], json!(1), "a miss asked again and missed stands for the old one");
        assert_eq!(misses["value"], json!(1));
    }

    #[test]
    fn areas_are_two_folders_and_the_rest_is_other_last() {
        assert_eq!(area("crates/kanna-daemon/src/db.rs"), "crates/kanna-daemon");
        assert_eq!(area("docs/specs/quiz.md"), "docs/specs");
        assert_eq!(area("scripts/e2e-kanna.sh"), "scripts");
        assert_eq!(area("README.md"), "README.md");
        let today = 20393;
        let files = ["a/x/1", "a/x/2", "b/x/1", "c/x/1", "d/x/1", "e/x/1", "f/x/1", "g/x/1"];
        let answers = files.iter().enumerate().map(|(i, f)| answer(i as i64 + 1, today, "why", f, i % 2 == 0, None)).collect();
        let report = report("/r", &facts(today, answers, vec![], vec![]));
        let rows = tile_of(&report, "quiz", "areas")["chart"]["rows"].as_array().unwrap().clone();
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0]["label"], json!("b/x"));
        assert_eq!(rows[5]["label"], json!("Other"));
        assert_eq!(rows[5]["note"], json!("1/2"));
        let kinds = tile_of(&report, "quiz", "kinds")["chart"]["rows"].as_array().unwrap().clone();
        assert_eq!(kinds, vec![json!({ "label": "why", "value": 50.0, "note": "4/8" })]);
    }

    #[test]
    fn no_answers_says_how_to_get_some_and_no_code_says_so() {
        let report = report("/r", &facts(20393, vec![], vec![], vec![]));
        let sections = report["sections"].as_array().unwrap();
        assert_eq!(sections.iter().map(|s| s["id"].as_str().unwrap()).collect::<Vec<_>>(), ["quiz", "main", "tasks", "agents", "code"]);
        assert!(sections[0]["empty"].as_str().unwrap().starts_with("No quiz answers yet"));
        assert!(sections[3]["empty"].as_str().unwrap().starts_with("No agent status"));
        assert_eq!(sections[4]["placeholder"], Value::Null);
        assert_eq!(sections[4]["tiles"], json!([]));
        assert!(sections[4]["empty"].as_str().unwrap().starts_with("No Rust or Swift code"));
        assert_eq!(report["days"].as_array().unwrap().len(), 30);
        assert_eq!(report["days"][29], json!("2025-11-01"));
    }

    #[test]
    fn tasks_count_first_starts_and_time_made_to_done() {
        let today = 20393;
        let hour = 3600.0;
        let facts = facts(today, vec![], vec![], vec![
                life("old", noon(today - 50), Some(noon(today - 50)), Some(noon(today - 2))),
                life("a", noon(today - 1), Some(noon(today - 1) + hour), Some(noon(today) + 2.0 * hour)),
                life("b", noon(today), None, Some(noon(today) + 4.0 * hour)),
                life("c", noon(today), Some(noon(today)), None),
            ]);
        let report = report("/r", &facts);
        assert_eq!(tile_of(&report, "tasks", "made")["value"], json!(2.0));
        assert_eq!(tile_of(&report, "tasks", "made")["note"], json!("today · 3 in 30 days"));
        assert_eq!(tile_of(&report, "tasks", "started")["note"], json!("today · 2 in 30 days"), "the old task first started 50 days ago");
        assert_eq!(tile_of(&report, "tasks", "ended")["note"], json!("today · 3 in 30 days"));
        let lead = tile_of(&report, "tasks", "lead");
        assert_eq!(lead["value"], json!((26.0 * hour).round()));
        assert_eq!(lead["unit"], json!("s"));
        assert_eq!(lead["note"], json!("median of 3"));
        assert_eq!(lead["chart"]["points"].as_array().unwrap().len(), 3);
        assert_eq!(lead["chart"]["points"][1]["label"], json!("a name"));
        assert_eq!(lead["chart"]["line"][27], json!((48.0 * 24.0 * hour).round()));
    }

    #[test]
    fn open_tasks_at_the_end_of_each_day_come_from_made_and_done() {
        let state = std::env::temp_dir().join(format!("kanna-stats-open-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        let db = Db::open(&state);
        let today = local_day(now());
        let at = |day: i64, hours: f64| noon(today - day) + hours * 3600.0;
        let mut sql = String::new();
        for (id, repo, place, made) in [("old", "/r", "done", 50), ("stuck", "/r", "shelf", 45), ("same", "/r", "done", 5), ("new", "/r", "desk", 1), ("newer", "/r", "shelf", 0), ("legacy", "/r", "done", 60), ("elsewhere", "/o", "shelf", 3)] {
            sql += &format!("insert into tasks (id, repo, place, made_at, doc) values ('{id}', '{repo}', '{place}', {}, '{{\"name\": \"{id}\"}}');\n", at(made, 0.0));
            sql += &format!("insert into task_events (task, at, what) values ('{id}', {}, 'made');\n", at(made, 0.0));
        }
        for (id, day, hours, what) in [("old", 40, 0.0, "picked"), ("old", 2, 0.0, "done"), ("stuck", 10, 0.0, "picked"), ("stuck", 9, 0.0, "shelved"), ("same", 5, 1.0, "picked"), ("same", 5, 2.0, "done"), ("new", 0, 0.0, "picked"), ("legacy", 55, 0.0, "picked")] {
            sql += &format!("insert into task_events (task, at, what) values ('{id}', {}, '{what}');\n", at(day, hours));
        }
        db.run(&sql);
        let since = day_start(today - DAYS + 1);
        let report = report("/r", &facts(today, vec![], vec![], db.lives("/r", since).unwrap()));
        let open = tile_of(&report, "tasks", "open");
        let values: Vec<f64> = open["chart"]["values"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let mut expected = vec![2.0; 30];
        expected[27] = 1.0;
        expected[28] = 2.0;
        expected[29] = 3.0;
        assert_eq!(values, expected, "old and stuck open from before the 30 days, old done two days ago, new and newer made since; same made and done in one day; legacy done with no done event");
        assert_eq!(open["value"], json!(3.0));
        assert_eq!(open["note"], json!("+1 in 30 days"));
        assert_eq!(open["span"], json!(6));
        assert_eq!(open["chart"]["kind"], json!("flow"));
        assert_eq!(open["chart"]["made"][24], json!(1.0));
        assert_eq!(open["chart"]["done"][24], json!(1.0));
        assert_eq!(open["chart"]["made"], tile_of(&report, "tasks", "made")["chart"]["values"]);
        assert_eq!(open["chart"]["done"], tile_of(&report, "tasks", "ended")["chart"]["values"]);
        assert_eq!(tiles(&report, "tasks").iter().map(|t| t["id"].as_str().unwrap().to_string()).collect::<Vec<_>>(), ["open", "made", "started", "ended", "lead"]);
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn open_tasks_say_their_change_over_the_30_days() {
        let today = 20393;
        let lives = vec![life("a", noon(today - 40), None, Some(noon(today - 3))), life("b", noon(today - 40), None, Some(noon(today))), life("c", noon(today - 40), None, None)];
        let report = report("/r", &facts(today, vec![], vec![], lives));
        let open = tile_of(&report, "tasks", "open");
        assert_eq!(open["value"], json!(1.0));
        assert_eq!(open["note"], json!("\u{2212}2 in 30 days"));
        let none = super::report("/r", &facts(today, vec![], vec![], vec![]));
        assert_eq!(tile_of(&none, "tasks", "open")["value"], json!(0.0));
        assert_eq!(tile_of(&none, "tasks", "open")["note"], json!("no change in 30 days"));
    }

    pub fn git_in(repo: &Path, args: &[&str], at: i64) {
        let date = format!("@{at} +0000");
        let out = Command::new("git").current_dir(repo).args(args).env("GIT_AUTHOR_DATE", &date).env("GIT_COMMITTER_DATE", &date).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn fixture(name: &str) -> PathBuf {
        let repo = std::env::temp_dir().join(format!("kanna-stats-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(&repo).unwrap();
        let today = local_day(now());
        let at = |day: i64, hour: i64| noon(day) as i64 + hour * 60;
        git_in(&repo, &["init", "-q", "-b", "main"], at(today - 40, 0));
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "long ago"], at(today - 40, 0));
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "plain"], at(today - 3, 0));
        git_in(&repo, &["checkout", "-q", "-b", "feature"], at(today - 2, 0));
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "one"], at(today - 2, 0));
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "two"], at(today - 2, 1));
        git_in(&repo, &["checkout", "-q", "main"], at(today - 2, 2));
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "beside"], at(today - 2, 3));
        git_in(&repo, &["merge", "-q", "--no-ff", "-m", "Merge feature: it", "feature"], at(today, 0));
        repo
    }

    #[test]
    fn main_s_order_is_its_time_order_even_when_commit_times_tie_or_go_back() {
        let repo = std::env::temp_dir().join(format!("kanna-stats-order-{}", std::process::id()));
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(&repo).unwrap();
        let today = local_day(now());
        let noon = noon(today) as i64;
        git_in(&repo, &["init", "-q", "-b", "main"], noon);
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "root"], noon);
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "same second"], noon);
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "clock behind"], noon - 3600);
        let commits = first_parents(&repo, day_start(today - DAYS + 1)).unwrap();
        let order: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(order, ["clock behind", "same second", "root"]);
        assert!(commits[0].at > commits[1].at && commits[1].at > commits[2].at, "{:?}", commits.iter().map(|c| c.at).collect::<Vec<_>>());
        assert_eq!(commits[2].at, noon as f64);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn the_sampler_counts_what_each_of_main_s_commits_brought_and_fills_only_what_is_missing() {
        let repo = fixture("sampler");
        let today = local_day(now());
        let since = day_start(today - DAYS + 1);
        let samples = collect(&repo, since, &Revisions::new(), &HashMap::new());
        assert_eq!(samples.len(), 3 * METRICS.len(), "three commits on main in 30 days, a row per metric each");
        let of = |metric: &str| samples.iter().filter(|s| s.metric == metric).map(|s| s.value).collect::<Vec<_>>();
        assert_eq!(of("commits"), [1.0, 1.0, 2.0]);
        assert_eq!(of("merged"), [0.0, 0.0, 1.0]);
        let mut sampled: Revisions = samples.iter().filter(|s| s.metric == "commits").map(|s| ((s.commit.clone(), s.metric.clone()), s.revision)).collect();
        let rest = collect(&repo, since, &sampled, &HashMap::new());
        assert_eq!(rest.len(), 3 * (METRICS.len() - 1));
        assert!(rest.iter().all(|s| s.metric != "commits"));
        sampled.extend(rest.iter().map(|s| ((s.commit.clone(), s.metric.clone()), s.revision)));
        let older = samples.iter().find(|s| s.metric == "erosion").unwrap();
        sampled.insert((older.commit.clone(), older.metric.clone()), older.revision - 1);
        let again = collect(&repo, since, &sampled, &HashMap::new());
        assert_eq!(again.iter().map(|s| (s.commit.as_str(), s.metric.as_str())).collect::<Vec<_>>(), [(older.commit.as_str(), "erosion")], "a sample from an older definition is taken again");
        let report = report("/r", &facts(today, vec![], samples, vec![]));
        assert_eq!(tile_of(&report, "main", "commits")["note"], json!("today · 4 in 30 days"));
        assert_eq!(tile_of(&report, "main", "commits")["value"], json!(2.0));
        assert_eq!(tile_of(&report, "main", "landed")["value"], json!(1.0));
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn the_command_samples_once_and_reads_back_from_the_db() {
        let repo = fixture("command");
        let state = std::env::temp_dir().join(format!("kanna-stats-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        let db = Rc::new(Db::open(&state));
        let (sender, mut events) = crate::runtime::channel();
        let stats = Stats::new(db.clone(), sender, &state);
        let path = repo.to_string_lossy().to_string();
        let (first, reply) = kanna_mux::responder();
        let mut reply = Box::pin(reply);
        let (second, _) = kanna_mux::responder();
        stats.ask(&json!({ "repo": path }), first);
        stats.ask(&json!({ "repo": path }), second);
        let mut repo_sampled;
        loop {
            let Ok(Event::Sampled(sampled)) = crate::runtime::next(&mut events, std::time::Duration::from_secs(10)) else { panic!("no samples") };
            repo_sampled = sampled.repo.clone();
            let done = sampled.done;
            stats.sampled(sampled);
            if done {
                break;
            }
        }
        let sampled = repo_sampled;
        assert!(events.try_recv().is_err(), "one catch-up for both requests");
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let std::task::Poll::Ready(Ok(report)) = reply.as_mut().poll(&mut cx) else { panic!("no reply") };
        assert_eq!(report["sections"][1]["tiles"][0]["note"], json!("today · 4 in 30 days"));
        let since = day_start(local_day(now()) - DAYS + 1);
        assert_eq!(db.sampled(&sampled, since).unwrap().len(), 3 * METRICS.len());
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&state);
    }

    use std::future::Future;
}
