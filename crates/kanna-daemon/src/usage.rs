use crate::stats::days_from_civil;
use crate::tasks;
use kanna_mux::json::{json, string, Value};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub turns: u64,
    pub tools: u64,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub written: u64,
    pub cost: Option<f64>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cached: f64,
    pub written: f64,
}

impl Usage {
    fn add(&mut self, other: Usage) {
        self.turns += other.turns;
        self.tools += other.tools;
        self.input += other.input;
        self.output += other.output;
        self.cached += other.cached;
        self.written += other.written;
        self.model = self.model.take().or(other.model);
    }

    pub fn priced(&self, prices: &Value) -> Option<f64> {
        let price = price_of(prices, self.model.as_deref()?)?;
        let million = |count: u64, per: f64| count as f64 * per / 1e6;
        Some(million(self.input, price.input) + million(self.output, price.output) + million(self.cached, price.cached) + million(self.written, price.written))
    }

    pub fn doc(&self, prices: &Value) -> Value {
        let (cost, by) = match (self.cost, self.priced(prices)) {
            (Some(cost), _) => (Some(cost), Some("agent")),
            (None, Some(cost)) => (Some(cost), Some("pricing")),
            _ => (None, None),
        };
        json!({ "turns": self.turns, "tools": self.tools, "in": self.input, "out": self.output, "cached": self.cached, "written": self.written, "cost": cost, "costBy": by, "model": self.model })
    }
}

fn price_of(prices: &Value, model: &str) -> Option<Price> {
    let entry = prices.get(model)?;
    let per = |key: &str| entry.get(key).and_then(Value::as_f64);
    let (input, output) = (per("in")?, per("out")?);
    Some(Price { input, output, cached: per("cached").unwrap_or(input), written: per("written").unwrap_or(input) })
}

fn count(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub fn instant(text: &str) -> Option<f64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let mut time = time.split(':');
    let (hour, minute) = (time.next()?.parse::<i64>().ok()?, time.next()?.parse::<i64>().ok()?);
    let second = time.next()?.parse::<f64>().ok()?;
    Some((days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60) as f64 + second)
}

fn lines(path: &Path) -> impl Iterator<Item = Value> {
    let reader = fs::File::open(path).ok().map(BufReader::new);
    reader.into_iter().flat_map(|r| r.lines()).map_while(Result::ok).filter_map(|line| serde_json::from_str(&line).ok())
}

pub fn claude_file(path: &Path, since: f64) -> Usage {
    let mut usage = Usage::default();
    let (mut messages, mut tools) = (HashSet::new(), HashSet::new());
    let (mut before, mut last, mut started) = (None, None, false);
    for line in lines(path) {
        if string(&line, "type") == Some("cost-state") {
            let cost = line.get("totalCostUSD").and_then(Value::as_f64);
            if started {
                last = cost;
            } else {
                before = cost;
            }
            continue;
        }
        if string(&line, "timestamp").and_then(instant).is_some_and(|at| at >= since) {
            started = true;
        }
        if !started || string(&line, "type") != Some("assistant") {
            continue;
        }
        let Some(message) = line.get("message") else { continue };
        for block in message.get("content").and_then(Value::as_array).into_iter().flatten() {
            if string(block, "type") == Some("tool_use") && string(block, "id").is_some_and(|id| tools.insert(id.to_string())) {
                usage.tools += 1;
            }
        }
        let model = string(message, "model").filter(|m| !m.starts_with('<'));
        let (Some(id), Some(tokens), Some(model)) = (string(message, "id"), message.get("usage"), model) else { continue };
        if !messages.insert(id.to_string()) {
            continue;
        }
        usage.turns += 1;
        usage.input += count(tokens, "input_tokens");
        usage.output += count(tokens, "output_tokens");
        usage.cached += count(tokens, "cache_read_input_tokens");
        usage.written += count(tokens, "cache_creation_input_tokens");
        usage.model = Some(model.to_string());
    }
    usage.cost = last.map(|last| last - before.unwrap_or(0.0));
    usage
}

pub fn claude(projects: &Path, session: &str, since: f64) -> Option<Usage> {
    let file = format!("{session}.jsonl");
    let folder = fs::read_dir(projects).ok()?.flatten().map(|e| e.path()).find(|p| p.join(&file).is_file())?;
    let mut usage = claude_file(&folder.join(&file), since);
    let agents = fs::read_dir(folder.join(session).join("subagents")).into_iter().flatten().flatten();
    for agent in agents.map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "jsonl")) {
        usage.add(claude_file(&agent, since));
    }
    Some(usage)
}

pub fn codex_file(path: &Path, since: f64) -> Usage {
    let mut usage = Usage::default();
    let (mut baseline, mut last, mut started) = (None, None, false);
    for line in lines(path) {
        if string(&line, "timestamp").and_then(instant).is_some_and(|at| at >= since) {
            started = true;
        }
        if string(&line, "type") == Some("turn_context") {
            usage.model = line.get("payload").and_then(|p| string(p, "model")).map(String::from).or(usage.model.take());
            continue;
        }
        let Some(payload) = line.get("payload") else { continue };
        match (string(&line, "type"), string(payload, "type")) {
            (Some("event_msg"), Some("token_count")) => {
                let Some(total) = payload.get("info").and_then(|i| i.get("total_token_usage")).filter(|t| t.is_object()) else { continue };
                if !started {
                    baseline = Some(total.clone());
                } else if last.as_ref() != Some(total) && baseline.as_ref() != Some(total) {
                    usage.turns += 1;
                    last = Some(total.clone());
                }
            }
            (Some("response_item"), Some("function_call" | "custom_tool_call" | "local_shell_call" | "web_search_call")) if started => usage.tools += 1,
            _ => {}
        }
    }
    let Some(last) = last else { return usage };
    let spent = |key: &str| count(&last, key).saturating_sub(baseline.as_ref().map_or(0, |b| count(b, key)));
    usage.cached = spent("cached_input_tokens");
    usage.written = spent("cache_write_input_tokens");
    usage.input = spent("input_tokens").saturating_sub(usage.cached + usage.written);
    usage.output = spent("output_tokens");
    usage
}

