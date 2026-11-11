use crate::db::{Answer, Round};
use crate::stats::main_ref;
use crate::tasks::git;
use kanna_mux::json::{json, num, Value};
use kanna_quality::git::{tree, Blobs};
use kanna_quality::tree::counted;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const WINDOW_DAYS: f64 = 30.0;

pub const OTHER: &str = "Other";

const FEWEST: usize = 6;

const MOST: usize = 12;

const HALF_LIFE: f64 = 30.0;

const SHRINK: f64 = 2.0;

const SLIPPING_MOVED: usize = 10;

const SLIPPING_MASTERY: u32 = 50;

const OTHER_SHARE: f64 = 0.1;

const SHOWN_COMMITS: usize = 8;

const DAY: f64 = 86400.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Subsystem {
    pub name: String,
    pub paths: Vec<String>,
}

pub struct Commit {
    pub hash: String,
    pub at: f64,
    pub subject: String,
    pub files: Vec<String>,
}

pub struct Facts {
    pub commits: Vec<Commit>,
    pub files: Vec<(String, usize)>,
}

pub fn read_map(doc: &Value) -> Result<Vec<Subsystem>, String> {
    let list = doc.get("subsystems").and_then(Value::as_array).ok_or("a map has \"subsystems\": [{\"name\", \"paths\"}]")?;
    if !(FEWEST..=MOST).contains(&list.len()) {
        return Err(format!("a map has {FEWEST} to {MOST} subsystems, and this has {}", list.len()));
    }
    let mut seen = HashSet::new();
    let mut map = vec![];
    for entry in list {
        let name = entry.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or("each subsystem has a \"name\"")?;
        let paths: Vec<String> = entry.get("paths").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
        if paths.is_empty() {
            return Err(format!("{name} has no \"paths\""));
        }
        if name == OTHER || !seen.insert(name.to_string()) {
            return Err(format!("\"{name}\" is taken: names are unique, and {OTHER} is what matches nothing"));
        }
        map.push(Subsystem { name: name.to_string(), paths });
    }
    Ok(map)
}

fn matched(pattern: &[u8], path: &[u8]) -> bool {
    match pattern {
        [] => path.is_empty(),
        [b'*', b'*', b'/', rest @ ..] => matched(rest, path) || path.iter().enumerate().any(|(at, c)| *c == b'/' && matched(rest, &path[at + 1..])),
        [b'*', b'*', rest @ ..] => (0..=path.len()).any(|at| matched(rest, &path[at..])),
        [b'*', rest @ ..] => (0..=path.len()).take_while(|at| *at == 0 || path[at - 1] != b'/').any(|at| matched(rest, &path[at..])),
        [b'?', rest @ ..] => path.first().is_some_and(|c| *c != b'/') && matched(rest, &path[1..]),
        [c, rest @ ..] => path.first() == Some(c) && matched(rest, &path[1..]),
    }
}

pub fn owner(map: &[Subsystem], path: &str) -> Option<usize> {
    map.iter().position(|subsystem| subsystem.paths.iter().any(|pattern| matched(pattern.as_bytes(), path.as_bytes())))
}

pub fn subsystem_of(map: &[Subsystem], path: &str) -> String {
    owner(map, path).map_or(OTHER.to_string(), |at| map[at].name.clone())
}

pub fn mastery(answers: &[(f64, bool)], now: f64) -> Option<u32> {
    if answers.is_empty() {
        return None;
    }
    let weight = |at: f64| 0.5_f64.powf(((now - at) / DAY).max(0.0) / HALF_LIFE);
    let total: f64 = answers.iter().map(|(at, _)| weight(*at)).sum();
    let right: f64 = answers.iter().filter(|(_, right)| *right).map(|(at, _)| weight(*at)).sum();
    Some((100.0 * right / (total + SHRINK)).round() as u32)
}

pub fn level(mastery: Option<u32>) -> u32 {
    match mastery {
        None => 0,
        Some(0..=39) => 1,
        Some(40..=59) => 2,
        Some(60..=79) => 3,
        Some(_) => 4,
    }
}

