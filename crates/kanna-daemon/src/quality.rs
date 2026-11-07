use crate::db::Sample;
use crate::stats::{local_day, section, tile, Commit, Context, Facts, DAYS};
use kanna_mux::json::{json, Value};
use kanna_quality::szz;
use kanna_quality::tree::{counted, Ranked, Tree, HIGH_COMPLEXITY};
use std::collections::{BTreeMap, HashMap};

const WORST: usize = 10;

const FILES: usize = 40;

const LISTED: usize = 8;

const SETTLED: f64 = 10.0;

const CC_TIP: &str = "CC is cyclomatic complexity: 1 plus each place a function can branch (each if, loop, match arm, && and ||), the number of paths through it a test would need. Over 10 is SlopCodeBench's line for a function too tangled to follow or test easily.";

fn tipped(mut tile: Value, tip: &str) -> Value {
    tile["tip"] = json!(tip);
    tile
}

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

fn of_functions(count: usize, tree: &Tree) -> f64 {
    let all = tree.functions().len();
    share((all > 0).then(|| count as f64 / all as f64))
}

pub fn high(tree: &Tree) -> (f64, Value) {
    (of_functions(tree.high(), tree), json!({ "count": tree.high(), "of": tree.functions().len() }))
}

pub fn dead(tree: &Tree) -> (f64, Value) {
    let dead = tree.dead();
    (of_functions(dead.len(), tree), json!({ "count": dead.len(), "of": tree.functions().len(), "functions": dead.iter().take(WORST).map(function_row).collect::<Vec<_>>() }))
}