fn rollout(sessions: &Path, session: &str) -> Option<PathBuf> {
    let ending = format!("-{session}.jsonl");
    let newest = |folder: &Path| -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(folder).into_iter().flatten().flatten().map(|e| e.path()).collect();
        found.sort();
        found.reverse();
        found
    };
    let days = newest(sessions).into_iter().flat_map(|year| newest(&year)).flat_map(|month| newest(&month)).filter(|day| day.is_dir());
    days.flat_map(|day| newest(&day)).find(|file| file.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&ending)))
}

pub fn codex(sessions: &Path, session: &str, since: f64) -> Option<Usage> {
    Some(codex_file(&rollout(sessions, session)?, since))
}

fn claude_home() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()).map_or_else(|| tasks::home().join(".claude"), PathBuf::from)
}

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()).map_or_else(|| tasks::home().join(".codex"), PathBuf::from)
}

pub fn of(agent: &str, session: &str, since: f64) -> Option<Usage> {
    match agent {
        "claude" => claude(&claude_home().join("projects"), session, since),
        "codex" => codex(&codex_home().join("sessions"), session, since),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sessions").join(name)
    }

    #[test]
    fn claude_s_recording_counts_each_message_once_and_gives_its_own_cost() {
        let usage = claude_file(&fixture("claude.jsonl"), 0.0);
        assert_eq!(usage, Usage { turns: 3, tools: 1, input: 30, output: 30, cached: 90, written: 60, cost: Some(0.001038), model: Some("mock-model".into()) });
    }

    #[test]
    fn codex_s_recording_gives_its_last_totals_with_cached_out_of_in() {
        let usage = codex_file(&fixture("codex.jsonl"), 0.0);
        assert_eq!(usage, Usage { turns: 3, tools: 1, input: 18, output: 30, cached: 12, written: 0, cost: None, model: Some("mock-model".into()) });
    }

    #[test]
    fn a_session_resumed_by_a_later_visit_counts_only_what_came_after() {
        let times = |name: &str, kind: &str| -> Vec<f64> {
            let text = fs::read_to_string(fixture(name)).unwrap();
            text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|l| l.get("payload").map_or(string(l, "type"), |p| string(p, "type")) == Some(kind)).filter_map(|l| instant(string(&l, "timestamp")?)).collect()
        };
        let claude_at = times("claude.jsonl", "assistant");
        let later = claude_file(&fixture("claude.jsonl"), claude_at[1]);
        assert_eq!((later.turns, later.input, later.tools), (2, 20, 1));
        let codex_at = times("codex.jsonl", "token_count");
        let later = codex_file(&fixture("codex.jsonl"), codex_at[0] + 0.001);
        assert_eq!((later.turns, later.input, later.cached, later.output), (2, 12, 8, 20));
    }

    #[test]
    fn a_session_is_found_by_its_id_wherever_its_project_folder_is_and_adds_its_subagents() {
        let root = std::env::temp_dir().join(format!("kanna-usage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let folder = root.join("projects/-some-folder");
        fs::create_dir_all(folder.join("abc/subagents")).unwrap();
        fs::copy(fixture("claude.jsonl"), folder.join("abc.jsonl")).unwrap();
        fs::copy(fixture("claude.jsonl"), folder.join("abc/subagents/agent-1.jsonl")).unwrap();
        let usage = claude(&root.join("projects"), "abc", 0.0).unwrap();
        assert_eq!((usage.turns, usage.input, usage.cost), (6, 60, Some(0.001038)));
        assert_eq!(claude(&root.join("projects"), "nope", 0.0), None);
        let day = root.join("sessions/2026/11/04");
        fs::create_dir_all(&day).unwrap();
        fs::copy(fixture("codex.jsonl"), day.join("rollout-2026-11-04T07-15-17-01a1-77.jsonl")).unwrap();
        assert_eq!(codex(&root.join("sessions"), "01a1-77", 0.0).map(|u| u.turns), Some(3));
        assert_eq!(codex(&root.join("sessions"), "nope", 0.0), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn instants_are_utc_seconds() {
        assert_eq!(instant("1970-01-02T00:00:01.500Z"), Some(86401.5));
        assert_eq!(instant("2026-11-03T11:15:18.008Z"), Some(1793704518.008));
        assert_eq!(instant("yesterday"), None);
    }

    #[test]
    fn prices_are_per_million_and_a_model_without_one_has_none() {
        let usage = Usage { input: 1_000_000, output: 2_000_000, cached: 4_000_000, written: 0, model: Some("m".into()), ..Usage::default() };
        let prices = json!({ "m": { "in": 3.0, "out": 15.0, "cached": 0.25 } });
        assert_eq!(usage.priced(&prices), Some(34.0));
        assert_eq!(usage.doc(&prices)["costBy"], json!("pricing"));
        assert_eq!(Usage { model: Some("other".into()), ..usage.clone() }.priced(&prices), None);
        let own = Usage { cost: Some(1.5), ..usage };
        assert_eq!((own.doc(&prices)["cost"].clone(), own.doc(&prices)["costBy"].clone()), (json!(1.5), json!("agent")));
    }
}
