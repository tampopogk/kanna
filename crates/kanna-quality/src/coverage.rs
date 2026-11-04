use crate::git::git;
use crate::tree::counted;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

#[derive(Default, Debug, PartialEq)]
pub struct FileCoverage {
    pub lines: BTreeMap<usize, u64>,
    pub branches: BTreeMap<(usize, String), u64>,
}

#[derive(Default, Debug, PartialEq, Clone, Copy)]
pub struct Count {
    pub hit: usize,
    pub found: usize,
}

impl Count {
    fn add(&mut self, hit: bool) {
        self.found += 1;
        self.hit += usize::from(hit);
    }
}

pub fn parse(text: &str, root: &Path) -> BTreeMap<String, FileCoverage> {
    let mut files: BTreeMap<String, FileCoverage> = BTreeMap::new();
    let roots: Vec<String> = [Some(root.to_path_buf()), root.canonicalize().ok()].into_iter().flatten().map(|r| r.to_string_lossy().trim_end_matches('/').to_string()).collect();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(path) = line.strip_prefix("SF:") {
            let relative = roots.iter().find_map(|root| path.strip_prefix(root.as_str()).filter(|rest| rest.starts_with('/'))).map(|p| p.trim_start_matches('/')).unwrap_or(path);
            current = (!relative.starts_with('/') && counted(relative)).then(|| relative.to_string());
        } else if line == "end_of_record" {
            current = None;
        } else if let Some(file) = current.as_ref() {
            if let Some(data) = line.strip_prefix("DA:") {
                let mut parts = data.split(',');
                let (Some(number), Some(count)) = (parts.next().and_then(|n| n.parse().ok()), parts.next().and_then(|c| c.parse::<f64>().ok())) else { continue };
                *files.entry(file.clone()).or_default().lines.entry(number).or_default() += count.max(0.0) as u64;
            } else if let Some(data) = line.strip_prefix("BRDA:") {
                let parts: Vec<&str> = data.split(',').collect();
                let [number, block, branch, taken] = parts.as_slice() else { continue };
                let Ok(number) = number.parse() else { continue };
                let taken = taken.parse::<u64>().unwrap_or(0);
                *files.entry(file.clone()).or_default().branches.entry((number, format!("{block},{branch}"))).or_default() += taken;
            }
        }
    }
    files
}

pub fn totals(files: &BTreeMap<String, FileCoverage>) -> (Count, Count) {
    let (mut lines, mut branches) = (Count::default(), Count::default());
    for file in files.values() {
        file.lines.values().for_each(|c| lines.add(*c > 0));
        file.branches.values().for_each(|c| branches.add(*c > 0));
    }
    (lines, branches)
}

pub fn landed_lines(repo: &Path, commit: &str, file: &str, since: f64) -> Result<BTreeMap<usize, String>, String> {
    let out = git(repo, &["blame", "-w", "--first-parent", "--line-porcelain", &format!("--since=@{}", since as i64), commit, "--", file])?;
    let mut lines = BTreeMap::new();
    let (mut current, mut number, mut boundary) = (String::new(), 0usize, false);
    for line in out.lines() {
        if line.starts_with('\t') {
            if !boundary {
                lines.insert(number, current.clone());
            }
            boundary = false;
        } else if line == "boundary" {
            boundary = true;
        } else if line.len() > 41 && line.as_bytes()[..40].iter().all(u8::is_ascii_hexdigit) {
            let mut words = line.split_whitespace();
            current = words.next().unwrap_or("").to_string();
            number = words.nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
        }
    }
    Ok(lines)
}

pub struct Changed {
    pub files: BTreeMap<String, Count>,
    pub landings: HashMap<String, Count>,
}

pub fn changed(repo: &Path, commit: &str, files: &BTreeMap<String, FileCoverage>, since: f64) -> Changed {
    let mut changed = Changed { files: BTreeMap::new(), landings: HashMap::new() };
    for (file, coverage) in files {
        let Ok(landed) = landed_lines(repo, commit, file, since) else { continue };
        for (number, count) in &coverage.lines {
            let Some(landing) = landed.get(number) else { continue };
            changed.files.entry(file.clone()).or_default().add(*count > 0);
            changed.landings.entry(landing.clone()).or_default().add(*count > 0);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcov_lines_and_branches_merge_across_records_and_keep_only_counted_files() {
        let text = "SF:/w/crates/a/src/lib.rs\nDA:1,3\nDA:2,0\nBRDA:2,0,0,1\nBRDA:2,0,1,-\nend_of_record\nSF:/w/crates/a/src/lib.rs\nDA:2,1\nend_of_record\nSF:/w/crates/a/tests/t.rs\nDA:1,1\nend_of_record\nSF:/Users/me/.cargo/registry/x.rs\nDA:1,1\nend_of_record\n";
        let files = parse(text, Path::new("/w/"));
        assert_eq!(files.keys().collect::<Vec<_>>(), ["crates/a/src/lib.rs"]);
        let (lines, branches) = totals(&files);
        assert_eq!((lines.hit, lines.found), (2, 2));
        assert_eq!((branches.hit, branches.found), (1, 2));
    }

    fn run(repo: &Path, args: &[&str], at: i64) -> String {
        let date = format!("@{at} +0000");
        let out = std::process::Command::new("git").current_dir(repo).args(args).env("GIT_AUTHOR_DATE", &date).env("GIT_COMMITTER_DATE", &date).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn changed_lines_are_the_ones_landings_wrote_since_the_window_began() {
        let repo = std::env::temp_dir().join(format!("kanna-quality-coverage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let now = 1_790_000_000;
        run(&repo, &["init", "-q", "-b", "main"], now - 90 * 86400);
        std::fs::write(repo.join("src/lib.rs"), "fn a() {\n    old();\n}\n").unwrap();
        run(&repo, &["add", "."], now - 90 * 86400);
        run(&repo, &["commit", "-q", "-m", "old"], now - 90 * 86400);
        std::fs::write(repo.join("src/lib.rs"), "fn a() {\n    old();\n}\nfn b() {\n    new();\n}\n").unwrap();
        run(&repo, &["commit", "-q", "-am", "new"], now - 86400);
        let head = run(&repo, &["rev-parse", "HEAD"], now);
        let files = parse("SF:src/lib.rs\nDA:1,1\nDA:2,1\nDA:4,1\nDA:5,0\nend_of_record\n", &repo);
        let changed = changed(&repo, &head, &files, (now - 30 * 86400) as f64);
        assert_eq!(changed.files["src/lib.rs"], Count { hit: 1, found: 2 });
        assert_eq!(changed.landings[&head], Count { hit: 1, found: 2 });
        assert_eq!(changed.landings.len(), 1);
        let _ = std::fs::remove_dir_all(&repo);
    }
}
