use crate::git::git;
use crate::tree::counted;
use std::collections::BTreeMap;
use std::path::Path;

const FIX_WORDS: &[&str] = &["fix", "fixes", "fixed", "bug", "bugs", "regression", "crash", "crashes", "crashed", "broken", "wrong", "revert"];

const PROBLEM_WORDS: &[&str] = &["fails", "failing", "failed", "stall", "stalls", "stalled", "stuck", "hang", "hangs", "leak", "leaks", "lose", "loses", "race", "flaky", "doesn't", "don't", "isn't", "aren't", "nothing"];

pub fn not_a_landing(subject: &str) -> bool {
    ["Merge main into", "Merge branch 'main'", "Merge branch 'master'", "Merge remote-tracking branch"].iter().any(|p| subject.starts_with(p))
}

pub fn fix_reason(subject: &str, task: Option<&str>) -> Option<String> {
    if not_a_landing(subject) {
        return None;
    }
    if let Some(branch) = branch_of(subject).filter(|b| ["fix-", "bug-", "hotfix-"].iter().any(|p| b.starts_with(p))) {
        return Some(format!("branch {branch}"));
    }
    let clause = first_clause(subject);
    if let Some(word) = word_in(clause, FIX_WORDS) {
        return Some(format!("“{word}” in the commit"));
    }
    let task = task?;
    word_in(task, FIX_WORDS).or_else(|| word_in(task, PROBLEM_WORDS)).map(|word| format!("“{word}” in the task"))
}

fn first_clause(subject: &str) -> &str {
    let rest = match subject.strip_prefix("Merge ") {
        Some(rest) => rest.split_once(": ").map_or(rest, |(_, after)| after),
        None => subject,
    };
    let end = rest.find(['.', ';']).unwrap_or(rest.len()).min(160);
    let mut end = end.min(rest.len());
    while !rest.is_char_boundary(end) {
        end -= 1;
    }
    &rest[..end]
}

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