pub fn read_facts(repo: &Path, head: &str, since: f64, keep: impl Fn(&str) -> bool) -> Result<Facts, String> {
    let log = git(repo, &["log", "--no-merges", &format!("--since=@{}", since as i64), "--format=%x01%H%x09%ct%x09%s", "--name-only", head])?;
    let mut commits = vec![];
    for chunk in log.split('\u{1}').filter(|c| !c.trim().is_empty()) {
        let mut lines = chunk.lines();
        let mut fields = lines.next().unwrap_or_default().splitn(3, '\t');
        let (Some(hash), Some(at)) = (fields.next(), fields.next().and_then(|a| a.parse().ok())) else { continue };
        commits.push(Commit { hash: hash.to_string(), at, subject: fields.next().unwrap_or_default().to_string(), files: lines.filter(|l| !l.is_empty()).map(String::from).collect() });
    }
    let mut blobs = Blobs::open(repo)?;
    let mut files = vec![];
    for entry in tree(repo, head)? {
        if keep(&entry.path) {
            let text = blobs.read(&entry.blob)?;
            files.push((entry.path, text.lines().count()));
        }
    }
    Ok(Facts { commits, files })
}

pub fn head_of(repo: &Path) -> Result<String, String> {
    git(repo, &["rev-parse", &main_ref(repo)]).map(|head| head.trim().to_string())
}

pub fn before(repo: &Path, commit: &str, since: f64) -> HashSet<String> {
    git(repo, &["rev-list", "--no-merges", &format!("--since=@{}", since as i64), commit]).map(|out| out.lines().map(String::from).collect()).unwrap_or_default()
}

pub fn wanted(map: &[Subsystem], path: &str) -> bool {
    owner(map, path).is_some() || counted(path)
}

pub struct Inputs<'a> {
    pub map: &'a [Subsystem],
    pub facts: &'a Facts,
    pub answers: &'a [Answer],
    pub misses: &'a [Answer],
    pub rounds: &'a [Round],
    pub notes: &'a [(String, String)],
    pub known: &'a HashMap<String, HashSet<String>>,
    pub now: f64,
}

pub fn last_rights(map: &[Subsystem], answers: &[Answer], rounds: &[Round]) -> Vec<String> {
    let heads: HashMap<i64, &str> = rounds.iter().map(|r| (r.id, r.head.as_str())).collect();
    let mut last: HashMap<String, (f64, String)> = HashMap::new();
    for answer in answers.iter().filter(|a| a.right) {
        let Some(head) = heads.get(&answer.round) else { continue };
        let entry = last.entry(subsystem_of(map, &answer.file)).or_insert((f64::MIN, String::new()));
        if answer.at > entry.0 {
            *entry = (answer.at, head.to_string());
        }
    }
    let mut lasts: Vec<String> = last.into_values().map(|(_, head)| head).collect();
    lasts.sort();
    lasts.dedup();
    lasts
}

fn was(answers: &[&Answer], at_round: &HashMap<i64, f64>) -> Option<(u32, f64)> {
    let last = answers.iter().map(|a| a.round).max()?;
    let earlier: Vec<(f64, bool)> = answers.iter().filter(|a| a.round != last).map(|a| (a.at, a.right)).collect();
    let at = at_round.get(&last).copied().unwrap_or(0.0);
    Some((mastery(&earlier, at)?, earlier.iter().map(|(at, _)| *at).fold(0.0, f64::max)))
}

fn tile(name: &str, paths: &[String], lines: usize, inputs: &Inputs) -> Value {
    let mine = |file: &str| subsystem_of(inputs.map, file) == name;
    let answers: Vec<&Answer> = inputs.answers.iter().filter(|a| mine(&a.file)).collect();
    let pairs: Vec<(f64, bool)> = answers.iter().map(|a| (a.at, a.right)).collect();
    let score = mastery(&pairs, inputs.now);
    let heads: HashMap<i64, &str> = inputs.rounds.iter().map(|r| (r.id, r.head.as_str())).collect();
    let at_round: HashMap<i64, f64> = inputs.rounds.iter().map(|r| (r.id, r.at)).collect();
    let last = answers.iter().filter(|a| a.right).max_by(|a, b| a.at.total_cmp(&b.at)).and_then(|a| heads.get(&a.round)).copied();
    let known = last.and_then(|head| inputs.known.get(head));
    let moved: Vec<&Commit> = inputs.facts.commits.iter().filter(|c| c.files.iter().any(|f| mine(f))).filter(|c| !known.is_some_and(|k| k.contains(&c.hash))).collect();
    let misses: Vec<Value> = inputs.misses.iter().filter(|a| mine(&a.file)).map(|a| {
        let line = a.doc.get("line").and_then(Value::as_u64).map_or(String::new(), |l| format!(":{l}"));
        json!({ "answer": format!("a{}", a.id), "text": a.doc.get("topic").or(a.doc.get("question")).and_then(Value::as_str).unwrap_or(&a.kind), "file": format!("{}{line}", a.file) })
    }).collect();
    let shown: Vec<Value> = moved.iter().take(SHOWN_COMMITS).map(|c| json!({ "hash": c.hash.chars().take(7).collect::<String>(), "at": num(c.at), "subject": c.subject })).collect();
    let (was_mastery, was_at) = was(&answers, &at_round).map_or((None, None), |(m, at)| (Some(m), Some(num(at))));
    json!({
        "name": name,
        "paths": paths,
        "lines": lines,
        "mastery": score,
        "level": level(score),
        "answers": answers.len(),
        "asked": answers.iter().map(|a| a.at).reduce(f64::max).map(num),
        "was": was_mastery,
        "wasAt": was_at,
        "moved": moved.len(),
        "slipping": moved.len() >= SLIPPING_MOVED && score.is_some_and(|m| m < SLIPPING_MASTERY),
        "commits": shown,
        "misses": misses,
        "notes": inputs.notes.iter().find(|(subsystem, _)| subsystem == name).map(|(_, text)| text.as_str()).unwrap_or(""),
    })
}

