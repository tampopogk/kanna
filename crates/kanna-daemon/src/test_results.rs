use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub label: String,
    pub source: Option<String>,
    pub binary: Option<String>,
    pub finished: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Test {
    pub key: String,
    pub name: String,
    pub group: usize,
    pub passed: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub groups: Vec<Group>,
    pub tests: Vec<Test>,
}

impl Results {
    pub fn failing(&self) -> BTreeSet<String> {
        self.tests.iter().filter(|t| !t.passed).map(|t| t.key.clone()).collect()
    }

    pub fn passing(&self) -> BTreeSet<String> {
        self.tests.iter().filter(|t| t.passed).map(|t| t.key.clone()).collect()
    }

    pub fn test(&self, key: &str) -> Option<&Test> {
        self.tests.iter().find(|t| t.key == key)
    }

    pub fn alone(&self, key: &str, worker: &Path) -> Option<Alone> {
        let test = self.test(key)?;
        let group = self.groups.get(test.group)?;
        let binary = group.binary.as_ref()?;
        if test.name.is_empty() {
            return None;
        }
        let path = worker.join(binary);
        let cwd = group.source.as_deref().map_or_else(|| worker.to_path_buf(), |source| package_dir(worker, source, binary));
        Some(Alone { key: key.to_string(), name: test.name.clone(), source: group.source.clone().unwrap_or_default(), binary: binary.clone(), path, cwd })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Alone {
    pub key: String,
    pub name: String,
    pub source: String,
    pub binary: String,
    pub path: PathBuf,
    pub cwd: PathBuf,
}

pub fn alone_script(tests: &[Alone]) -> String {
    let mut script = String::from("status=0\n");
    for t in tests {
        let cwd = t.cwd.to_string_lossy();
        script.push_str(&format!(
            "printf '     Running %s (%s)\\n' {source} {binary}\n( cd {cwd} && CARGO_MANIFEST_DIR={cwd} {path} --exact {name} --test-threads 1 ) || status=1\n",
            source = quote(&t.source),
            binary = quote(&t.binary),
            cwd = quote(&cwd),
            path = quote(&t.path.to_string_lossy()),
            name = quote(&t.name),
        ));
    }
    script.push_str("exit $status\n");
    script
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out
}

fn crate_of(binary: &str) -> String {
    let file = binary.rsplit('/').next().unwrap_or(binary);
    match file.rsplit_once('-') {
        Some((name, hash)) if !name.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit()) => name.to_string(),
        _ => file.to_string(),
    }
}

fn package_dir(worker: &Path, source: &str, binary: &str) -> PathBuf {
    let wanted = crate_of(binary);
    let mut found: Vec<(PathBuf, bool)> = vec![];
    let mut seen = vec![worker.to_path_buf()];
    let mut i = 0;
    while i < seen.len() && i < 2000 {
        let dir = seen[i].clone();
        i += 1;
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file() && dir.join(source).is_file() {
            let text = fs::read_to_string(&manifest).unwrap_or_default();
            let named = text.lines().find_map(|l| l.trim().strip_prefix("name").map(|r| r.trim_start().trim_start_matches('=').trim().trim_matches('"').replace('-', "_")));
            found.push((dir.clone(), named.as_deref() == Some(wanted.as_str())));
        }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                seen.push(entry.path());
            }
        }
    }
    found.iter().find(|(_, named)| *named).or(found.first()).map_or_else(|| worker.to_path_buf(), |(dir, _)| dir.clone())
}

