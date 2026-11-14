use super::{repo_here, usage, Command, Flow};
use crate::args::Argv;
use crate::output::home_relative;
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "stats",
    group: "Information",
    summary: "Print this project's numbers over the last 30 days, as the Desk view's Stats tab shows them.",
    usage: "kanna stats\n       kanna stats pipelines [--json] [--days N]",
    options: &[("--days N", "pipelines: runs started in the last N days; 0 for every run. Default: 30.")],
    notes: "Run in the repo: its main checkout's numbers, from the daemon on this machine. Quiz: the share of your kept quiz answers right in the 30 days before each day, by kind and by area, and the misses not asked again. Main: commits and merges per day, from the daemon's samples of main's first-parent commits, taken when you ask. Tasks: made, first started and done per day, and the time from made to done. Code quality: how good the code on main is, measured at each of its commits. Pipelines: runs compared by pipeline and revision. It answers at once with what's stored: while the daemon catches up behind it, a line under the title (`status` with --json) says what it's doing. Each line is a number, its note and a sparkline of its 30 days, oldest first. pipelines prints the runs compared, with --json the numbers per pipeline, per run and per stage and machine.",
    examples: &[("kanna stats", "This project's numbers."), ("kanna stats --json", "The sections and tiles, as the Stats tab draws them."), ("kanna stats pipelines --days 90", "Runs of the last 90 days by pipeline and revision.")],
    parse,
    show,
    flow: Flow::PLAIN,
};

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let days = args.option("--days")?;
    let what = args.positional();
    let repo = repo_here().map_err(|_| usage("stats", "stats are a git repo's: run this in one"))?;
    let mut out = json!({ "repo": repo });
    match what.as_deref() {
        None if days.is_some() => return Err(usage("stats", "--days is for stats pipelines")),
        None => return Ok(out),
        Some("pipelines") => out["pipelines"] = json!(true),
        Some(other) => return Err(usage("stats", &format!("unknown \"{other}\" (pipelines)"))),
    }
    if let Some(days) = days {
        let days = days.parse::<u32>().map_err(|_| usage("stats", "--days takes a number of days"))?;
        out["days"] = json!(days);
    }
    Ok(out)
}

pub fn show(_: &Value, reply: &Value) -> String {
    let mut rows: Vec<[String; 5]> = vec![];
    for section in reply["sections"].as_array().into_iter().flatten() {
        let title = section["title"].as_str().unwrap_or("?").to_string();
        let tiles = section["tiles"].as_array().cloned().unwrap_or_default();
        if tiles.is_empty() {
            rows.push([title, section["empty"].as_str().unwrap_or("").to_string(), String::new(), String::new(), String::new()]);
            continue;
        }
        for (i, tile) in tiles.iter().enumerate() {
            let chart = &tile["chart"];
            let label = tile["label"].as_str().unwrap_or("?").to_string();
            let head = if i == 0 { title.clone() } else { String::new() };
            if chart["kind"] == "list" {
                rows.push([head, label, String::new(), String::new(), String::new()]);
                let listed = chart["rows"].as_array().cloned().unwrap_or_default();
                if listed.is_empty() {
                    rows.push([String::new(), format!("  {}", chart["empty"].as_str().unwrap_or("")), String::new(), String::new(), String::new()]);
                }
                for row in listed {
                    let detail = [row["detail"].as_str().unwrap_or(""), row["note"].as_str().unwrap_or("")].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" · ");
                    rows.push([String::new(), format!("  {}", row["label"].as_str().unwrap_or("?")), row["figure"].as_str().unwrap_or("").to_string(), String::new(), detail]);
                }
                continue;
            }
            if chart["kind"] == "table" {
                rows.push([head, label, String::new(), String::new(), String::new()]);
                for line in table_lines(chart) {
                    rows.push([String::new(), format!("  {line}"), String::new(), String::new(), String::new()]);
                }
                if let Some(note) = tile["note"].as_str().filter(|n| !n.is_empty()) {
                    rows.push([String::new(), format!("  {note}"), String::new(), String::new(), String::new()]);
                }
                continue;
            }
            if chart["kind"] == "runs" {
                let medians: Vec<&str> = chart["medians"].as_array().into_iter().flatten().filter_map(|m| m["label"].as_str()).collect();
                rows.push([head, format!("{label}: {}", medians.join(", ")), String::new(), String::new(), String::new()]);
                continue;
            }
            if chart["kind"] == "ranked" {
                rows.push([head, label, String::new(), String::new(), String::new()]);
                for row in chart["rows"].as_array().into_iter().flatten() {
                    rows.push([String::new(), format!("  {}", row["label"].as_str().unwrap_or("?")), figure(&row["value"], Some("%")), row["note"].as_str().unwrap_or("").to_string(), String::new()]);
                }
                continue;
            }
            let values = match chart["kind"].as_str() {
                Some("dots") => chart["line"].as_array().cloned().unwrap_or_default(),
                _ => chart["values"].as_array().cloned().unwrap_or_default(),
            };
            let unit = tile["unit"].as_str();
            rows.push([head, label, figure(&tile["value"], unit), tile["note"].as_str().unwrap_or("").to_string(), spark(&values, chart["max"].as_f64())]);
        }
    }
    let width = |column: usize| rows.iter().filter(|r| !r[2].is_empty() || column < 1).map(|r| r[column].chars().count()).max().unwrap_or(0);
    let widths = [width(0), width(1), width(2), width(3)];
    let mut out = vec![];
    if let Some(project) = reply["project"].as_str() {
        let window = match reply.get("days") {
            Some(Value::Number(days)) => format!("last {days} days"),
            Some(Value::Null) => "all runs".to_string(),
            _ => "last 30 days".to_string(),
        };
        out.push(format!("{} · main · {window}", home_relative(project)));
    }
    if let Some(status) = reply["status"].as_str() {
        out.push(status.to_string());
    }
    if reply["sections"].as_array().is_some_and(Vec::is_empty) {
        out.push("No numbers yet: run this again in a moment.".to_string());
    }
    for row in &rows {
        let line = if row[2].is_empty() && row[4].is_empty() {
            format!("{:<w0$}  {}", row[0], row[1], w0 = widths[0])
        } else {
            format!("{:<w0$}  {:<w1$}  {:>w2$}  {:<w3$}  {}", row[0], row[1], row[2], row[3], row[4], w0 = widths[0], w1 = widths[1], w2 = widths[2], w3 = widths[3])
        };
        out.push(line.trim_end().to_string());
    }
    out.join("\n")
}

