use std::path::{Component, Path, PathBuf};

const SOURCE_FLAGS: &[&str] = &["-project", "-workspace", "-xctestrun", "--package-path"];

const CACHE_FLAGS: &[&str] = &["-derivedDataPath", "--scratch-path"];

const RESULT_FLAGS: &[&str] = &["-resultBundlePath", "-archivePath", "-exportPath"];

#[derive(Clone, Debug, PartialEq)]
pub struct Translated {
    pub argv: Vec<String>,
    pub map: Vec<(String, String)>,
    pub results: Vec<(PathBuf, String)>,
}

pub struct Roots<'a> {
    pub linux: &'a str,
    pub cwd: &'a str,
    pub src: &'a Path,
    pub results: &'a Path,
    pub cache: &'a Path,
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn tool_of(argv: &[String]) -> (&str, usize) {
    let name = |a: &str| a.rsplit('/').next().unwrap_or(a).to_string();
    if argv.first().map(|a| name(a)).as_deref() != Some("xcrun") {
        return (argv.first().map(|a| a.rsplit('/').next().unwrap_or(a)).unwrap_or_default(), 0);
    }
    let mut index = 1;
    while let Some(arg) = argv.get(index) {
        match arg.as_str() {
            "-sdk" | "--sdk" | "-toolchain" | "--toolchain" => index += 2,
            a if a.starts_with('-') => index += 1,
            a => return (a.rsplit('/').next().unwrap_or(a), index),
        }
    }
    ("xcrun", 0)
}

pub fn translate(argv: &[String], roots: &Roots) -> Result<Translated, String> {
    let linux = Path::new(roots.linux);
    let mut mapped = argv.to_vec();
    let mut results = Vec::new();
    let mut map = vec![(roots.src.to_string_lossy().into_owned(), roots.linux.to_string())];
    let (tool, at) = tool_of(argv);
    let mut index = 0;
    while index + 1 < argv.len() {
        let flag = argv[index].as_str();
        let given = Path::new(&argv[index + 1]);
        let absolute = normalize(&if given.is_absolute() { given.to_path_buf() } else { linux.join(roots.cwd).join(given) });
        let inside = absolute.strip_prefix(linux).ok().map(Path::to_path_buf);
        let target = if SOURCE_FLAGS.contains(&flag) {
            let rel = inside.ok_or_else(|| format!("{flag} {} is outside the worktree ({}); only the worktree is sent to the build Mac", argv[index + 1], roots.linux))?;
            Some(roots.src.join(rel))
        } else if CACHE_FLAGS.contains(&flag) {
            Some(roots.cache.join(inside.unwrap_or_else(|| PathBuf::from(absolute.file_name().unwrap_or_default()))))
        } else if RESULT_FLAGS.contains(&flag) {
            let name = absolute.file_name().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("result"));
            let target = roots.results.join(format!("out-{}", results.len())).join(name);
            results.push((target.clone(), absolute.to_string_lossy().into_owned()));
            Some(target)
        } else {
            None
        };
        if let Some(target) = target {
            map.push((target.to_string_lossy().into_owned(), absolute.to_string_lossy().into_owned()));
            mapped[index + 1] = target.to_string_lossy().into_owned();
            index += 2;
        } else {
            index += 1;
        }
    }
    let has = |flag: &str| argv.iter().any(|a| a == flag);
    if tool == "xcodebuild" {
        if !has("-derivedDataPath") {
            mapped.extend(["-derivedDataPath".to_string(), roots.cache.join("DerivedData").to_string_lossy().into_owned()]);
        }
        let testing = argv.iter().any(|a| matches!(a.as_str(), "test" | "test-without-building"));
        if testing && !has("-resultBundlePath") {
            let target = roots.results.join("Tests.xcresult");
            mapped.extend(["-resultBundlePath".to_string(), target.to_string_lossy().into_owned()]);
        }
    }
    if tool == "swift" && argv.get(at + 1).is_some_and(|a| matches!(a.as_str(), "build" | "test" | "run")) && !has("--scratch-path") {
        mapped.extend(["--scratch-path".to_string(), roots.cache.join("swiftpm").to_string_lossy().into_owned()]);
    }
    for (mac, linux) in map.clone() {
        for (long, short) in [("/private/tmp/", "/tmp/"), ("/private/var/", "/var/")] {
            if let Some(rest) = mac.strip_prefix(long) {
                map.push((format!("{short}{rest}"), linux.clone()));
            }
        }
    }
    map.sort_by_key(|(mac, _)| std::cmp::Reverse(mac.len()));
    Ok(Translated { argv: mapped, map, results })
}

