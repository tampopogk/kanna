use crate::db::Sample;
use crate::stats::{area, local_day, section, tile, Commit, Context, Facts, DAYS};
use kanna_mux::json::{json, Value};
use kanna_quality::szz;
use kanna_quality::tree::{counted, Ranked, Tree, HIGH_COMPLEXITY};
use std::collections::{BTreeMap, HashMap, HashSet};

const WORST: usize = 10;

const FILES: usize = 40;

const LISTED: usize = 8;

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn function_row(ranked: &Ranked) -> Value {
    let f = ranked.function;
    json!({ "name": f.name, "file": ranked.file, "line": f.line, "cc": f.cyclomatic, "cognitive": f.cognitive, "lines": f.lines, "calls": f.calls.len(), "mass": (f.mass() * 10.0).round() / 10.0 })
}

fn share(value: Option<f64>) -> f64 {
    value.map_or(0.0, |v| (v * 1000.0).round() / 10.0)
}

pub fn erosion(tree: &Tree) -> (f64, Value) {
    let files: Vec<Value> = tree
        .files_by_complexity()
        .into_iter()
        .take(FILES)
        .map(|(file, cognitive, lines)| json!({ "file": file, "cognitive": cognitive, "lines": lines }))
        .collect();
    let (mut high, mut all) = (0.0, 0.0);
    for f in tree.files.iter().flat_map(|f| &f.facts.functions) {
        let mass = f.cognitive as f64 * (f.lines as f64).sqrt();
        all += mass;
        if f.cognitive > HIGH_COMPLEXITY {
            high += mass;
        }
    }
    let worst: Vec<Value> = tree.worst().iter().take(WORST).map(function_row).collect();
    (share(tree.erosion()), json!({ "gini": tree.gini().map(|g| (g * 1000.0).round() / 1000.0), "cognitive": share((all > 0.0).then(|| high / all)), "worst": worst, "files": files }))
}

pub fn verbosity(tree: &Tree) -> (f64, Value) {
    let clones = tree.clones();
    let groups: Vec<Value> = clones
        .groups
        .iter()
        .take(WORST)
        .map(|g| json!({ "lines": g.lines, "count": g.places.len(), "places": g.places.iter().take(6).map(|p| json!({ "file": p.file, "line": p.line, "end": p.end })).collect::<Vec<_>>() }))
        .collect();
    let wrapper_lines: usize = tree.files.iter().map(|f| f.facts.wrapped.len()).sum();
    let flagged_lines: usize = tree.files.iter().map(|f| f.facts.flagged.len()).sum();
    let mut by_rule: BTreeMap<&str, (usize, usize, Vec<(&str, usize, usize)>)> = BTreeMap::new();
    for file in &tree.files {
        for hit in &file.facts.hits {
            let entry = by_rule.entry(hit.rule).or_default();
            entry.0 += 1;
            entry.1 += hit.end - hit.line + 1;
            entry.2.push((file.path.as_str(), hit.line, hit.end));
        }
    }
    let mut rules: Vec<_> = by_rule.into_iter().collect();
    rules.sort_by(|a, b| b.1 .1.cmp(&a.1 .1).then_with(|| a.0.cmp(b.0)));
    let rules: Vec<Value> = rules
        .into_iter()
        .take(WORST)
        .map(|(rule, (hits, lines, mut places))| {
            places.sort_by(|a, b| (b.2 - b.1).cmp(&(a.2 - a.1)).then_with(|| a.0.cmp(b.0)).then_with(|| a.1.cmp(&b.1)));
            json!({ "rule": rule, "hits": hits, "lines": lines, "places": places.iter().take(6).map(|(file, line, end)| json!({ "file": file, "line": line, "end": end })).collect::<Vec<_>>() })
        })
        .collect();
    (share(tree.verbosity()), json!({ "clones": clones.duplicated_lines(), "wrappers": wrapper_lines, "flagged": flagged_lines, "lines": tree.lines(), "groups": groups, "rules": rules }))
}

pub fn high(tree: &Tree) -> (f64, Value) {
    (tree.high() as f64, json!({ "functions": tree.functions().len() }))
}

pub fn dead(tree: &Tree) -> (f64, Value) {
    let dead = tree.dead();
    (dead.len() as f64, json!({ "functions": dead.iter().take(WORST).map(function_row).collect::<Vec<_>>() }))
}