fn table_lines(chart: &Value) -> Vec<String> {
    let columns = chart["columns"].as_array().cloned().unwrap_or_default();
    let cells = |row: &Value| -> Vec<String> { row["cells"].as_array().into_iter().flatten().map(|c| c["text"].as_str().unwrap_or("").to_string()).collect() };
    let mut lines: Vec<Vec<String>> = vec![columns.iter().map(|c| c["title"].as_str().unwrap_or("").replace('\n', " ")).collect()];
    lines.extend(chart["rows"].as_array().into_iter().flatten().map(cells));
    let width = |column: usize| lines.iter().filter_map(|l| l.get(column)).map(|t| t.chars().count()).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns.len()).map(width).collect();
    lines
        .iter()
        .map(|line| {
            let cell = |(i, text): (usize, &String)| if columns[i]["right"] == true { format!("{text:>w$}", w = widths[i]) } else { format!("{text:<w$}", w = widths[i]) };
            line.iter().enumerate().map(cell).collect::<Vec<_>>().join("  ").trim_end().to_string()
        })
        .collect()
}

fn spark(values: &[Value], max: Option<f64>) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let top = max.unwrap_or_else(|| values.iter().filter_map(Value::as_f64).fold(0.0, f64::max)).max(1.0);
    values.iter().map(|v| v.as_f64().map_or(' ', |v| BARS[((v / top).clamp(0.0, 1.0) * 7.0).round() as usize])).collect()
}

fn figure(value: &Value, unit: Option<&str>) -> String {
    match (value.as_f64(), unit) {
        (None, _) => "—".to_string(),
        (Some(v), Some("%")) => format!("{}%", v.round() as i64),
        (Some(v), Some("s")) => duration(v),
        (Some(v), Some("h")) => hours(v),
        (Some(v), Some("x")) => format!("{v:.1}"),
        (Some(v), _) => format!("{}", v.round() as i64),
    }
}

fn hours(hours: f64) -> String {
    if hours < 1.0 {
        format!("{}m", (hours * 60.0).round() as i64)
    } else if hours < 10.0 {
        format!("{}h", (hours * 10.0).round() / 10.0)
    } else {
        format!("{}h", hours.round() as i64)
    }
}