fn is_diagnostic(line: &str) -> bool {
    [": error:", ": warning:", ": note:"].iter().any(|kind| {
        line.find(kind).is_some_and(|at| {
            let before = &line[..at];
            let mut parts = before.rsplitn(3, ':');
            let last = parts.next().unwrap_or_default();
            let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            digits(last)
        })
    })
}

pub fn map_line(line: &str, map: &[(String, String)]) -> String {
    let start = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(start);
    for (mac, linux) in map {
        if let Some(tail) = rest.strip_prefix(mac.as_str()) {
            if tail.starts_with('/') || tail.starts_with(':') {
                return format!("{indent}{linux}{tail}");
            }
        }
    }
    line.to_string()
}

pub fn diagnostics(text: &str, map: &[(String, String)]) -> Vec<String> {
    text.lines().filter(|line| is_diagnostic(line)).map(|line| map_line(line, map)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> (PathBuf, PathBuf, PathBuf) {
        (PathBuf::from("/private/tmp/m/src"), PathBuf::from("/private/tmp/m/jobs/j1/results"), PathBuf::from("/private/tmp/m/derived/x26"))
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn path_arguments_move_to_the_mirror_caches_and_results() {
        let (src, results, cache) = roots();
        let roots = Roots { linux: "/home/u/app/.kanna-worktrees/task-1", cwd: "App", src: &src, results: &results, cache: &cache };
        let argv = strings(&["xcodebuild", "-project", "../App.xcodeproj", "-scheme", "App", "test", "-resultBundlePath", "/tmp/R.xcresult"]);
        let t = translate(&argv, &roots).unwrap();
        assert_eq!(t.argv[2], "/private/tmp/m/src/App.xcodeproj");
        assert_eq!(t.argv[7], "/private/tmp/m/jobs/j1/results/out-0/R.xcresult");
        assert_eq!(&t.argv[8..], ["-derivedDataPath", "/private/tmp/m/derived/x26/DerivedData"]);
        assert_eq!(t.results, vec![(results.join("out-0/R.xcresult"), "/tmp/R.xcresult".to_string())]);
        let outside = strings(&["xcodebuild", "-workspace", "/etc/W.xcworkspace"]);
        assert!(translate(&outside, &roots).unwrap_err().contains("outside the worktree"));
    }

    #[test]
    fn a_test_without_a_result_path_gets_one_and_swift_gets_a_scratch_path() {
        let (src, results, cache) = roots();
        let roots = Roots { linux: "/w", cwd: "", src: &src, results: &results, cache: &cache };
        let t = translate(&strings(&["xcodebuild", "-scheme", "App", "test"]), &roots).unwrap();
        assert!(t.argv.ends_with(&strings(&["-resultBundlePath", "/private/tmp/m/jobs/j1/results/Tests.xcresult"])));
        let t = translate(&strings(&["swift", "test"]), &roots).unwrap();
        assert_eq!(&t.argv[2..], ["--scratch-path", "/private/tmp/m/derived/x26/swiftpm"]);
        let t = translate(&strings(&["xcrun", "simctl", "list"]), &roots).unwrap();
        assert_eq!(t.argv, strings(&["xcrun", "simctl", "list"]));
    }

    #[test]
    fn xcodebuild_and_swift_through_xcrun_get_the_same_defaults() {
        let (src, results, cache) = roots();
        let roots = Roots { linux: "/w", cwd: "", src: &src, results: &results, cache: &cache };
        let direct = translate(&strings(&["xcodebuild", "-scheme", "App", "test"]), &roots).unwrap();
        let wrapped = translate(&strings(&["xcrun", "-sdk", "macosx", "xcodebuild", "-scheme", "App", "test"]), &roots).unwrap();
        assert_eq!(&wrapped.argv[3..], &direct.argv[..]);
        let t = translate(&strings(&["xcrun", "--toolchain", "swift", "swift", "test"]), &roots).unwrap();
        assert!(t.argv.ends_with(&strings(&["--scratch-path", "/private/tmp/m/derived/x26/swiftpm"])));
    }

    #[test]
    fn diagnostics_name_the_worktree_also_when_xcode_drops_private() {
        let (src, results, cache) = roots();
        let roots = Roots { linux: "/w", cwd: "", src: &src, results: &results, cache: &cache };
        let t = translate(&strings(&["xcodebuild", "build"]), &roots).unwrap();
        let log = "CompileSwift normal arm64 /tmp/m/src/App/App.swift\n/tmp/m/src/App/App.swift:14:30: error: cannot find 'x' in scope\n/private/tmp/m/src/App/B.swift:3: warning: unused\nnote: not a location\n/elsewhere/C.swift:1:1: error: outside\n";
        assert_eq!(diagnostics(log, &t.map), ["/w/App/App.swift:14:30: error: cannot find 'x' in scope", "/w/App/B.swift:3: warning: unused", "/elsewhere/C.swift:1:1: error: outside"]);
    }
}