pub fn wrappers(tree: &Tree) -> (f64, Value) {
    let wrappers = tree.wrappers();
    (wrappers.len() as f64, json!({ "functions": wrappers.iter().take(WORST).map(function_row).collect::<Vec<_>>() }))
}

pub fn lines(tree: &Tree) -> (f64, Value) {
    (tree.lines() as f64, json!({ "files": tree.files.len() }))
}

fn base(commit: &Commit) -> &str {
    commit.parents.first().map_or(EMPTY_TREE, String::as_str)
}

pub fn churn(cx: &Context, commit: &Commit) -> Result<(f64, Value), String> {
    let numstat = crate::tasks::git(cx.repo, &["diff", "--numstat", "--no-renames", base(commit), &commit.id])?;
    let (mut added, mut deleted, mut files) = (0.0, 0.0, Vec::new());
    for line in numstat.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        if !counted(path) {
            continue;
        }
        added += a.parse().unwrap_or(0.0);
        deleted += d.parse().unwrap_or(0.0);
        files.push(path.to_string());
    }
    Ok((added + deleted, json!({ "added": added, "deleted": deleted, "files": files })))
}

pub fn fixes(cx: &Context, commit: &Commit) -> Result<(f64, Value), String> {
    let landing = !szz::not_a_landing(&commit.subject);
    let task = szz::branch_of(&commit.subject).and_then(|branch| cx.tasks.get(&branch).cloned());
    let named = szz::named_fixes(cx.repo, &commit.id, &commit.parents);
    let reason = if named.is_empty() { szz::fix_reason(&commit.subject, task.as_deref()) } else { Some("a Fixes: trailer".to_string()) };
    let mut doc = json!({ "landing": landing, "subject": commit.subject, "fix": reason });
    let Some(_) = reason else { return Ok((0.0, doc)) };
    let introduced = match named.first() {
        Some(sha) => szz::landing_of(cx.repo, sha, &commit.id).map(|c| (c, 0, vec![])),
        None if commit.parents.is_empty() => None,
        None => szz::introduced(cx.repo, &commit.id).unwrap_or_else(|error| {
            log!("kanna-daemon: couldn't trace the fix {} back: {error}", &commit.id[..commit.id.len().min(8)]);
            None
        }).map(|i| (i.commit, i.lines, i.files)),
    };
    if let Some((blamed, lines, files)) = introduced.filter(|(c, _, _)| *c != commit.id) {
        let shown = crate::tasks::git(cx.repo, &["show", "-s", "--format=%ct%x09%s", &blamed])?;
        let (at, subject) = shown.split_once('\t').unwrap_or(("0", ""));
        doc["blamed"] = json!(blamed);
        doc["blamedAt"] = json!(at.trim().parse::<f64>().unwrap_or(0.0));
        doc["blamedSubject"] = json!(subject.trim());
        doc["lines"] = json!(lines);
        doc["files"] = json!(files);
    }
    Ok((1.0, doc))
}

fn of<'a>(facts: &'a Facts, metric: &str) -> Vec<&'a Sample> {
    let mut rows: Vec<&Sample> = facts.samples.iter().filter(|s| s.metric == metric).collect();
    rows.sort_by(|a, b| a.at.total_cmp(&b.at));
    rows
}

fn daily(rows: &[&Sample], first: i64) -> Vec<Value> {
    (first..first + DAYS)
        .map(|day| rows.iter().rev().find(|s| local_day(s.at) <= day).map_or(Value::Null, |s| json!(s.value)))
        .collect()
}

fn trend(values: &[Value], unit: &str) -> String {
    let known: Vec<f64> = values.iter().filter_map(Value::as_f64).collect();
    let (Some(first), Some(last)) = (known.first(), known.last()) else { return String::new() };
    let change = last - first;
    let size = if unit == "%" { format!("{:.1} points", change.abs()) } else { format!("{}", change.abs().round()) };
    if change.abs() < 0.05 {
        "level over 30 days".to_string()
    } else {
        format!("{} {size} in 30 days", if change > 0.0 { "up" } else { "down" })
    }
}

fn trend_tile(id: &str, label: &str, rows: &[&Sample], first: i64, unit: Option<&str>, what: &str) -> Value {
    let values = daily(rows, first);
    let note = trend(&values, unit.unwrap_or(""));
    let value = rows.last().map_or(Value::Null, |s| json!(s.value));
    let mut chart = json!({ "kind": "line", "values": values, "what": what });
    if unit == Some("%") {
        chart["max"] = json!(100);
    }
    tile(id, label, value, unit, note, 2, chart)
}