pub fn parse(output: &str) -> Option<Results> {
    let mut results = Results::default();
    let mut current: Option<usize> = None;
    let mut libtest = false;
    for raw in output.lines() {
        let line = strip_ansi(raw);
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("Running ") {
            let rest = rest.trim_end();
            if let (Some(open), true) = (rest.rfind(" ("), rest.ends_with(')')) {
                let source = rest[..open].trim().trim_start_matches("unittests ").to_string();
                let binary = rest[open + 2..rest.len() - 1].to_string();
                let label = format!("{} {source}", crate_of(&binary));
                results.groups.push(Group { label, source: Some(source), binary: Some(binary), finished: false });
                current = Some(results.groups.len() - 1);
                continue;
            }
        }
        if let Some(name) = trimmed.strip_prefix("Doc-tests ") {
            results.groups.push(Group { label: format!("doc-tests {}", name.trim()), source: None, binary: None, finished: false });
            current = Some(results.groups.len() - 1);
            continue;
        }
        if line.starts_with("test result: ") {
            libtest = true;
            let group = current.unwrap_or_else(|| default_group(&mut results));
            results.groups[group].finished = true;
            current = None;
            continue;
        }
        let Some(rest) = line.strip_prefix("test ") else { continue };
        let Some((name, verdict)) = rest.rsplit_once(" ... ") else { continue };
        let passed = match verdict.trim() {
            "ok" => true,
            "FAILED" => false,
            v if v.starts_with("ignored") => continue,
            _ => continue,
        };
        libtest = true;
        let group = current.unwrap_or_else(|| {
            let g = default_group(&mut results);
            current = Some(g);
            g
        });
        let name = name.trim().to_string();
        let key = key(&results.groups[group].label, &name);
        match results.tests.iter_mut().find(|t| t.key == key) {
            Some(t) => t.passed = t.passed && passed,
            None => results.tests.push(Test { key, name, group, passed }),
        }
    }
    if !libtest {
        return None;
    }
    for (i, group) in results.groups.clone().iter().enumerate() {
        if !group.finished && group.binary.is_some() {
            results.tests.push(Test { key: format!("{}: didn't finish", group.label), name: String::new(), group: i, passed: false });
        }
    }
    Some(results)
}

fn default_group(results: &mut Results) -> usize {
    if let Some(i) = results.groups.iter().position(|g| g.label.is_empty()) {
        return i;
    }
    results.groups.push(Group { label: String::new(), source: None, binary: None, finished: false });
    results.groups.len() - 1
}

fn key(label: &str, name: &str) -> String {
    match label.is_empty() {
        true => name.to_string(),
        false => format!("{name} ({label})"),
    }
}

