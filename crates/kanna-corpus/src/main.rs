use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use kanna_detect::{store, Agent, Screen, Source, Tracker, Turn};
use kanna_vt::{Lines, Mode, Terminal};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: kanna-corpus RECORDING.jsonl --version VERSION --out DIR [--per-phase N]
       kanna-corpus --turns RECORDING.jsonl... [--version VERSION]

Replays a tools/agent-harness recording through libghostty-vt, samples the
settled screen every quarter second, labels each sample by the phase the
harness marked (idle, working, needs-you) and writes the distinct ones to
DIR as corpus cases. Samples near a phase's edges, and phases whose end
the harness never saw (a timeout), are left out.

--turns replays each recording through its agent's bundled rules and the
daemon's status tracker every 20 ms, and prints, for each settle time a
Shelf task might wait, when its turn would be judged over: during a turn
the harness marked as the agent's (too early), or after it.";

const STEP: f64 = 0.25;
const SETTLE: f64 = 0.06;

struct Event {
    t: f64,
    kind: String,
    bytes: Vec<u8>,
    data: Value,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--turns") {
        return turns(&args[1..]);
    }
    let option = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let (Some(recording), Some(version), Some(out)) = (args.first().filter(|a| !a.starts_with('-')), option("--version"), option("--out")) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let per_phase: usize = option("--per-phase").and_then(|n| n.parse().ok()).unwrap_or(6);
    match sample(Path::new(recording), &version, Path::new(&out), per_phase) {
        Ok(written) => println!("{recording}: {written} cases"),
        Err(error) => {
            eprintln!("kanna-corpus: {recording}: {error}");
            std::process::exit(1);
        }
    }
}

fn read(path: &Path) -> Result<Vec<Event>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
            let bytes = v.get("b").and_then(Value::as_str).map(|b| BASE64.decode(b).unwrap_or_default()).unwrap_or_default();
            Ok(Event { t: v["t"].as_f64().unwrap_or(0.0), kind: v["k"].as_str().unwrap_or("").into(), bytes, data: v.get("d").cloned().unwrap_or(Value::Null) })
        })
        .collect()
}

fn windows(events: &[Event]) -> Result<Vec<(&'static str, f64, f64)>, String> {
    let mark = |name: &str| events.iter().find(|e| e.kind == "mark" && e.data == name).map(|e| e.t).ok_or(format!("no {name} mark"));
    let enter_after = |t: f64| events.iter().find(|e| e.kind == "i" && e.t >= t && e.bytes == b"\r").map(|e| e.t).ok_or("no Enter".to_string());
    let matched_after = |t: f64| events.iter().find(|e| e.kind == "matched" && e.t >= t).map(|e| e.t).ok_or("no match".to_string());
    let (startup, plain, plain_end, tool, asking, approve, idle, end) =
        (mark("startup")?, mark("plain")?, mark("plain_end")?, mark("tool")?, mark("asking")?, mark("approve")?, mark("idle")?, mark("end")?);
    let ready = matched_after(startup)?;
    let (plain_enter, tool_enter) = (enter_after(plain)?, enter_after(tool)?);
    let timed_out = |from: f64, to: f64| events.iter().any(|e| e.kind == "timeout" && e.t >= from && e.t <= to + 0.6);
    let all = vec![
        ("idle", ready + 2.0, plain - 0.3),
        ("working", plain_enter + 0.6, plain_end - 0.3),
        ("idle", plain_end + 3.0, tool_enter - 0.3),
        ("working", tool_enter + 0.6, asking - 0.5),
        ("needs-you", asking + 0.5, approve - 0.2),
        ("working", approve + 0.6, idle - 0.5),
        ("idle", idle + 3.0, end - 0.5),
    ];
    Ok(all.into_iter().filter(|(_, from, to)| !timed_out(*from, *to)).collect())
}