fn list(id: &str, label: &str, rows: Vec<Value>, what: &str, empty: &str) -> Value {
    let mut chart = json!({ "kind": "list", "rows": rows, "what": what });
    if chart["rows"].as_array().is_none_or(Vec::is_empty) {
        chart["empty"] = json!(empty);
    }
    tile(id, label, Value::Null, None, String::new(), 3, chart)
}

fn row(label: &str, detail: String, value: f64, figure: String, note: String, open: Value) -> Value {
    json!({ "label": label, "detail": detail, "value": value, "figure": figure, "note": note, "open": open })
}

fn at_line(file: &str, line: u64) -> Value {
    json!({ "path": file, "line": line })
}

fn worst_functions(latest: Option<&Sample>) -> Vec<Value> {
    let Some(sample) = latest else { return vec![] };
    sample.doc["worst"]
        .as_array()
        .into_iter()
        .flatten()
        .take(LISTED)
        .map(|f| {
            let (file, line) = (f["file"].as_str().unwrap_or(""), f["line"].as_u64().unwrap_or(1));
            row(f["name"].as_str().unwrap_or("?"), format!("{file}:{line}"), f["mass"].as_f64().unwrap_or(0.0), format!("CC {}", f["cc"]), format!("{} lines · cognitive {} · calls {}", f["lines"], f["cognitive"], f["calls"]), at_line(file, line))
        })
        .collect()
}

fn duplicated(latest: Option<&Sample>) -> Vec<Value> {
    let Some(sample) = latest else { return vec![] };
    sample.doc["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .take(LISTED)
        .filter_map(|g| {
            let places = g["places"].as_array()?;
            let first = places.first()?;
            let (file, line) = (first["file"].as_str().unwrap_or(""), first["line"].as_u64().unwrap_or(1));
            let count = g["count"].as_u64().unwrap_or(places.len() as u64);
            let lines = g["lines"].as_u64().unwrap_or(0);
            let others: Vec<String> = places.iter().skip(1).take(2).map(|p| format!("{}:{}", p["file"].as_str().unwrap_or("").rsplit('/').next().unwrap_or(""), p["line"])).collect();
            let more = if count as usize > others.len() + 1 { format!(" and {} more", count as usize - others.len() - 1) } else { String::new() };
            Some(row(&format!("{lines} lines × {count}"), format!("{file}:{line}"), (lines * (count - 1)) as f64, format!("{} lines", lines * (count - 1)), format!("also {}{more}", others.join(", ")), at_line(file, line)))
        })
        .collect()
}

fn flagged(latest: Option<&Sample>) -> Vec<Value> {
    let Some(sample) = latest else { return vec![] };
    sample.doc["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .take(LISTED)
        .filter_map(|r| {
            let rule = r["rule"].as_str()?;
            let first = r["places"].as_array()?.first()?;
            let (file, line) = (first["file"].as_str().unwrap_or(""), first["line"].as_u64().unwrap_or(1));
            let (hits, lines) = (r["hits"].as_u64().unwrap_or(0), r["lines"].as_u64().unwrap_or(0));
            let ports = kanna_quality::rules::RULES.iter().find(|(id, _)| *id == rule).map_or("", |(_, scb)| *scb);
            Some(row(rule, format!("{file}:{line}"), lines as f64, format!("{lines} lines"), format!("{hits} place{} · SlopCodeBench's {ports}", if hits == 1 { "" } else { "s" }), at_line(file, line)))
        })
        .collect()
}

fn hotspots(latest: Option<&Sample>, churn: &[&Sample]) -> Vec<Value> {
    let Some(sample) = latest else { return vec![] };
    let mut changes: HashMap<&str, usize> = HashMap::new();
    for s in churn {
        for file in s.doc["files"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            *changes.entry(file).or_default() += 1;
        }
    }
    let mut spots: Vec<(String, usize, u64, f64)> = sample.doc["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let file = f["file"].as_str()?;
            let cognitive = f["cognitive"].as_u64().unwrap_or(0);
            let changed = *changes.get(file)?;
            Some((file.to_string(), changed, cognitive, (changed as u64 * cognitive) as f64))
        })
        .collect();
    spots.sort_by(|a, b| b.3.total_cmp(&a.3).then_with(|| a.0.cmp(&b.0)));
    spots.into_iter().take(LISTED).map(|(file, changed, cognitive, score)| row(file.rsplit('/').next().unwrap_or(&file), file.clone(), score, format!("{changed} × {cognitive}"), format!("changed by {changed} landing{} · cognitive {cognitive}", if changed == 1 { "" } else { "s" }), at_line(&file, 1))).collect()
}

struct Fix<'a> {
    sample: &'a Sample,
    blamed: Option<&'a str>,
}

