use super::{repo_here, usage, Command, Flow};
use crate::args::Argv;
use crate::output::home_relative;
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "stats",
    group: "Information",
    summary: "Print this project's numbers over the last 30 days, as the Desk view's Stats tab shows them.",
    usage: "kanna stats",
    options: &[],
    notes: "Run in the repo: its main checkout's numbers, from the daemon on this machine. Quiz: the share of your kept quiz answers right in the 30 days before each day, by kind and by area, and the misses not asked again. Main: commits and merges per day, from the daemon's samples of main's first-parent commits, taken when you ask. Tasks: made, first started and done per day, and the time from made to done. Code quality: how good the code on main is, measured at each of its commits. It answers at once with what's stored: while the daemon catches up behind it, a line under the title (`status` with --json) says what it's doing. Each line is a number, its note and a sparkline of its 30 days, oldest first.",
    examples: &[("kanna stats", "This project's numbers."), ("kanna stats --json", "The sections and tiles, as the Stats tab draws them.")],
    parse,
    show,
    flow: Flow::PLAIN,
};

fn parse(_: &mut Argv) -> Result<Value, Failure> {
    let repo = repo_here().map_err(|_| usage("stats", "stats are a git repo's: run this in one"))?;
    Ok(json!({ "repo": repo }))
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
        out.push(format!("{} · main · last 30 days", home_relative(project)));
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
        (Some(v), _) => format!("{}", v.round() as i64),
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
