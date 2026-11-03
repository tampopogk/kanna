use kanna_quality::{git::git, szz};
use std::path::Path;
fn main() {
    let repo = Path::new(".");
    let t = std::time::Instant::now();
    let log = git(repo, &["log", "--first-parent", "--format=%H%x09%ct%x09%s", "main"]).unwrap();
    let mut fixes = 0; let mut blamed_total = std::collections::BTreeMap::<String, usize>::new();
    let at: std::collections::HashMap<String, (i64, String)> = log.lines().map(|l| { let p: Vec<&str> = l.splitn(3, '\t').collect(); (p[0].to_string(), (p[1].parse().unwrap(), p[2].chars().take(70).collect())) }).collect();
    let mut delays = vec![];
    for line in log.lines() {
        let p: Vec<&str> = line.splitn(3, '\t').collect();
        let Some(w) = szz::fix_word(szz::first_sentence(p[2])) else { continue };
        fixes += 1;
        let parent = format!("{}^1", p[0]);
        let removed = szz::removed_lines(repo, &parent, p[0]).unwrap();
        let blamed = szz::blame(repo, &parent, &removed).unwrap();
        println!("FIX [{w}] {} → {}", &p[2].chars().take(90).collect::<String>(), blamed.iter().map(|b| format!("{}({})", &b.commit[..8], b.lines)).collect::<Vec<_>>().join(" "));
        for b in &blamed { *blamed_total.entry(b.commit.clone()).or_default() += 1; if let Some((t0, _)) = at.get(&b.commit) { delays.push((p[1].parse::<i64>().unwrap() - t0) as f64 / 3600.0); } }
    }
    println!("fixes {fixes} landings {} blamed landings {} in {:?}", log.lines().count(), blamed_total.len(), t.elapsed());
    delays.sort_by(f64::total_cmp); if !delays.is_empty() { println!("median hours to fix {:.1}", delays[delays.len()/2]); }
    let mut top: Vec<_> = blamed_total.into_iter().collect(); top.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (c, n) in top.iter().take(8) { println!("{n} {} {}", &c[..8], at.get(c).map(|x| x.1.clone()).unwrap_or_default()); }
}