pub fn failure_text(output: &str, name: &str, lines: usize) -> Vec<String> {
    let start = format!("---- {name} stdout ----");
    let mut out = vec![];
    let mut inside = false;
    for raw in output.lines() {
        let line = strip_ansi(raw);
        if inside {
            if line.starts_with("---- ") || line.trim() == "failures:" || out.len() >= lines {
                break;
            }
            out.push(line);
        } else if line.trim() == start {
            inside = true;
            out.push(line);
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARGO: &str = "   Compiling kanna-daemon v0.1.0
    Finished `test` profile [optimized + debuginfo] target(s) in 3.10s
     Running unittests src/lib.rs (target/debug/deps/kanna_daemon-1f2e3d4c5b6a7980)

running 3 tests
test queue::tests::places_read_as_ordinals ... ok
test tasks::tests::flaky_one ... FAILED
test db::tests::slow ... ignored, takes a minute

failures:

---- tasks::tests::flaky_one stdout ----
thread 'tasks::tests::flaky_one' panicked at crates/kanna-daemon/src/tasks.rs:10:5:
assertion failed: ready

failures:
    tasks::tests::flaky_one

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running tests/cli.rs (target/debug/deps/cli-0a1b2c3d4e5f6071)

running 1 test
test usage_errors_are_kmux_errors ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s

     Running unittests src/main.rs (target/debug/deps/kanna_daemon-99aa88bb77cc66dd)

running 2 tests
test a_test_that_crashes ... ok
error: test failed, to rerun pass `-p kanna-daemon --bin kanna-daemon`

Caused by:
  process didn't exit successfully: `target/debug/deps/kanna_daemon-99aa88bb77cc66dd` (signal: 6, SIGABRT: process abort signal)
   Doc-tests kanna_mux

running 1 test
test crates/kanna-mux/src/lib.rs - json (line 3) ... FAILED

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.30s
";

    #[test]
    fn cargo_test_output_reads_as_each_test_and_whether_it_passed() {
        let results = parse(CARGO).unwrap();
        let failing: Vec<String> = results.failing().into_iter().collect();
        assert_eq!(
            failing,
            [
                "crates/kanna-mux/src/lib.rs - json (line 3) (doc-tests kanna_mux)",
                "kanna_daemon src/main.rs: didn't finish",
                "tasks::tests::flaky_one (kanna_daemon src/lib.rs)",
            ]
        );
        let passing: Vec<String> = results.passing().into_iter().collect();
        assert_eq!(passing, ["a_test_that_crashes (kanna_daemon src/main.rs)", "queue::tests::places_read_as_ordinals (kanna_daemon src/lib.rs)", "usage_errors_are_kmux_errors (cli tests/cli.rs)"]);
        assert!(results.test("db::tests::slow (kanna_daemon src/lib.rs)").is_none(), "ignored tests aren't results");
    }

    #[test]
    fn other_output_has_no_test_results() {
        assert_eq!(parse("Running: sh test.sh\n\nFAILED: broken is here\n"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn a_failing_test_runs_again_alone_from_its_binary_in_its_package() {
        let dir = std::env::temp_dir().join(format!("kanna-test-results-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("crates/kanna-daemon/src")).unwrap();
        fs::create_dir_all(dir.join("crates/other/src")).unwrap();
        fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(dir.join("crates/other/Cargo.toml"), "[package]\nname = \"other\"\n").unwrap();
        fs::write(dir.join("crates/other/src/lib.rs"), "").unwrap();
        fs::write(dir.join("crates/kanna-daemon/Cargo.toml"), "[package]\nname = \"kanna-daemon\"\n").unwrap();
        fs::write(dir.join("crates/kanna-daemon/src/lib.rs"), "").unwrap();
        let results = parse(CARGO).unwrap();
        let alone = results.alone("tasks::tests::flaky_one (kanna_daemon src/lib.rs)", &dir).unwrap();
        assert_eq!(alone.cwd, dir.join("crates/kanna-daemon"), "the package whose name is the binary's crate");
        assert_eq!(alone.path, dir.join("target/debug/deps/kanna_daemon-1f2e3d4c5b6a7980"));
        assert!(results.alone("crates/kanna-mux/src/lib.rs - json (line 3) (doc-tests kanna_mux)", &dir).is_none(), "doc tests have no binary to run");
        assert!(results.alone("kanna_daemon src/main.rs: didn't finish", &dir).is_none());
        let script = alone_script(&[alone]);
        assert!(script.contains("printf '     Running %s (%s)\\n' 'src/lib.rs' 'target/debug/deps/kanna_daemon-1f2e3d4c5b6a7980'"), "{script}");
        assert!(script.contains("--exact 'tasks::tests::flaky_one' --test-threads 1"), "{script}");
        let rerun = format!("     Running src/lib.rs (target/debug/deps/kanna_daemon-1f2e3d4c5b6a7980)\n\nrunning 1 test\ntest tasks::tests::flaky_one ... ok\n\ntest result: ok. 1 passed; 0 failed\n");
        assert_eq!(parse(&rerun).unwrap().passing().into_iter().collect::<Vec<_>>(), ["tasks::tests::flaky_one (kanna_daemon src/lib.rs)"], "the rerun's keys match the first run's");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_s_own_lines_are_found_by_its_name() {
        let lines = failure_text(CARGO, "tasks::tests::flaky_one", 10);
        assert_eq!(lines, ["---- tasks::tests::flaky_one stdout ----", "thread 'tasks::tests::flaky_one' panicked at crates/kanna-daemon/src/tasks.rs:10:5:", "assertion failed: ready"]);
    }
}
