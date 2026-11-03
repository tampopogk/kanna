use kanna_quality::{git::git, szz};
use std::path::Path;
fn main() {
    let repo = Path::new(".");
    let t = std::time::Instant::now();
    let log = git(repo, &["log", "--first-parent", "--format=%H%x09%ct%x09%s", "main"]).unwrap();
    let rows: Vec<Vec<&str>> = log.lines().map(|l| l.splitn(3, '\t').collect()).collect();
    let at: std::collections::HashMap<&str, (i64, &str)> = rows.iter().map(|p| (p[0], (p[1].parse().unwrap(), p[2]))).collect();
    let (mut fixes, mut blamed) = (0, std::collections::BTreeMap::<&str, usize>::new());
    let mut delays = vec![];
    for p in &rows {
        let Some(reason) = szz::fix_reason(p[2], None) else { continue };
        fixes += 1;
        let found = szz::introduced(repo, p[0]).unwrap();
        println!("FIX [{reason}] {} → {}", p[2].chars().take(80).collect::<String>(), found.as_ref().map(|i| format!("{} ({} lines) {}", &i.commit[..8], i.lines, at.get(i.commit.as_str()).map(|x| x.1.chars().take(50).collect::<String>()).unwrap_or_default())).unwrap_or("-".into()));
        if let Some(i) = found {
            let key = at.get_key_value(i.commit.as_str()).map(|(k, _)| *k).unwrap_or("?");
            *blamed.entry(key).or_default() += 1;
            if let Some((t0, _)) = at.get(i.commit.as_str()) { delays.push((p[1].parse::<i64>().unwrap() - t0) as f64 / 3600.0); }
        }
    }
    delays.sort_by(f64::total_cmp);
    println!("fixes {fixes} of {} landings; {} blamed landings; median {:.1} h; in {:?}", rows.len(), blamed.len(), delays.get(delays.len() / 2).copied().unwrap_or(0.0), t.elapsed());
}