fn fix_rows(facts: &Facts) -> Vec<Fix<'_>> {
    of(facts, "fixes").into_iter().filter(|s| s.doc["fix"].is_string()).map(|sample| Fix { sample, blamed: sample.doc["blamed"].as_str() }).collect()
}

fn fixes_tiles(facts: &Facts, first: i64) -> Vec<Value> {
    let rows = of(facts, "fixes");
    let landings: Vec<&&Sample> = rows.iter().filter(|s| s.doc["landing"] == json!(true)).collect();
    let fixes = fix_rows(facts);
    let blamed: HashSet<&str> = fixes.iter().filter_map(|f| f.blamed).collect();
    let later_fixed = landings.iter().filter(|s| blamed.contains(s.commit.as_str())).count();
    let rate: Vec<Value> = (first..first + DAYS)
        .map(|day| {
            let near: Vec<&&&Sample> = landings.iter().filter(|s| (day - 6..=day).contains(&local_day(s.at))).collect();
            if near.is_empty() {
                Value::Null
            } else {
                json!((near.iter().filter(|s| blamed.contains(s.commit.as_str())).count() as f64 * 1000.0 / near.len() as f64).round() / 10.0)
            }
        })
        .collect();
    let share = if landings.is_empty() { Value::Null } else { json!((later_fixed as f64 * 1000.0 / landings.len() as f64).round() / 10.0) };
    let mut per_day = vec![0.0; DAYS as usize];
    for fix in &fixes {
        let index = local_day(fix.sample.at) - first;
        if (0..DAYS).contains(&index) {
            per_day[index as usize] += 1.0;
        }
    }
    let points: Vec<Value> = fixes
        .iter()
        .filter_map(|fix| {
            let introduced = fix.sample.doc["blamedAt"].as_f64()?;
            let day = local_day(fix.sample.at) - first;
            (0..DAYS).contains(&day).then(|| json!({ "day": day, "value": (fix.sample.at - introduced).max(0.0).round(), "label": short(&fix.sample.doc["subject"]) }))
        })
        .collect();
    let line: Vec<Value> = (0..DAYS)
        .map(|day| {
            let mut near: Vec<f64> = points.iter().filter(|p| p["day"].as_i64().is_some_and(|d| d > day - 7 && d <= day)).filter_map(|p| p["value"].as_f64()).collect();
            median(&mut near).map_or(Value::Null, |m| json!(m.round()))
        })
        .collect();
    let mut all: Vec<f64> = points.iter().filter_map(|p| p["value"].as_f64()).collect();
    let middle = median(&mut all);
    let total: f64 = per_day.iter().sum();
    vec![
        tile("fixed", "Landings fixed later", share, Some("%"), format!("{later_fixed} of {} landings", landings.len()), 2, json!({ "kind": "line", "values": rate, "max": 100, "what": "of the week's landings were fixed later" })),
        tile("fixes", "Fixes", json!(per_day.last().copied().unwrap_or(0.0)), None, format!("today · {total} in 30 days"), 2, json!({ "kind": "columns", "values": per_day, "what": "fixes landed" })),
        tile("tofix", "Landing to fix", middle.map_or(Value::Null, |m| json!(m.round())), Some("s"), if points.is_empty() { String::new() } else { format!("median of {}", points.len()) }, 2, json!({ "kind": "dots", "points": points, "line": line, "what": "7-day median" })),
    ]
}

fn short(subject: &Value) -> String {
    let text = subject.as_str().unwrap_or("");
    let text = match text.strip_prefix("Merge ") {
        Some(rest) => szz::branch_of(text).unwrap_or_else(|| rest.to_string()),
        None => text.to_string(),
    };
    let mut cut: String = text.chars().take(60).collect();
    if text.chars().count() > 60 {
        cut.push('…');
    }
    cut
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 1 { values[middle] } else { (values[middle - 1] + values[middle]) / 2.0 })
}