pub fn wrappers(tree: &Tree) -> (f64, Value) {
    let wrappers = tree.wrappers();
    (of_functions(wrappers.len(), tree), json!({ "count": wrappers.len(), "of": tree.functions().len(), "functions": wrappers.iter().take(WORST).map(function_row).collect::<Vec<_>>() }))
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

fn settled(lines: &[&Sample], first: i64) -> usize {
    let latest = lines.last().map_or(0.0, |s| s.value);
    daily(lines, first).iter().position(|v| v.as_f64().is_some_and(|v| v >= latest / SETTLED)).unwrap_or(0)
}

fn trend(values: &[Value], unit: &str, days: usize) -> String {
    let known: Vec<f64> = values.iter().filter_map(Value::as_f64).collect();
    let (Some(first), Some(last)) = (known.first(), known.last()) else { return String::new() };
    let change = last - first;
    let size = if unit == "%" { format!("{:.1} points", change.abs()) } else { format!("{}", change.abs().round()) };
    if change.abs() < 0.05 {
        format!("level over {days} days")
    } else {
        format!("{} {size} in {days} days", if change > 0.0 { "up" } else { "down" })
    }
}

fn trend_tile(id: &str, label: &str, rows: &[&Sample], first: i64, unit: Option<&str>, what: &str, from: usize) -> Value {
    let mut values = daily(rows, first);
    values[..from].fill(Value::Null);
    let note = trend(&values, unit.unwrap_or(""), DAYS as usize - from);
    let value = rows.last().map_or(Value::Null, |s| json!(s.value));
    let mut chart = json!({ "kind": "line", "values": values, "what": what });
    if unit == Some("%") {
        chart["max"] = json!(100);
    }
    tile(id, label, value, unit, note, 2, chart)
}

fn share_tile(id: &str, label: &str, rows: &[&Sample], first: i64, what: &str, from: usize) -> Value {
    let mut tile = trend_tile(id, label, rows, first, Some("%"), what, from);
    if let Some(chart) = tile["chart"].as_object_mut() {
        chart.remove("max");
    }
    if let Some(last) = rows.last() {
        let trend = tile["note"].as_str().unwrap_or("").to_string();
        tile["note"] = json!(format!("{} of {} · {trend}", last.doc["count"], last.doc["of"]));
    }
    tile
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
    if erosion.is_empty() && of(facts, "coverage").is_empty() {
        let mut empty = section("code", "Code quality", note, vec![]);
        empty["empty"] = json!(if facts.pending { "Measuring the code on main." } else { "No Rust or Swift code on main in the last 30 days." });
        return empty;
    }
    let from = settled(&of(facts, "lines"), first);
    let mut tiles = vec![
        tipped(trend_tile("erosion", "Erosion", &erosion, first, Some("%"), "of complexity in functions over CC 10", from), CC_TIP),
        trend_tile("verbosity", "Verbosity", &verbosity, first, Some("%"), "of lines flagged, cloned or in trivial wrappers", from),
        tipped(share_tile("high", "Functions over CC 10", &of(facts, "high"), first, "of functions over CC 10", from), CC_TIP),
        share_tile("dead", "Dead functions", &of(facts, "dead"), first, "of functions nothing outside tests calls", from),
        share_tile("wrappers", "Trivial wrappers", &of(facts, "wrappers"), first, "of functions whose body only returns one expression", from),
        trend_tile("lines", "Code lines", &of(facts, "lines"), first, None, "lines of Rust and Swift outside tests", 0),
    ];
    tiles.extend(coverage_tiles(facts, first));
    tiles.push(tipped(list("worst", "Worst functions", worst_functions(erosion.last().copied()), "complexity mass: CC × √lines", "No functions yet."), CC_TIP));
    tiles.push(list("duplicated", "Most duplicated code", duplicated(verbosity.last().copied()), "lines in copies beyond the first", "No copies of 3 lines or more."));
    tiles.push(list("flagged", "Most flagged patterns", flagged(verbosity.last().copied()), "lines in code a rule would rewrite", "No rule matched."));
    tiles.push(list("hotspots", "Hotspots", hotspots(erosion.last().copied(), &churn), "landings that changed it × its cognitive complexity", "Nothing changed in 30 days."));
    tiles.push(list("uncovered", "Least covered changed files", least_covered(facts), "changed lines tests didn't run", "No coverage run yet."));
    section("code", "Code quality", note, tiles)
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
        report["sections"][4]["tiles"].as_array().unwrap().iter().find(|t| t["id"] == id).cloned().unwrap_or_else(|| panic!("no tile {id}"))
    }

    #[test]
    fn a_share_s_trend_starts_once_main_has_a_tenth_of_today_s_code() {
        let sample = |at: f64, value: f64| Sample { commit: String::new(), metric: "lines".into(), at, value, doc: json!({}), revision: 1 };
        let today = local_day(now());
        let first = today - DAYS + 1;
        let lines = [sample(noon(first + 2), 96.0), sample(noon(first + 3), 9765.0), sample(noon(today), 55000.0)];
        let rows: Vec<&Sample> = lines.iter().collect();
        assert_eq!(settled(&rows, first), 3);
        let shares = [sample(noon(first + 2), 40.0), sample(noon(first + 3), 6.9), sample(noon(today), 8.8)];
        let tile = trend_tile("high", "High", &shares.iter().collect::<Vec<_>>(), first, Some("%"), "what", 3);
        assert_eq!(tile["note"], json!("up 1.9 points in 27 days"), "not down 31.2 from the day main had five functions");
        assert_eq!(tile["chart"]["values"][2], Value::Null);
        assert_eq!(tile["chart"]["values"][3], json!(6.9));
    }

    #[test]
    fn the_code_section_trends_each_measure_and_lists_the_worst() {
        let (repo, landing) = fixture("section");
        let today = local_day(now());
        let samples = collect(&repo, day_start(today - DAYS + 1), &Default::default(), &Default::default());
        let report = report("/r", &facts(today, vec![], samples.clone(), vec![]));
        let erosion = tile_of(&report, "erosion");
        assert!(erosion["value"].as_f64().unwrap() > 50.0, "busy holds most of the mass: {erosion}");
        assert_eq!(erosion["unit"], json!("%"));
        assert_eq!(erosion["chart"]["values"].as_array().unwrap().len(), 30);
        assert_eq!(erosion["chart"]["values"][0], Value::Null, "nothing on main 29 days ago");
        assert!(tile_of(&report, "verbosity")["value"].as_f64().unwrap() > 0.0);
        let high = tile_of(&report, "high");
        assert_eq!((high["value"].clone(), high["unit"].clone()), (json!(16.7), json!("%")), "one of six functions");
        assert!(high["note"].as_str().unwrap().starts_with("1 of 6 · "), "{high}");
        assert_eq!(high["chart"]["max"], Value::Null, "a share this small needs its own scale");
        assert!(tile_of(&report, "high")["tip"].as_str().unwrap().starts_with("CC is cyclomatic complexity"));
        assert_eq!(tile_of(&report, "worst")["tip"], tile_of(&report, "erosion")["tip"]);
        assert_eq!(tile_of(&report, "verbosity")["tip"], Value::Null);
        assert_eq!(tile_of(&report, "dead")["value"], json!(33.3), "unused, and start, which nothing calls, of six functions");
        let wrappers = tile_of(&report, "wrappers");
        assert_eq!(wrappers["value"], json!(50.0), "pass, unused and start each return one expression, of six functions");
        assert!(wrappers["note"].as_str().unwrap().starts_with("3 of 6 · "), "{wrappers}");
        let flagged = &tile_of(&report, "flagged")["chart"]["rows"][0];
        assert_eq!(flagged["label"], json!("loop-accumulate"));
        assert_eq!(flagged["note"], json!("2 places · SlopCodeBench's manual-sum-loop, manual-count-loop, manual-str-join"));
        let worst = &tile_of(&report, "worst")["chart"];
        assert_eq!(worst["kind"], json!("list"));
        assert_eq!(worst["rows"][0]["label"], json!("busy"));
        assert_eq!(worst["rows"][0]["open"], json!({ "path": "src/lib.rs", "line": 1 }));
        let duplicated = &tile_of(&report, "duplicated")["chart"]["rows"][0];
        assert_eq!(duplicated["label"], json!("5 lines × 2"));
        let fix = samples.iter().filter(|s| s.metric == "fixes" && s.value == 1.0).collect::<Vec<_>>();
        assert_eq!(fix.len(), 1, "the fix-busy landing, sampled for the factory's stats though Code quality doesn't show it");
        assert_eq!(fix[0].doc["blamed"], json!(landing));
        let ids: Vec<&str> = report["sections"][4]["tiles"].as_array().unwrap().iter().filter_map(|t| t["id"].as_str()).collect();
        assert!(!ids.iter().any(|id| ["fixed", "fixes", "tofix", "causes", "fix-areas"].contains(id)), "fixes are the factory's, not the code's: {ids:?}");
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
