use crate::db::AgentRow;
use crate::stats::{day_start, dots, local_day, median, median_line, section, tile, Facts, DAYS};
use kanna_mux::json::{json, Value};
use std::collections::{BTreeMap, HashSet};

const WATCHED_SECONDS: f64 = 5.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum End {
    Met,
    Dropped,
    Open,
}

#[derive(Clone, Debug)]
pub struct Span {
    pub task: String,
    pub name: String,
    pub from: f64,
    pub to: f64,
    pub end: End,
}

impl Span {
    fn seconds(&self) -> f64 {
        (self.to - self.from).max(0.0)
    }
}

#[derive(Default)]
pub struct Ledger {
    pub turns: Vec<Span>,
    pub waits: Vec<Span>,
    pub unread: Vec<Span>,
}

#[derive(Default)]
struct Agent {
    working: Option<f64>,
    waiting: Option<f64>,
    stopped: bool,
    unread: Option<f64>,
}

pub fn ledger(rows: &[AgentRow], now: f64) -> Ledger {
    let mut tasks: BTreeMap<&str, Vec<&AgentRow>> = BTreeMap::new();
    for row in rows {
        tasks.entry(&row.task).or_default().push(row);
    }
    let mut out = Ledger::default();
    for (task, rows) in tasks {
        let Some(first) = rows.first() else { continue };
        let span = |from: f64, to: f64, end: End| Span { task: task.to_string(), name: first.name.clone(), from, to, end };
        let mut agent = Agent::default();
        for row in rows.iter().copied() {
            let close = |open: &mut Option<f64>, into: &mut Vec<Span>, end: End| {
                if let Some(from) = open.take() {
                    into.push(span(from, row.at, end));
                }
            };
            match (row.what.as_str(), row.status.as_deref()) {
                ("status", Some("working")) => {
                    close(&mut agent.waiting, &mut out.waits, End::Met);
                    close(&mut agent.unread, &mut out.unread, End::Met);
                    agent.working.get_or_insert(row.at);
                }
                ("status", Some("needs-you")) => {
                    close(&mut agent.working, &mut out.turns, End::Met);
                    close(&mut agent.unread, &mut out.unread, End::Met);
                    agent.waiting.get_or_insert(row.at);
                    agent.stopped = false;
                }
                ("status", Some("idle")) => {
                    if agent.working.is_some() {
                        agent.unread.get_or_insert(row.at);
                    }
                    close(&mut agent.working, &mut out.turns, End::Met);
                    close(&mut agent.waiting, &mut out.waits, End::Met);
                }
                ("status", Some("exited")) | ("stopped", _) => {
                    close(&mut agent.working, &mut out.turns, End::Met);
                    agent.stopped = true;
                }
                ("started", _) | ("picked", _) => {
                    if agent.stopped {
                        close(&mut agent.waiting, &mut out.waits, End::Met);
                    }
                    if row.what == "picked" {
                        close(&mut agent.unread, &mut out.unread, End::Met);
                    }
                }
                ("looked", _) | ("activated", _) => close(&mut agent.unread, &mut out.unread, End::Met),
                ("done", _) => {
                    close(&mut agent.working, &mut out.turns, End::Met);
                    close(&mut agent.waiting, &mut out.waits, End::Dropped);
                    close(&mut agent.unread, &mut out.unread, End::Dropped);
                }
                _ => {}
            }
        }
        out.turns.extend(agent.working.map(|from| span(from, now, End::Open)));
        out.waits.extend(agent.waiting.map(|from| span(from, now, End::Open)));
        out.unread.extend(agent.unread.map(|from| span(from, now, End::Open)));
    }
    out
}

fn hours_per_day(first: i64, spans: &[Span]) -> Vec<f64> {
    (first..first + DAYS)
        .map(|day| {
            let (start, end) = (day_start(day), day_start(day + 1));
            let seconds: f64 = spans.iter().map(|s| (s.to.min(end) - s.from.max(start)).max(0.0)).sum();
            (seconds / 36.0).round() / 100.0
        })
        .collect()
}

pub fn hours_text(hours: f64) -> String {
    if hours < 1.0 {
        format!("{}m", (hours * 60.0).round() as i64)
    } else if hours < 10.0 {
        format!("{}h", (hours * 10.0).round() / 10.0)
    } else {
        format!("{}h", hours.round() as i64)
    }
}

fn total_tile(id: &str, label: &str, days: Vec<f64>, what: &str) -> Value {
    let today = days.last().copied().unwrap_or(0.0);
    let total = (days.iter().sum::<f64>() * 100.0).round() / 100.0;
    tile(id, label, json!(total), Some("h"), format!("in 30 days · {} today", hours_text(today)), 2, json!({ "kind": "columns", "values": days, "what": what }))
}