fn causes(facts: &Facts) -> Vec<Value> {
    let mut by: BTreeMap<&str, (Vec<&Sample>, &Value)> = BTreeMap::new();
    for fix in fix_rows(facts) {
        if let Some(blamed) = fix.blamed {
            let entry = by.entry(blamed).or_insert((vec![], &fix.sample.doc["blamedSubject"]));
            entry.0.push(fix.sample);
        }
    }
    let mut rows: Vec<(&str, Vec<&Sample>, &Value)> = by.into_iter().map(|(c, (f, s))| (c, f, s)).collect();
    rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));
    rows.into_iter()
        .take(LISTED)
        .map(|(commit, fixes, subject)| {
            let by: Vec<String> = fixes.iter().take(2).map(|f| short(&f.doc["subject"])).collect();
            row(&short(subject), commit[..commit.len().min(8)].to_string(), fixes.len() as f64, format!("{} fix{}", fixes.len(), if fixes.len() == 1 { "" } else { "es" }), format!("fixed by {}", by.join(", ")), json!({ "commit": commit }))
        })
        .collect()
}

fn fixes_by_area(facts: &Facts) -> Vec<Value> {
    let mut areas: BTreeMap<String, usize> = BTreeMap::new();
    for fix in fix_rows(facts) {
        if let Some(file) = fix.sample.doc["files"].as_array().and_then(|f| f.first()).and_then(Value::as_str) {
            *areas.entry(area(file)).or_default() += 1;
        }
    }
    let mut rows: Vec<(String, usize)> = areas.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows.into_iter().take(LISTED).map(|(name, count)| row(&name, String::new(), count as f64, format!("{count}"), String::new(), Value::Null)).collect()
}

fn coverage_tiles(facts: &Facts, first: i64) -> Vec<Value> {
    let rows = of(facts, "coverage");
    let percent = |hit: f64, found: f64| if found > 0.0 { json!((hit * 1000.0 / found).round() / 10.0) } else { Value::Null };
    let series = |key: &str| -> Vec<Value> {
        (first..first + DAYS)
            .map(|day| rows.iter().rev().find(|s| local_day(s.at) <= day).map_or(Value::Null, |s| percent(s.doc[key]["hit"].as_f64().unwrap_or(0.0), s.doc[key]["found"].as_f64().unwrap_or(0.0))))
            .collect()
    };
    let latest = rows.last();
    let status = facts.coverage.as_ref().map(|c| c.to_string());
    let Some(latest) = latest else {
        let note = status.unwrap_or_else(|| "not set up: stats.coverage in ~/.kanna-v4/config.json".to_string());
        return vec![tile("lines-covered", "Lines covered", Value::Null, Some("%"), note, 2, json!({ "kind": "line", "values": vec![Value::Null; DAYS as usize], "max": 100, "what": "of the lines tests ran" }))];
    };
    let of_key = |key: &str| percent(latest.doc[key]["hit"].as_f64().unwrap_or(0.0), latest.doc[key]["found"].as_f64().unwrap_or(0.0));
    let landed: Vec<&Value> = latest.doc["landings"].as_array().into_iter().flatten().filter(|l| l["at"].as_f64().is_some_and(|at| local_day(at) >= first)).collect();
    let (hit, found) = landed.iter().fold((0.0, 0.0), |(h, f), l| (h + l["hit"].as_f64().unwrap_or(0.0), f + l["found"].as_f64().unwrap_or(0.0)));
    let mut changed = vec![Value::Null; DAYS as usize];
    for day in 0..DAYS {
        let that: Vec<&&Value> = landed.iter().filter(|l| l["at"].as_f64().is_some_and(|at| local_day(at) - first == day)).collect();
        let (h, f) = that.iter().fold((0.0, 0.0), |(h, f), l| (h + l["hit"].as_f64().unwrap_or(0.0), f + l["found"].as_f64().unwrap_or(0.0)));
        changed[day as usize] = percent(h, f);
    }
    let ran = status.unwrap_or_else(|| format!("at {}", &latest.commit[..latest.commit.len().min(8)]));
    vec![
        tile("lines-covered", "Lines covered", of_key("lines"), Some("%"), ran, 2, json!({ "kind": "line", "values": series("lines"), "max": 100, "what": "of the lines tests ran" })),
        tile("branches-covered", "Branches covered", of_key("branches"), Some("%"), String::new(), 2, json!({ "kind": "line", "values": series("branches"), "max": 100, "what": "of the branches tests took" })),
        tile("changed-covered", "Changed lines covered", percent(hit, found), Some("%"), format!("of {found} lines landed in 30 days"), 2, json!({ "kind": "columns", "values": changed.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect::<Vec<_>>(), "max": 100, "what": "of the lines landed that day, covered" })),
    ]
}

