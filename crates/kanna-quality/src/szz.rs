use crate::git::git;
use crate::tree::counted;
use std::collections::BTreeMap;
use std::path::Path;

const FIX_WORDS: &[&str] = &[
    "fix", "fixes", "fixed", "fixing", "bug", "bugs", "broken", "breaks", "crash", "crashes", "crashed", "regression", "wrong", "fails", "failing", "failed", "stall", "stalls", "stalled", "stuck", "hang", "hangs", "leak", "leaks", "lose", "loses", "lost", "race", "flaky", "doesn't", "don't", "isn't", "nothing",
];

pub fn branch_of(subject: &str) -> Option<String> {
    let rest = subject.strip_prefix("Merge ")?;
    let rest = rest.strip_prefix("branch ").unwrap_or(rest);
    let name = if let Some(quoted) = rest.strip_prefix('\'') { quoted.split('\'').next()? } else { rest.split([':', ';', ' ']).next()? };
    (!name.is_empty() && name != "main" && name != "master" && !name.contains("://")).then(|| name.to_string())
}

pub fn first_sentence(text: &str) -> &str {
    let text = text.trim();
    let end = text.find(". ").map(|at| at + 1).or_else(|| text.find('\n')).unwrap_or(text.len()).min(text.len());
    let mut end = end.min(300);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub fn fix_word(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase().replace('’', "'");
    let words: Vec<&str> = lower.split(|c: char| !(c.is_alphanumeric() || c == '\'')).filter(|w| !w.is_empty()).collect();
    FIX_WORDS.iter().find(|word| words.contains(word)).copied()
}

pub fn is_trivial(line: &str) -> bool {
    let code = line.trim();
    code.is_empty() || code.starts_with("//") || code.starts_with("/*") || code.starts_with('*') || code.chars().all(|c| "{}()[];,".contains(c) || c.is_whitespace())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Blamed {
    pub commit: String,
    pub lines: usize,
}

pub fn removed_lines(repo: &Path, parent: &str, fix: &str) -> Result<BTreeMap<String, Vec<(usize, usize)>>, String> {
    let diff = git(repo, &["diff", "-w", "-U0", "--no-color", "--no-ext-diff", "--no-renames", parent, fix])?;
    let mut removed: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    let mut file: Option<String> = None;
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("--- ") {
            file = path.strip_prefix("a/").filter(|p| counted(p)).map(String::from);
        } else if let (Some(hunk), Some(path)) = (line.strip_prefix("@@ -"), file.as_ref()) {
            let old = hunk.split_whitespace().next().unwrap_or("");
            let (start, count) = match old.split_once(',') {
                Some((start, count)) => (start.parse().unwrap_or(0), count.parse().unwrap_or(0)),
                None => (old.parse().unwrap_or(0), 1),
            };
            if count > 0 && start > 0 {
                removed.entry(path.clone()).or_default().push((start, count));
            }
        }
    }
    Ok(removed)
}

pub fn blame(repo: &Path, parent: &str, removed: &BTreeMap<String, Vec<(usize, usize)>>) -> Result<Vec<Blamed>, String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (path, ranges) in removed {
        let mut args: Vec<String> = vec!["blame".into(), "-w".into(), "--first-parent".into(), "--line-porcelain".into()];
        for (start, count) in ranges {
            args.push("-L".into());
            args.push(format!("{start},+{count}"));
        }
        args.push(parent.into());
        args.push("--".into());
        args.push(path.clone());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let Ok(out) = git(repo, &refs) else { continue };
        let mut commit = String::new();
        for line in out.lines() {
            if let Some(text) = line.strip_prefix('\t') {
                if !is_trivial(text) && !commit.starts_with('^') {
                    *counts.entry(commit.clone()).or_default() += 1;
                }
            } else if line.len() >= 40 && line.as_bytes()[..40].iter().all(u8::is_ascii_hexdigit) {
                commit = line[..40].to_string();
            } else if line == "boundary" {
                commit = format!("^{commit}");
            }
        }
    }
    let mut blamed: Vec<Blamed> = counts.into_iter().map(|(commit, lines)| Blamed { commit, lines }).collect();
    blamed.sort_by(|a, b| b.lines.cmp(&a.lines).then_with(|| a.commit.cmp(&b.commit)));
    Ok(blamed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes_are_found_by_their_words_and_merges_by_their_branch() {
        assert_eq!(branch_of("Merge branch 'stats-tab': the Desk view's Stats tab"), Some("stats-tab".into()));
        assert_eq!(branch_of("Merge cloud-provisioning: Kanna Cloud"), Some("cloud-provisioning".into()));
        assert_eq!(branch_of("Merge main into task-transfer"), None);
        assert_eq!(branch_of("Transfer: no tab moves"), None);
        assert_eq!(fix_word("Paired daemons lose each other for minutes"), Some("lose"));
        assert_eq!(fix_word("Don’t resume another conversation"), Some("don't"));
        assert_eq!(fix_word("Status dots on Shelf rows"), None);
        assert_eq!(fix_word("Prefix the suffix"), None);
        assert_eq!(first_sentence("Markdown tables draw wrong while scrolling: rows are clipped. Zooming fixes it."), "Markdown tables draw wrong while scrolling: rows are clipped.");
        assert!(is_trivial("    }"));
        assert!(is_trivial("// note"));
        assert!(!is_trivial("x += 1;"));
    }

    fn run(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git").current_dir(repo).args(args).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn a_fix_blames_the_landing_that_wrote_the_lines_it_changed() {
        let repo = std::env::temp_dir().join(format!("kanna-quality-szz-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(repo.join("src")).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("src/lib.rs"), "fn a() -> u8 {\n    1\n}\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "first"]);
        run(&repo, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.join("src/lib.rs"), "fn a() -> u8 {\n    1\n}\nfn b(x: u8) -> u8 {\n    x + 2\n}\n").unwrap();
        run(&repo, &["commit", "-q", "-am", "b"]);
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["merge", "-q", "--no-ff", "-m", "Merge branch 'feature': b", "feature"]);
        let landing = run(&repo, &["rev-parse", "HEAD"]);
        run(&repo, &["checkout", "-q", "-b", "fix-b"]);
        std::fs::write(repo.join("src/lib.rs"), "fn a() -> u8 {\n    1\n}\nfn b(x: u8) -> u8 {\n    x + 3\n}\n").unwrap();
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("tests/t.rs"), "fn t() {}\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "b adds 3"]);
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["merge", "-q", "--no-ff", "-m", "Merge branch 'fix-b': b is wrong", "fix-b"]);
        let fix = run(&repo, &["rev-parse", "HEAD"]);
        let removed = removed_lines(&repo, &format!("{fix}^1"), &fix).unwrap();
        assert_eq!(removed, BTreeMap::from([("src/lib.rs".to_string(), vec![(5, 1)])]));
        assert_eq!(blame(&repo, &format!("{fix}^1"), &removed).unwrap(), vec![Blamed { commit: landing, lines: 1 }]);
        let _ = std::fs::remove_dir_all(&repo);
    }
}