pub fn numbers(inputs: &Inputs) -> Vec<Value> {
    let mut lines = vec![0_usize; inputs.map.len()];
    let mut other = 0;
    for (path, count) in &inputs.facts.files {
        match owner(inputs.map, path) {
            Some(at) => lines[at] += count,
            None => other += count,
        }
    }
    let total: usize = lines.iter().sum::<usize>() + other;
    let mut tiles: Vec<Value> = inputs.map.iter().zip(&lines).map(|(subsystem, lines)| tile(&subsystem.name, &subsystem.paths, *lines, inputs)).collect();
    if total > 0 && other as f64 > OTHER_SHARE * total as f64 {
        tiles.push(tile(OTHER, &[], other, inputs));
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> Vec<Subsystem> {
        read_map(&json!({ "subsystems": [
            { "name": "Machines", "paths": ["crates/kanna-net/**", "docs/specs/machine*.md"] },
            { "name": "Desk", "paths": ["crates/kanna-daemon/src/tasks.rs", "docs/specs/desk*.md"] },
            { "name": "Net again", "paths": ["crates/kanna-net/src/**"] },
            { "name": "D", "paths": ["d/**"] }, { "name": "E", "paths": ["e/**"] }, { "name": "F", "paths": ["f/**"] }] })).unwrap()
    }

    fn answer(id: i64, round: i64, at: f64, file: &str, right: bool) -> Answer {
        Answer { id, round, at, kind: "why".into(), file: file.into(), right, again: None, doc: json!({ "topic": "A topic", "line": 3 }) }
    }

    fn round(id: i64, at: f64, head: &str) -> Round {
        Round { id, repo: "/r".into(), at, head: head.into(), doc: json!({}) }
    }

    #[test]
    fn first_match_wins_and_stars_stop_at_a_folder() {
        let map = map();
        assert_eq!(subsystem_of(&map, "crates/kanna-net/src/lib.rs"), "Machines");
        assert_eq!(subsystem_of(&map, "docs/specs/machine-links.md"), "Machines");
        assert_eq!(subsystem_of(&map, "docs/specs/old/machine-links.md"), OTHER);
        assert_eq!(subsystem_of(&map, "crates/kanna-daemon/src/tasks.rs"), "Desk");
        assert_eq!(subsystem_of(&map, "crates/kanna-daemon/src/tasks.rs.bak"), OTHER);
        assert!(matched(b"**/x.rs", b"x.rs") && matched(b"**/x.rs", b"a/b/x.rs") && !matched(b"**/x.rs", b"ax.rs"));
        assert!(matched(b"a/?.rs", b"a/b.rs") && !matched(b"a/?.rs", b"a//.rs"));
    }

    #[test]
    fn a_map_has_six_to_twelve_unique_named_subsystems() {
        assert!(read_map(&json!({ "subsystems": [{ "name": "A", "paths": ["a"] }] })).unwrap_err().contains("this has 1"));
        let mut doc = json!({ "subsystems": [{ "name": "A", "paths": ["a"] }, { "name": "A", "paths": ["b"] }, { "name": "C", "paths": ["c"] }, { "name": "D", "paths": ["d"] }, { "name": "E", "paths": ["e"] }, { "name": "F", "paths": ["f"] }] });
        assert!(read_map(&doc).unwrap_err().contains("taken"));
        doc["subsystems"][1]["name"] = json!("Other");
        assert!(read_map(&doc).unwrap_err().contains("taken"));
        doc["subsystems"][1] = json!({ "name": "B" });
        assert!(read_map(&doc).unwrap_err().contains("no \"paths\""));
    }

    #[test]
    fn mastery_shrinks_towards_unknown_and_fades_by_half_each_thirty_days() {
        assert_eq!(mastery(&[], 0.0), None);
        assert_eq!(mastery(&[(0.0, true)], 0.0), Some(33), "one lucky answer doesn't read 100%");
        assert_eq!(mastery(&[(0.0, false)], 0.0), Some(0));
        let five: Vec<(f64, bool)> = (0..5).map(|_| (0.0, true)).collect();
        assert_eq!(mastery(&five, 0.0), Some(71));
        let aged = mastery(&five, 30.0 * DAY).unwrap();
        assert_eq!(aged, (100.0 * 2.5 / 4.5_f64).round() as u32, "right answers age out");
        let mixed = [(0.0, false), (30.0 * DAY, true)];
        assert_eq!(mastery(&mixed, 30.0 * DAY), Some((100.0 / 3.5_f64).round() as u32), "the new answer weighs more than the old");
    }

    #[test]
    fn the_shades_are_the_prototype_s() {
        assert_eq!([None, Some(0), Some(39), Some(40), Some(59), Some(60), Some(79), Some(80), Some(100)].map(level), [0, 1, 1, 2, 2, 3, 3, 4, 4]);
    }

    fn commit(hash: &str, file: &str) -> Commit {
        Commit { hash: hash.into(), at: 10.0, subject: format!("touch {file}"), files: vec![file.into()] }
    }

    #[test]
    fn a_subsystem_s_numbers_come_from_its_answers_and_the_commits_since_a_right_one() {
        let map = map();
        let mut commits: Vec<Commit> = (0..12).map(|n| commit(&format!("c{n}"), "crates/kanna-net/a.rs")).collect();
        commits.push(commit("d1", "docs/specs/desk.md"));
        let facts = Facts { commits, files: vec![("crates/kanna-net/a.rs".into(), 90), ("docs/specs/desk.md".into(), 10)] };
        let answers = vec![answer(1, 1, 100.0, "crates/kanna-net/a.rs", true), answer(2, 2, 200.0, "docs/specs/machines.md", false), answer(3, 2, 200.0, "crates/kanna-net/b.rs", false), answer(4, 2, 200.0, "docs/specs/desk.md", false)];
        let misses = vec![answers[2].clone()];
        let rounds = vec![round(1, 100.0, "old"), round(2, 200.0, "new")];
        let known = HashMap::from([("old".to_string(), HashSet::from(["c0".to_string(), "c1".to_string()]))]);
        assert_eq!(last_rights(&map, &answers, &rounds), ["old"]);
        let tiles = numbers(&Inputs { map: &map, facts: &facts, answers: &answers, misses: &misses, rounds: &rounds, notes: &[("Machines".into(), "Strong".into())], known: &known, now: 200.0 });
        assert_eq!(tiles.len(), 6, "nothing in Other");
        let machines = &tiles[0];
        assert_eq!((machines["name"].as_str(), machines["lines"].as_u64(), machines["answers"].as_u64()), (Some("Machines"), Some(90), Some(3)));
        assert_eq!(machines["mastery"], json!(20));
        assert_eq!(machines["level"], json!(1));
        assert_eq!(machines["moved"], json!(10), "the two before the right answer's commit don't count");
        assert_eq!(machines["slipping"], json!(true));
        assert_eq!(machines["notes"], "Strong");
        assert_eq!(machines["commits"].as_array().unwrap().len(), SHOWN_COMMITS);
        assert_eq!(machines["misses"][0], json!({ "answer": "a3", "text": "A topic", "file": "crates/kanna-net/b.rs:3" }));
        assert_eq!(machines["was"], json!(33));
        let desk = &tiles[1];
        assert_eq!((desk["moved"].as_u64(), desk["slipping"].as_bool()), (Some(1), Some(false)), "no right answer yet: the whole window");
        assert_eq!(tiles[3]["mastery"], Value::Null);
    }

    #[test]
    fn other_shows_only_past_a_tenth_of_the_code() {
        let map = map();
        let answers = [];
        let tiles = |code: usize| {
            let facts = Facts { commits: vec![], files: vec![("crates/kanna-net/a.rs".into(), 90), ("apps/a.swift".into(), code)] };
            numbers(&Inputs { map: &map, facts: &facts, answers: &answers, misses: &[], rounds: &[], notes: &[], known: &HashMap::new(), now: 0.0 })
        };
        assert_eq!(tiles(10).len(), 6);
        let shown = tiles(11);
        assert_eq!(shown[6]["name"], OTHER);
    }
}