fn least_covered(facts: &Facts) -> Vec<Value> {
    let rows = of(facts, "coverage");
    let Some(latest) = rows.last() else { return vec![] };
    let mut files: Vec<(&str, f64, f64)> = latest.doc["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| Some((f["file"].as_str()?, f["changedHit"].as_f64().unwrap_or(0.0), f["changedFound"].as_f64().unwrap_or(0.0))))
        .filter(|(_, _, found)| *found > 0.0)
        .collect();
    files.sort_by(|a, b| (b.2 - b.1).total_cmp(&(a.2 - a.1)).then_with(|| a.0.cmp(b.0)));
    files
        .into_iter()
        .take(LISTED)
        .map(|(file, hit, found)| row(file.rsplit('/').next().unwrap_or(file), file.to_string(), found - hit, format!("{}%", ((hit * 100.0 / found).round())), format!("{} of {} changed lines not run", found - hit, found), at_line(file, 1)))
        .collect()
}

pub fn code_section(facts: &Facts) -> Value {
    let first = facts.today - DAYS + 1;
    let erosion = of(facts, "erosion");
    let verbosity = of(facts, "verbosity");
    let churn = of(facts, "churn");
    let note = "how good the code on main is, and its trend";
    if erosion.is_empty() && of(facts, "coverage").is_empty() && facts.sampling.is_none() {
        let mut empty = section("code", "Code quality", note, vec![]);
        empty["empty"] = json!("No Rust or Swift code on main in the last 30 days.");
        return empty;
    }
    let mut tiles = vec![
        trend_tile("erosion", "Erosion", &erosion, first, Some("%"), "of complexity in functions over CC 10"),
        trend_tile("verbosity", "Verbosity", &verbosity, first, Some("%"), "of lines flagged, cloned or in trivial wrappers"),
        trend_tile("high", "Functions over CC 10", &of(facts, "high"), first, None, "functions over CC 10"),
        trend_tile("dead", "Dead functions", &of(facts, "dead"), first, None, "functions nothing outside tests calls"),
        trend_tile("wrappers", "Trivial wrappers", &of(facts, "wrappers"), first, None, "functions whose body only returns one expression"),
        trend_tile("lines", "Code lines", &of(facts, "lines"), first, None, "lines of Rust and Swift outside tests"),
    ];
    tiles.extend(coverage_tiles(facts, first));
    tiles.extend(fixes_tiles(facts, first));
    tiles.push(list("worst", "Worst functions", worst_functions(erosion.last().copied()), "complexity mass: CC × √lines", "No functions yet."));
    tiles.push(list("duplicated", "Most duplicated code", duplicated(verbosity.last().copied()), "lines in copies beyond the first", "No copies of 3 lines or more."));
    tiles.push(list("flagged", "Most flagged patterns", flagged(verbosity.last().copied()), "lines in code a rule would rewrite", "No rule matched."));
    tiles.push(list("hotspots", "Hotspots", hotspots(erosion.last().copied(), &churn), "landings that changed it × its cognitive complexity", "Nothing changed in 30 days."));
    tiles.push(list("uncovered", "Least covered changed files", least_covered(facts), "changed lines tests didn't run", "No coverage run yet."));
    tiles.push(list("causes", "Landings that needed the most fixes", causes(facts), "later fixes traced back to it", "No fix traced back to a landing in 30 days."));
    tiles.push(list("fix-areas", "Fixes by area", fixes_by_area(facts), "fixes whose changed lines were written there", "No fixes in 30 days."));
    let mut code = section("code", "Code quality", note, tiles);
    if let Some(sampling) = &facts.sampling {
        code["note"] = json!(format!("{note} · {sampling}"));
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::tests::{collect, facts, git_in, noon};
    use crate::stats::{day_start, now, report};
    use crate::tasks::CoverageSettings;
    use std::fs;
    use std::path::PathBuf;

    const BUSY: &str = "pub fn busy(a: u8, b: bool) -> u8 {
    if a > 1 && b {
        if a > 2 {
            return 1;
        }
    } else if a > 3 || !b {
        for i in 0..a {
            if i == 4 {
                return 2;
            }
            while b {
                if a == 9 {
                    break;
                }
            }
        }
    }
    match a {
        1 => 1,
        2 => 2,
        3 => 3,
        _ => 4,
    }
}
";

    fn copy(name: &str) -> String {
        format!("pub fn {name}(xs: &[u32]) -> u32 {{\n    let mut total = 0;\n    for x in xs {{\n        total += x * 2;\n    }}\n    total\n}}\n")
    }

    fn fixture(name: &str) -> (PathBuf, String) {
        let repo = std::env::temp_dir().join(format!("kanna-quality-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(repo.join("src")).unwrap();
        let today = local_day(now());
        let at = |day: i64, minute: i64| noon(day) as i64 + minute * 60;
        git_in(&repo, &["init", "-q", "-b", "main"], at(today - 3, 0));
        fs::write(repo.join("src/lib.rs"), "pub fn start() -> u8 {\n    1\n}\n").unwrap();
        git_in(&repo, &["add", "."], at(today - 3, 0));
        git_in(&repo, &["commit", "-q", "-m", "start"], at(today - 3, 0));
        git_in(&repo, &["checkout", "-q", "-b", "feature"], at(today - 2, 0));
        let lib = format!("{BUSY}{}{}pub fn pass(a: u8, b: bool) -> u8 {{\n    busy(a, b)\n}}\nfn unused() -> u8 {{\n    7\n}}\npub fn start() -> u8 {{\n    pass(1, true) + sum(&[1]) as u8 + add_up(&[2]) as u8\n}}\n", copy("sum"), copy("add_up"));
        fs::write(repo.join("src/lib.rs"), lib).unwrap();
        git_in(&repo, &["commit", "-q", "-am", "the feature"], at(today - 2, 0));
        git_in(&repo, &["checkout", "-q", "main"], at(today - 2, 1));
        git_in(&repo, &["merge", "-q", "--no-ff", "-m", "Merge branch 'feature': busy things", "feature"], at(today - 2, 2));
        let landing = String::from_utf8(std::process::Command::new("git").current_dir(&repo).args(["rev-parse", "HEAD"]).output().unwrap().stdout).unwrap().trim().to_string();
        git_in(&repo, &["checkout", "-q", "-b", "fix-busy"], at(today, 0));
        let fixed = fs::read_to_string(repo.join("src/lib.rs")).unwrap().replace("if i == 4 {", "if i == 5 {");
        fs::write(repo.join("src/lib.rs"), fixed).unwrap();
        git_in(&repo, &["commit", "-q", "-am", "busy stops at 5"], at(today, 0));
        git_in(&repo, &["checkout", "-q", "main"], at(today, 1));
        git_in(&repo, &["merge", "-q", "--no-ff", "-m", "Merge branch 'fix-busy': busy stops at 5", "fix-busy"], at(today, 2));
        (repo, landing)
    }

    fn tile_of(report: &kanna_mux::json::Map, id: &str) -> Value {
        report["sections"][3]["tiles"].as_array().unwrap().iter().find(|t| t["id"] == id).cloned().unwrap_or_else(|| panic!("no tile {id}"))
    }

    #[test]
    fn the_code_section_trends_each_measure_and_lists_the_worst() {
        let (repo, landing) = fixture("section");
        let today = local_day(now());
        let samples = collect(&repo, day_start(today - DAYS + 1), &Default::default(), &Default::default());
        let report = report("/r", &facts(today, vec![], samples, vec![]));
        let erosion = tile_of(&report, "erosion");
        assert!(erosion["value"].as_f64().unwrap() > 50.0, "busy holds most of the mass: {erosion}");
        assert_eq!(erosion["unit"], json!("%"));
        assert_eq!(erosion["chart"]["values"].as_array().unwrap().len(), 30);
        assert_eq!(erosion["chart"]["values"][0], Value::Null, "nothing on main 29 days ago");
        assert!(tile_of(&report, "verbosity")["value"].as_f64().unwrap() > 0.0);
        assert_eq!(tile_of(&report, "high")["value"], json!(1.0));
        assert_eq!(tile_of(&report, "dead")["value"], json!(2.0), "unused, and start, which nothing calls");
        assert_eq!(tile_of(&report, "wrappers")["value"], json!(3.0), "pass, unused and start each return one expression");
        let flagged = &tile_of(&report, "flagged")["chart"]["rows"][0];
        assert_eq!(flagged["label"], json!("loop-accumulate"));
        assert_eq!(flagged["note"], json!("2 places · SlopCodeBench's manual-sum-loop, manual-count-loop, manual-str-join"));
        let worst = &tile_of(&report, "worst")["chart"];
        assert_eq!(worst["kind"], json!("list"));
        assert_eq!(worst["rows"][0]["label"], json!("busy"));
        assert_eq!(worst["rows"][0]["open"], json!({ "path": "src/lib.rs", "line": 1 }));
        let duplicated = &tile_of(&report, "duplicated")["chart"]["rows"][0];
        assert_eq!(duplicated["label"], json!("5 lines × 2"));
        let fixed = tile_of(&report, "fixed");
        assert_eq!(fixed["note"], json!("1 of 3 landings"));
        let causes = &tile_of(&report, "causes")["chart"]["rows"];
        assert_eq!(causes[0]["open"], json!({ "commit": landing }));
        assert_eq!(causes[0]["label"], json!("feature"));
        assert_eq!(tile_of(&report, "tofix")["value"], json!(2.0 * 86400.0));
        assert_eq!(tile_of(&report, "hotspots")["chart"]["rows"][0]["detail"], json!("src/lib.rs"));
        assert_eq!(tile_of(&report, "lines-covered")["note"], json!("not set up: stats.coverage in ~/.kanna-v4/config.json"));
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_coverage_run_measures_lines_branches_and_what_each_landing_changed() {
        let (repo, landing) = fixture("coverage");
        let head = crate::tasks::git(&repo, &["rev-parse", "HEAD"]).unwrap();
        let state = std::env::temp_dir().join(format!("kanna-quality-coverage-state-{}", std::process::id()));
        let command = r#"printf 'SF:%s/src/lib.rs\nDA:1,1\nDA:2,1\nDA:3,0\nBRDA:2,0,0,1\nBRDA:2,0,1,0\nend_of_record\nSF:/elsewhere/x.rs\nDA:1,1\nend_of_record\n' "$PWD" > "$KANNA_COVERAGE""#;
        let settings = CoverageSettings { command: command.into(), every: 6, timeout: 60 };
        let sample = crate::coverage::run(&repo, &head, &settings, &state, now()).unwrap();
        assert!(repo.join(crate::coverage::WORKTREE).join("src/lib.rs").exists(), "it ran in the sampler's own worktree");
        assert_eq!(crate::tasks::git(&repo, &["status", "--porcelain"]).unwrap(), "", "the checkout is untouched");
        assert_eq!(sample.value, 66.7);
        assert_eq!(sample.doc["branches"], json!({ "hit": 1, "found": 2 }));
        assert_eq!(sample.doc["landings"].as_array().unwrap().iter().map(|l| l["commit"].as_str().unwrap()).collect::<Vec<_>>(), [landing.as_str()], "the three lines came with the feature");
        let report = report("/r", &facts(local_day(now()), vec![], vec![sample], vec![]));
        assert_eq!(tile_of(&report, "lines-covered")["value"], json!(66.7));
        assert_eq!(tile_of(&report, "branches-covered")["value"], json!(50.0));
        assert_eq!(tile_of(&report, "changed-covered")["value"], json!(66.7));
        assert_eq!(tile_of(&report, "uncovered")["chart"]["rows"][0]["note"], json!("1 of 3 changed lines not run"));
        let home = state.join("home");
        fs::create_dir_all(home.join(".kanna-v4")).unwrap();
        fs::write(home.join(".kanna-v4/config.json"), json!({ "stats": { "coverage": { "~/elsewhere": "true", repo.to_string_lossy(): "\"$KANNA_REPO/scripts/coverage.sh\"" }, "coverageEvery": 2 } }).to_string()).unwrap();
        assert_eq!(crate::tasks::coverage_settings(&home, &repo), Some(CoverageSettings { command: "\"$KANNA_REPO/scripts/coverage.sh\"".into(), every: 2, timeout: 7200 }));
        assert_eq!(crate::tasks::coverage_settings(&home, &home), None, "only the projects you name in your own settings");
        fs::create_dir_all(repo.join(".kanna")).unwrap();
        fs::write(repo.join(".kanna/config.json"), json!({ "stats": { "coverage": { repo.to_string_lossy(): "touch pwned" } } }).to_string()).unwrap();
        assert_eq!(crate::tasks::coverage_settings(&state, &repo), None, "a repo can't name its own coverage command");
        let failing = CoverageSettings { command: "exit 3".into(), every: 6, timeout: 60 };
        assert!(crate::coverage::run(&repo, &head, &failing, &state, now()).unwrap_err().starts_with("it exited 3 without writing $KANNA_COVERAGE"));
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&state);
    }

}