fn within(first: i64, today: i64, at: f64) -> Option<i64> {
    let day = local_day(at);
    (first..=today).contains(&day).then_some(day - first)
}

fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn median_tile(id: &str, label: &str, first: i64, today: i64, spans: &[Span], pending: usize, still: &str, what: &str) -> Value {
    let points: Vec<(i64, f64, &str)> = spans.iter().filter(|s| s.end == End::Met).filter_map(|s| within(first, today, s.to).map(|day| (day, s.seconds(), s.name.as_str()))).collect();
    let mut all: Vec<f64> = points.iter().map(|(_, seconds, _)| *seconds).collect();
    let middle = median(&mut all);
    let mut notes = vec![];
    if !points.is_empty() {
        notes.push(format!("median of {}", points.len()));
    }
    if pending > 0 {
        notes.push(format!("{pending} {still}"));
    }
    tile(id, label, middle.map_or(Value::Null, |m| json!(m.round())), Some("s"), notes.join(" · "), 2, dots(&points, median_line(&points), what))
}

fn turns_tile(first: i64, today: i64, turns: &[(i64, &Span)]) -> Value {
    let per_task = |of: &[&(i64, &Span)]| {
        let tasks: HashSet<&str> = of.iter().map(|(_, s)| s.task.as_str()).collect();
        (of.len(), tasks.len())
    };
    let days: Vec<Value> = (0..=today - first)
        .map(|day| match per_task(&turns.iter().filter(|(d, _)| *d == day).collect::<Vec<_>>()) {
            (_, 0) => Value::Null,
            (count, tasks) => json!((count as f64 * 10.0 / tasks as f64).round() / 10.0),
        })
        .collect();
    let (count, tasks) = per_task(&turns.iter().collect::<Vec<_>>());
    let value = if tasks == 0 { Value::Null } else { json!((count as f64 * 10.0 / tasks as f64).round() / 10.0) };
    let note = if tasks == 0 { String::new() } else { format!("{} · {}", counted(count, "turn", "turns"), counted(tasks, "task", "tasks")) };
    tile("turns", "Turns per task", value, Some("x"), note, 2, json!({ "kind": "line", "values": days, "what": "turns per task that day" }))
}

fn length_tile(first: i64, today: i64, turns: &[(i64, &Span)]) -> Value {
    let days: Vec<Value> = (0..=today - first)
        .map(|day| {
            let mut near: Vec<f64> = turns.iter().filter(|(d, _)| *d == day).map(|(_, s)| s.seconds()).collect();
            median(&mut near).map_or(Value::Null, |m| json!(m.round()))
        })
        .collect();
    let mut all: Vec<f64> = turns.iter().map(|(_, s)| s.seconds()).collect();
    let note = if turns.is_empty() { String::new() } else { format!("median of {}", turns.len()) };
    tile("turn-length", "Turn length", median(&mut all).map_or(Value::Null, |m| json!(m.round())), Some("s"), note, 2, json!({ "kind": "line", "values": days, "what": "median turn that day" }))
}

pub fn section_of(facts: &Facts) -> Value {
    let (id, title, note) = ("agents", "Agents", "task agents, from each change of their status");
    if !facts.agents.iter().any(|row| row.what == "status") {
        let mut empty = section(id, title, note, vec![]);
        empty["empty"] = json!("No agent status recorded yet. Once a task's agent runs, its time shows here.");
        return empty;
    }
    let first = facts.today - DAYS + 1;
    let book = ledger(&facts.agents, facts.now);
    let looked: Vec<Span> = book.unread.iter().filter(|s| s.end != End::Met || s.seconds() >= WATCHED_SECONDS).cloned().collect();
    let turns: Vec<(i64, &Span)> = book.turns.iter().filter(|s| s.end == End::Met).filter_map(|s| within(first, facts.today, s.to).map(|day| (day, s))).collect();
    let pending = |spans: &[Span]| spans.iter().filter(|s| s.end == End::Open).count();
    section(
        id,
        title,
        note,
        vec![
            total_tile("agent-time", "Agent time", hours_per_day(first, &book.turns), "agents worked"),
            total_tile("waiting", "Waiting on you", hours_per_day(first, &book.waits), "at needs-you"),
            median_tile("answer", "Time to your answer", first, facts.today, &book.waits, pending(&book.waits), "waiting now", "7-day median"),
            median_tile("unread", "Done, not looked at", first, facts.today, &looked, pending(&looked), "not looked at yet", "7-day median"),
            turns_tile(first, facts.today, &turns),
            length_tile(first, facts.today, &turns),
        ],
    )
}