fn sample(recording: &Path, version: &str, out: &Path, per_phase: usize) -> Result<usize, String> {
    let events = read(recording)?;
    let start = events.iter().find(|e| e.kind == "start").ok_or("no start")?;
    let (cols, rows) = (start.data["cols"].as_u64().unwrap_or(120) as u16, start.data["rows"].as_u64().unwrap_or(40) as u16);
    let phases = windows(&events)?;
    let name = recording.file_stem().and_then(|s| s.to_str()).unwrap_or("recording");
    let mut terminal = Terminal::new(cols, rows, 1000).map_err(|e| e.to_string())?;
    let outputs: Vec<&Event> = events.iter().filter(|e| e.kind == "o").collect();
    let mut next = 0;
    let mut last_output = f64::NEG_INFINITY;
    let mut samples: Vec<(&'static str, usize, f64, Value)> = vec![];
    let end = events.last().map(|e| e.t).unwrap_or(0.0);
    let mut t = 0.0;
    while t <= end {
        while next < outputs.len() && outputs[next].t <= t {
            terminal.write(&outputs[next].bytes);
            last_output = outputs[next].t;
            next += 1;
        }
        let settled = t - last_output >= SETTLE && !terminal.mode(Mode::SYNC_OUTPUT);
        if let Some((phase, label, _, _)) = phases.iter().enumerate().find(|(_, (_, from, to))| t >= *from && t <= *to).map(|(i, (l, f, to))| (i, *l, f, to)) {
            if settled {
                let screen: Vec<String> = terminal.text(Lines::Screen).lines().map(str::to_owned).collect();
                let case = json!({
                    "rows": screen,
                    "title": terminal.title(),
                    "progress": terminal.progress().as_str(),
                    "cliVersion": version,
                    "expected": label,
                    "source": format!("tools/agent-harness {name} @ {t:.2}s"),
                    "size": format!("{cols}x{rows}"),
                });
                samples.push((label, phase, t, case));
            }
        }
        t = ((t / STEP).round() + 1.0) * STEP;
    }
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut written = 0;
    let mut seen = HashSet::new();
    for phase in 0..phases.len() {
        let distinct: Vec<&(&str, usize, f64, Value)> = samples
            .iter()
            .filter(|s| s.1 == phase)
            .filter(|s| seen.insert((s.3["rows"].to_string(), s.3["title"].to_string(), s.3["progress"].to_string())))
            .collect();
        let step = (distinct.len() as f64 / per_phase as f64).max(1.0);
        for k in 0..distinct.len().min(per_phase) {
            let (label, _, t, case) = distinct[(k as f64 * step) as usize];
            let file: PathBuf = out.join(format!("{name}-{label}-{t:06.2}.json"));
            std::fs::write(&file, serde_json::to_string_pretty(case).unwrap() + "\n").map_err(|e| e.to_string())?;
            written += 1;
        }
    }
    Ok(written)
}

const TURN_STEP: f64 = 0.02;
const SETTLES: [f64; 5] = [0.5, 1.0, 2.0, 3.0, 5.0];

fn turns(args: &[String]) {
    let version = args.iter().position(|a| a == "--version").and_then(|i| args.get(i + 1)).cloned();
    let files: Vec<&String> = args.iter().enumerate().filter(|(i, a)| !a.starts_with("--") && args.get(i.wrapping_sub(1)).is_none_or(|p| p != "--version")).map(|(_, a)| a).collect();
    if files.is_empty() {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    for file in files {
        match replay(Path::new(file), version.as_deref()) {
            Ok(report) => print!("{report}"),
            Err(error) => eprintln!("kanna-corpus: {file}: {error}"),
        }
    }
}

fn replay(recording: &Path, version: Option<&str>) -> Result<String, String> {
    let events = read(recording)?;
    let name = recording.file_stem().and_then(|s| s.to_str()).unwrap_or("recording");
    let agent_name = name.split('-').next().unwrap_or("");
    let (_, text) = store::bundled().find(|(n, _)| *n == agent_name).ok_or(format!("no bundled rules for {agent_name}"))?;
    let agent = Agent::parse(text, Source::Bundled)?;
    let start = events.iter().find(|e| e.kind == "start").ok_or("no start")?;
    let (cols, rows) = (start.data["cols"].as_u64().unwrap_or(120) as u16, start.data["rows"].as_u64().unwrap_or(40) as u16);
    let mark = |name: &str| events.iter().find(|e| e.kind == "mark" && e.data == name).map(|e| e.t);
    let enter_after = |t: f64| events.iter().find(|e| e.kind == "i" && e.t >= t && e.bytes == b"\r").map(|e| e.t);
    let turns: Vec<(f64, f64)> = [(mark("plain").and_then(enter_after), mark("plain_end")), (mark("tool").and_then(enter_after), mark("asking")), (mark("approve"), mark("idle"))]
        .into_iter()
        .filter_map(|(from, to)| Some((from?, to?)))
        .filter(|(from, to)| !events.iter().any(|e| e.kind == "timeout" && e.t >= *from && e.t <= to + 0.6))
        .collect();
    let mut terminal = Terminal::new(cols, rows, 1000).map_err(|e| e.to_string())?;
    let outputs: Vec<&Event> = events.iter().filter(|e| e.kind == "o").collect();
    let base = std::time::Instant::now();
    let at = |t: f64| base + std::time::Duration::from_secs_f64(t);
    let mut tracker = Tracker::default();
    let mut machines: Vec<(f64, Turn, Vec<(f64, &'static str)>)> = SETTLES.iter().map(|s| (*s, Turn::default(), vec![])).collect();
    let mut shown: Vec<(f64, &'static str)> = vec![];
    let (mut next, mut last_output) = (0, f64::NEG_INFINITY);
    let end = events.last().map(|e| e.t).unwrap_or(0.0);
    let mut t = 0.0;
    while t <= end {
        while next < outputs.len() && outputs[next].t <= t {
            terminal.write(&outputs[next].bytes);
            last_output = outputs[next].t;
            next += 1;
        }
        if t - last_output >= SETTLE && !terminal.mode(Mode::SYNC_OUTPUT) {
            let screen = Screen { lines: terminal.text(Lines::Screen).lines().map(str::to_owned).collect(), title: terminal.title(), progress: terminal.progress().as_str().into() };
            if let Some(status) = tracker.judge(agent.evaluate(&screen, version).status, at(t)) {
                shown.push((t, status.as_str()));
                for (_, turn, _) in machines.iter_mut() {
                    turn.see(Some(status), at(t));
                }
            }
        }
        for (settle, turn, ended) in machines.iter_mut() {
            if let Some(status) = turn.ended(at(t), std::time::Duration::from_secs_f64(*settle)) {
                ended.push((t, status.as_str()));
                *turn = Turn::default();
            }
        }
        t += TURN_STEP;
    }
    let inside = |t: f64| turns.iter().any(|(from, to)| t > *from && t < *to);
    let mut out = format!("{name}\n  turns (harness): {}\n  status shown: {}\n", turns.iter().map(|(f, t)| format!("{f:.2}–{t:.2}")).collect::<Vec<_>>().join(", "), shown.iter().map(|(t, s)| format!("{t:.2} {s}")).collect::<Vec<_>>().join(", "));
    for (settle, _, ended) in &machines {
        let early = ended.iter().filter(|(t, _)| inside(t - settle)).count();
        out += &format!("  settle {settle:.1}s: {} ended, {early} during a turn: {}\n", ended.len(), ended.iter().map(|(t, s)| format!("{t:.2} {s}{}", if inside(t - settle) { " (early)" } else { "" })).collect::<Vec<_>>().join(", "));
    }
    Ok(out)
}