pub fn word_in(text: &str, list: &[&'static str]) -> Option<&'static str> {
    let lower = text.to_lowercase().replace('’', "'");
    let words: Vec<&str> = lower.split(|c: char| !(c.is_alphanumeric() || c == '\'')).filter(|w| !w.is_empty()).collect();
    list.iter().find(|word| words.contains(word)).copied()
}

pub fn named_fixes(repo: &Path, landing: &str, parents: &[String]) -> Vec<String> {
    let range = match parents.first() {
        Some(first) if parents.len() > 1 => format!("{first}..{landing}"),
        _ => format!("{landing}^!"),
    };
    let Ok(bodies) = git(repo, &["log", "--invert-grep", "-F", "--grep=(cherry picked from commit ", "--format=%(trailers:key=Fixes,valueonly)", &range]) else { return vec![] };
    bodies.lines().filter_map(|line| line.split_whitespace().next()).filter(|sha| sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit())).map(String::from).collect()
}

pub fn landing_of(repo: &Path, commit: &str, upto: &str) -> Option<String> {
    let full = git(repo, &["rev-parse", "--verify", "--quiet", &format!("{commit}^{{commit}}")]).ok()?.trim().to_string();
    let first_parents = git(repo, &["rev-list", "--first-parent", upto]).ok()?;
    let chain: Vec<&str> = first_parents.lines().collect();
    if chain.contains(&full.as_str()) {
        return Some(full);
    }
    chain.iter().rev().find(|landing| git(repo, &["merge-base", "--is-ancestor", &full, landing]).is_ok()).map(|l| l.to_string())
}

pub struct Introduced {
    pub commit: String,
    pub lines: usize,
    pub files: Vec<String>,
}

pub fn introduced(repo: &Path, fix: &str) -> Result<Option<Introduced>, String> {
    let parent = format!("{fix}^1");
    let removed = removed_lines(repo, &parent, fix)?;
    let blamed = blame(repo, &parent, &removed)?;
    let mut latest: Option<(i64, &Blamed)> = None;
    for candidate in &blamed {
        let at: i64 = git(repo, &["show", "-s", "--format=%ct", &candidate.commit])?.trim().parse().unwrap_or(0);
        if latest.is_none_or(|(best, _)| at > best) {
            latest = Some((at, candidate));
        }
    }
    Ok(latest.map(|(_, b)| Introduced { commit: b.commit.clone(), lines: b.lines, files: b.files.clone() }))
}

pub fn is_trivial(line: &str) -> bool {
    let code = line.trim();
    code.is_empty() || code.starts_with("//") || code.starts_with("/*") || code.starts_with('*') || code.chars().all(|c| "{}()[];,".contains(c) || c.is_whitespace())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Blamed {
    pub commit: String,
    pub lines: usize,
    pub files: Vec<String>,
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
    let mut counts: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
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
                    let entry = counts.entry(commit.clone()).or_default();
                    entry.0 += 1;
                    if !entry.1.contains(path) {
                        entry.1.push(path.clone());
                    }
                }
            } else if line.len() >= 40 && line.as_bytes()[..40].iter().all(u8::is_ascii_hexdigit) {
                commit = line[..40].to_string();
            } else if line == "boundary" {
                commit = format!("^{commit}");
            }
        }
    }
    let mut blamed: Vec<Blamed> = counts.into_iter().map(|(commit, (lines, files))| Blamed { commit, lines, files }).collect();
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
        assert_eq!(fix_reason("Merge branch 'pairing': paired daemons find each other", Some("Paired daemons lose each other for minutes")).as_deref(), Some("“lose” in the task"));
        assert_eq!(fix_reason("Merge branch 'resume': x", Some("Don’t resume another conversation")).as_deref(), Some("“don't” in the task"));
        assert_eq!(fix_reason("Merge branch 'fix-pane-menu': right-clicking shows the menu again", None).as_deref(), Some("branch fix-pane-menu"));
        assert_eq!(fix_reason("Merge restart: restarting loses nothing; it saves its terminals", None), None, "problem words count only in the task's own words");
        assert_eq!(fix_reason("Merge agent-sign-in: agents run on the user's accounts; a crash banner", None), None, "only the subject's first clause");
        assert_eq!(fix_reason("Merge main into stats: a bug fix", None), None);
        assert_eq!(fix_reason("Status dots on Shelf rows", Some("Shelf rows show a dot")), None);
        assert_eq!(fix_reason("Prefix the suffix", None), None);
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
        assert_eq!(blame(&repo, &format!("{fix}^1"), &removed).unwrap(), vec![Blamed { commit: landing.clone(), lines: 1, files: vec!["src/lib.rs".into()] }]);
        assert_eq!(introduced(&repo, &fix).unwrap().map(|i| i.commit), Some(landing.clone()));
        let inside = run(&repo, &["rev-parse", "feature"]);
        assert_eq!(landing_of(&repo, &inside, &fix), Some(landing.clone()), "a branch's commit lands with its merge");
        assert_eq!(landing_of(&repo, &landing, &fix), Some(landing));
        let parents: Vec<String> = run(&repo, &["show", "-s", "--format=%P", &fix]).split(' ').map(String::from).collect();
        assert_eq!(named_fixes(&repo, &fix, &parents), Vec::<String>::new());
        run(&repo, &["branch", "release", &fix]);
        std::fs::write(repo.join("src/other.rs"), "fn c() {}\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", &format!("c\n\nFixes: {fix}")]);
        let named = run(&repo, &["rev-parse", "HEAD"]);
        assert_eq!(named_fixes(&repo, &named, std::slice::from_ref(&fix)), vec![fix.clone()]);
        run(&repo, &["checkout", "-q", "release"]);
        run(&repo, &["cherry-pick", "-x", &named]);
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["merge", "-q", "--no-ff", "-m", "Merge branch 'release-back'", "release"]);
        let back = run(&repo, &["rev-parse", "HEAD"]);
        assert_eq!(named_fixes(&repo, &back, &[named, run(&repo, &["rev-parse", "release"])]), Vec::<String>::new(), "a release branch's pick was counted when it landed on main");
        let _ = std::fs::remove_dir_all(&repo);
    }
}