pub fn duration(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as i64;
    match minutes {
        m if m < 60 => format!("{m}m"),
        m if m < 48 * 60 && m % 60 == 0 => format!("{}h", m / 60),
        m if m < 48 * 60 => format!("{}h {}m", m / 60, m % 60),
        m => format!("{}d", (m as f64 / 1440.0).round() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::parse;

    #[test]
    fn hours_and_ratios_read_as_the_app_shows_them() {
        assert_eq!(figure(&json!(212.4), Some("h")), "212h");
        assert_eq!(figure(&json!(3.44), Some("h")), "3.4h");
        assert_eq!(figure(&json!(3.0), Some("h")), "3h");
        assert_eq!(figure(&json!(0.5), Some("h")), "30m");
        assert_eq!(figure(&json!(4.3), Some("x")), "4.3");
    }

    #[test]
    fn stats_pipelines_asks_for_its_window_and_prints_tables_and_medians() {
        let repo = parse("kanna quiz history").unwrap().1["repo"].clone();
        assert_eq!(parse("kanna stats pipelines").unwrap().1, json!({ "repo": repo, "pipelines": true }));
        assert_eq!(parse("kanna stats pipelines --days 0").unwrap().1, json!({ "repo": repo, "pipelines": true, "days": 0 }));
        assert!(parse("kanna stats --days 3").is_err());
        assert!(parse("kanna stats pipelines --days soon").is_err());
        let reply = json!({ "project": "/r", "days": 90, "sections": [{ "id": "pipelines", "title": "Pipelines", "tiles": [
            { "id": "pipelines", "label": "", "value": null, "note": "Per run: medians.", "chart": { "kind": "table", "columns": [{ "title": "Pipeline" }, { "title": "Time to land,\nmedian", "right": true }], "rows": [{ "cells": [{ "text": "feature rev 2" }, { "text": "3.4h" }] }] } },
            { "id": "run-land", "label": "Time to land", "value": null, "note": "", "chart": { "kind": "runs", "medians": [{ "label": "feature rev 2 median 3.4h" }] } },
        ] }] });
        let text = show(&json!({}), &reply);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "/r · main · last 90 days");
        assert_eq!(lines[2], "             Pipeline       Time to land, median");
        assert_eq!(lines[3], "             feature rev 2                  3.4h");
        assert_eq!(lines[4], "             Per run: medians.");
        assert_eq!(lines[5], "           Time to land: feature rev 2 median 3.4h");
    }

    #[test]
    fn stats_print_a_line_a_tile_with_its_sparkline() {
        let repo = parse("kanna quiz history").unwrap().1["repo"].clone();
        assert_eq!(parse("kanna stats").unwrap().1, json!({ "repo": repo }));
        let reply = json!({ "project": "/r", "days": [], "sections": [
            { "id": "quiz", "title": "Quiz", "tiles": [
                { "id": "right", "label": "Answers right", "value": 79.0, "unit": "%", "note": "of 95 answers", "chart": { "kind": "line", "max": 100, "values": [null, 50.0, 100.0] } },
                { "id": "kinds", "label": "By kind", "value": null, "note": "", "chart": { "kind": "ranked", "rows": [{ "label": "why", "value": 91.7, "note": "11/12" }] } },
            ] },
            { "id": "tasks", "title": "Tasks", "tiles": [
                { "id": "open", "label": "Open", "value": 2.0, "note": "+2 in 30 days", "chart": { "kind": "flow", "values": [0.0, 1.0, 2.0], "made": [0.0, 1.0, 3.0], "done": [0.0, 0.0, 2.0] } },
                { "id": "lead", "label": "Made to done", "value": 12780.0, "unit": "s", "note": "median of 3", "chart": { "kind": "dots", "points": [], "line": [null, 3600.0, 7200.0] } },
            ] },
            { "id": "code", "title": "Code quality", "tiles": [
                { "id": "erosion", "label": "Erosion", "value": 48.5, "unit": "%", "note": "up 1.2 points in 30 days", "chart": { "kind": "line", "max": 100, "values": [47.3, 48.5] } },
                { "id": "worst", "label": "Worst functions", "value": null, "note": "", "chart": { "kind": "list", "rows": [{ "label": "run", "detail": "crates/kanna-daemon/src/main.rs:215", "figure": "CC 154", "note": "677 lines", "open": { "path": "crates/kanna-daemon/src/main.rs", "line": 215 } }] } },
                { "id": "causes", "label": "Landings that needed the most fixes", "value": null, "note": "", "chart": { "kind": "list", "rows": [], "empty": "No fix traced back to a landing in 30 days." } },
            ] },
            { "id": "empty", "title": "Later", "empty": "Not measured yet.", "tiles": [] },
        ] });
        let text = show(&json!({}), &reply);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "/r · main · last 30 days");
        let mut updating = reply.clone();
        updating["status"] = json!("Updating: running coverage");
        let shown = show(&json!({}), &updating);
        assert_eq!(shown.lines().nth(1), Some("Updating: running coverage"));
        assert_eq!(shown.lines().nth(2), Some(lines[1]));
        assert_eq!(show(&json!({}), &json!({ "project": "/r", "sections": [], "status": null })), "/r · main · last 30 days\nNo numbers yet: run this again in a moment.");
        assert_eq!(lines[1], "Quiz          Answers right     79%  of 95 answers              ▅█");
        assert_eq!(lines[2], "              By kind");
        assert_eq!(lines[3], "                why             92%  11/12");
        assert_eq!(lines[4], "Tasks         Open                2  +2 in 30 days             ▁▅█");
        assert_eq!(lines[5], "              Made to done   3h 33m  median of 3                ▅█");
        assert_eq!(lines[6], "Code quality  Erosion           49%  up 1.2 points in 30 days  ▄▄");
        assert_eq!(lines[7], "              Worst functions");
        assert_eq!(lines[8], "                run          CC 154                            crates/kanna-daemon/src/main.rs:215 · 677 lines");
        assert_eq!(lines[10], "                No fix traced back to a landing in 30 days.");
        assert_eq!(lines[11], "Later         Not measured yet.");
        assert_eq!(duration(59.0 * 60.0), "59m");
        assert_eq!(duration(3.0 * 3600.0), "3h");
        assert_eq!(duration(50.0 * 3600.0), "2d");
    }
}
