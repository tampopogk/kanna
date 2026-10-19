use crate::rules::{Agent, RuleSet};
use std::collections::HashMap;
use std::io::Read;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const PROGRAMS: &[(&str, &str)] =
    &[("claude", "claude"), ("codex", "codex"), ("opencode", "opencode"), ("copilot", "copilot"), ("agy", "antigravity"), ("antigravity", "antigravity"), ("mock-agent", "mock")];

const INTERPRETERS: &[&str] = &["node", "bun", "deno", "python", "python3", "ruby", "perl", "sh", "bash", "zsh", "dash", "fish"];
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_OUTPUT: u64 = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: i32,
    pub executable: Option<PathBuf>,
    pub argv: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub agent: String,
    pub pid: i32,
    pub executable: Option<PathBuf>,
}

pub fn known_agent(program: &str) -> Option<&'static str> {
    PROGRAMS.iter().find(|(name, _)| *name == program).map(|(_, agent)| *agent)
}

pub fn known_agents() -> Vec<&'static str> {
    let mut agents: Vec<&str> = PROGRAMS.iter().map(|(_, a)| *a).collect();
    agents.dedup();
    agents
}

pub fn foreground_group(fd: RawFd) -> Option<i32> {
    let group = unsafe { libc::tcgetpgrp(fd) };
    (group > 0).then_some(group)
}

pub fn identify(group: i32, rules: &RuleSet) -> Option<Program> {
    let mut members = group_members(group);
    members.sort_by_key(|pid| (*pid != group, *pid));
    let processes: Vec<Process> = members.into_iter().filter_map(process).collect();
    let found = processes.iter().filter_map(recognise).find(|p| p.executable.is_some()).or_else(|| processes.iter().find_map(recognise))?;
    rules.agent(&found.agent).is_some().then_some(found)
}

pub fn group_argv(group: i32) -> Vec<Vec<String>> {
    group_members(group).into_iter().filter_map(process).map(|p| p.argv).collect()
}

pub fn agent_of(executable: &Path) -> Option<&'static str> {
    let name = |p: &Path| p.file_name().and_then(|n| n.to_str()).map(str::to_owned);
    if let Some(agent) = name(executable).as_deref().and_then(known_agent) {
        return Some(agent);
    }
    let parent = executable.parent()?;
    if name(parent).as_deref() != Some("versions") {
        return None;
    }
    name(parent.parent()?).as_deref().and_then(known_agent)
}

fn resolved(executable: &Path) -> Option<PathBuf> {
    executable.is_absolute().then(|| std::fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf()))
}

pub fn recognise(process: &Process) -> Option<Program> {
    let base = |p: &str| Path::new(p).file_name().and_then(|n| n.to_str()).map(str::to_owned);
    let executable = process.executable.as_deref().and_then(resolved);
    let exe = executable.as_ref().and_then(|p| p.to_str()).and_then(base);
    if exe.as_deref().is_some_and(|n| n.starts_with("kanna")) {
        return None;
    }
    if let Some(path) = executable.as_ref() {
        if let Some(agent) = agent_of(path) {
            return Some(Program { agent: agent.into(), pid: process.pid, executable: executable.clone() });
        }
    }
    let argv0 = process.argv.first().and_then(|a| base(a));
    if let Some(agent) = argv0.as_deref().and_then(known_agent) {
        return Some(Program { agent: agent.into(), pid: process.pid, executable: None });
    }
    let names = [argv0, exe];
    let interpreter = names.iter().flatten().any(|n| INTERPRETERS.contains(&n.to_ascii_lowercase().trim_end_matches(|c: char| c.is_ascii_digit() || c == '.')));
    if !interpreter {
        return None;
    }
    let script = process.argv.iter().skip(1).take_while(|a| *a != "-c" && *a != "-e").find(|a| !a.starts_with('-'))?;
    let agent = known_agent(Path::new(script).file_stem()?.to_str()?)?;
    Some(Program { agent: agent.into(), pid: process.pid, executable: None })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProbeKey {
    executable: PathBuf,
    modified: u128,
}

enum Probed {
    Running,
    Done(Option<String>),
}

fn probes() -> &'static Mutex<HashMap<ProbeKey, Probed>> {
    static PROBES: OnceLock<Mutex<HashMap<ProbeKey, Probed>>> = OnceLock::new();
    PROBES.get_or_init(Mutex::default)
}

pub enum Version {
    Known(Option<String>),
    Pending,
}

pub fn version(program: &Program, agent: &Agent) -> Version {
    let (Some(executable), Some(pattern)) = (program.executable.as_deref().and_then(resolved), agent.probe_pattern().cloned()) else { return Version::Known(None) };
    if agent_of(&executable) != Some(program.agent.as_str()) || agent.name() != program.agent {
        return Version::Known(None);
    }
    let modified = std::fs::metadata(&executable).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    let key = ProbeKey { executable: executable.clone(), modified };
    let mut table = probes().lock().unwrap_or_else(|e| e.into_inner());
    match table.get(&key) {
        Some(Probed::Done(found)) => return Version::Known(found.clone()),
        Some(Probed::Running) => return Version::Pending,
        None => {}
    }
    table.insert(key.clone(), Probed::Running);
    drop(table);
    std::thread::Builder::new()
        .name("kanna.probe".into())
        .spawn(move || {
            let found = probe(&executable, PROBE_TIMEOUT).and_then(|text| pattern.captures(&text).and_then(|c| c.get(1).or_else(|| c.get(0))).map(|m| m.as_str().chars().take(64).collect()));
            probes().lock().unwrap_or_else(|e| e.into_inner()).insert(key, Probed::Done(found));
        })
        .map(|_| Version::Pending)
        .unwrap_or(Version::Known(None))
}

pub fn probe(executable: &Path, timeout: Duration) -> Option<String> {
    if !executable.is_absolute() {
        return None;
    }
    let mut command = Command::new(executable);
    command.arg("--version").env_clear().env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    for key in ["HOME", "USER", "LANG", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = (&mut stdout).take(PROBE_OUTPUT).read_to_end(&mut text);
        text
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(target_os = "macos")]
fn group_members(group: i32) -> Vec<i32> {
    let mut pids = vec![0i32; 256];
    let count = unsafe { libc::proc_listpgrppids(group, pids.as_mut_ptr().cast(), (pids.len() * std::mem::size_of::<i32>()) as libc::c_int) };
    if count <= 0 {
        return vec![group];
    }
    pids.truncate(count as usize);
    pids.retain(|pid| *pid > 0);
    if !pids.contains(&group) {
        pids.push(group);
    }
    pids
}

#[cfg(target_os = "macos")]
fn process(pid: i32) -> Option<Process> {
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    let executable = (n > 0).then(|| PathBuf::from(String::from_utf8_lossy(&path[..n as usize]).into_owned()));
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    let mut argv = vec![];
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } == 0 && size > 4 {
        let mut buffer = vec![0u8; size];
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buffer.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) } == 0 {
            buffer.truncate(size);
            argv = procargs(&buffer);
        }
    }
    (executable.is_some() || !argv.is_empty()).then_some(Process { pid, executable, argv })
}

#[cfg(target_os = "macos")]
fn procargs(buffer: &[u8]) -> Vec<String> {
    if buffer.len() < 4 {
        return vec![];
    }
    let argc = i32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]).max(0) as usize;
    let mut rest = &buffer[4..];
    let path_end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
    rest = &rest[path_end..];
    let start = rest.iter().position(|b| *b != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    rest.split(|b| *b == 0).take(argc).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
}

#[cfg(target_os = "linux")]
fn group_members(group: i32) -> Vec<i32> {
    let mut pids: Vec<i32> = std::fs::read_dir("/proc")
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
                .filter(|pid| {
                    std::fs::read_to_string(format!("/proc/{pid}/stat"))
                        .ok()
                        .and_then(|stat| stat.rsplit_once(')').map(|(_, rest)| rest.split_whitespace().nth(2).and_then(|g| g.parse::<i32>().ok())))
                        .flatten()
                        == Some(group)
                })
                .collect()
        })
        .unwrap_or_default();
    if !pids.contains(&group) {
        pids.push(group);
    }
    pids
}

#[cfg(target_os = "linux")]
fn process(pid: i32) -> Option<Process> {
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    let argv: Vec<String> = std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|raw| raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect())
        .unwrap_or_default();
    (executable.is_some() || !argv.is_empty()).then_some(Process { pid, executable, argv })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn group_members(group: i32) -> Vec<i32> {
    vec![group]
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process(_pid: i32) -> Option<Process> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Source;
    use std::os::unix::fs::PermissionsExt;

    fn proc(executable: &str, argv: &[&str]) -> Process {
        Process { pid: 7, executable: Some(executable.into()), argv: argv.iter().map(|s| s.to_string()).collect() }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-detect-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_small_process_group_lists_every_member_not_just_its_leader() {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("/bin/sh").args(["-c", "sleep 5 & sleep 5"]).process_group(0).spawn().unwrap();
        let group = child.id() as i32;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut members = group_members(group);
        while members.len() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            members = group_members(group);
        }
        let _ = unsafe { libc::killpg(group, libc::SIGTERM) };
        let _ = child.wait();
        assert_eq!(members.len(), 3, "the shell and its two sleeps: {members:?}");
    }

    #[test]
    fn agents_are_known_by_their_name_their_binary_or_their_script() {
        let found = |p: Process| recognise(&p).map(|p| (p.agent, p.executable));
        assert_eq!(
            found(proc("/Users/x/.local/share/claude/versions/2.1.296", &["claude"])),
            Some(("claude".into(), Some("/Users/x/.local/share/claude/versions/2.1.296".into())))
        );
        assert_eq!(found(proc("/opt/vendor/codex/codex", &["/opt/vendor/codex/codex", "--yolo"])).map(|f| f.0), Some("codex".into()));
        assert_eq!(found(proc("/usr/local/bin/node", &["node", "/n/lib/node_modules/@openai/codex/bin/codex.js", "resume"])), Some(("codex".into(), None)));
        assert_eq!(found(proc("/usr/bin/python3", &["/usr/bin/python3", "/t/mock-agent"])), Some(("mock".into(), None)));
        assert_eq!(found(proc("/bin/zsh", &["-zsh"])), None);
        assert_eq!(found(proc("/bin/zsh", &["zsh", "-l"])), None);
        assert_eq!(found(proc("/bin/zsh", &["zsh", "-l", "-c", "/t/mock-agent"])), None);
        assert_eq!(found(proc("/bin/zsh", &["zsh", "-l", "-i", "-c", "/t/mock-agent"])), None);
        assert_eq!(found(proc("/usr/bin/vim", &["vim", "claude.md"])), None);
        assert_eq!(found(proc("/x/kanna-keeper", &["claude"])), None);
        assert_eq!(found(proc("/usr/bin/evil", &["evil"])), None);
    }

    #[test]
    fn the_agent_is_decided_by_the_path_that_would_run() {
        let dir = scratch("gate");
        let ran = dir.join("ran");
        let evil = script(&dir, "evil", &format!("touch {}; echo 'claude 9.9.9'", ran.display()));
        let named = recognise(&proc(evil.to_str().unwrap(), &["claude"])).unwrap();
        assert_eq!((named.agent.as_str(), named.executable.as_ref()), ("claude", None));
        let link = dir.join("codex");
        std::os::unix::fs::symlink(&evil, &link).unwrap();
        assert_eq!(recognise(&proc(link.to_str().unwrap(), &["evil"])), None);
        let versions = dir.join("claude/versions");
        std::fs::create_dir_all(&versions).unwrap();
        let real = script(&versions, "2.1.296", "echo '2.1.296 (Claude Code)'");
        let found = recognise(&proc(real.to_str().unwrap(), &["claude"])).unwrap();
        assert_eq!(found.executable, Some(std::fs::canonicalize(&real).unwrap()));
        let text = r#"{"schema":1,"agent":"claude","version":"1","engine":1,"versionProbe":{"pattern":"(\\d+\\.\\d+\\.\\d+)"},"rules":[]}"#;
        let agent = Agent::parse(text, Source::Bundled).unwrap();
        let forged = Program { agent: "claude".into(), pid: 1, executable: Some(evil.clone()) };
        assert!(matches!(super::version(&forged, &agent), Version::Known(None)));
        let mut version = super::version(&found, &agent);
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(version, Version::Pending) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            version = super::version(&found, &agent);
        }
        assert!(matches!(version, Version::Known(Some(ref v)) if v == "2.1.296"));
        assert!(!ran.exists(), "a binary that only claimed the agent's name ran");
    }

    #[test]
    fn a_probe_that_never_answers_is_stopped() {
        let dir = scratch("hang");
        let hang = script(&dir, "hang", "exec sleep 30");
        let started = Instant::now();
        assert_eq!(probe(&hang, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn rules_cannot_choose_what_runs() {
        let dir = scratch("probe");
        let marker = dir.join("ran-the-rule");
        let args = dir.join("args");
        let codex = script(&dir, "codex", &format!("printf '%s|' \"$@\" > {}; echo 'codex-cli 9.8.7'", args.display()));
        let text = format!(
            r#"{{"schema":1,"agent":"codex","version":"1","engine":1,"programs":["evil"],
                "versionProbe":{{"argv":["/bin/sh","-c","touch {m}"],"args":["-c","touch {m}"],"pattern":"(\\d+\\.\\d+\\.\\d+)"}},"rules":[]}}"#,
            m = marker.display()
        );
        let agent = Agent::parse(&text, Source::Remote("https://example.com/codex.json".into())).unwrap();
        let program = Program { agent: "codex".into(), pid: 1, executable: Some(codex.clone()) };
        let mut version = super::version(&program, &agent);
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(version, Version::Pending) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            version = super::version(&program, &agent);
        }
        assert!(matches!(version, Version::Known(Some(ref v)) if v == "9.8.7"));
        assert_eq!(std::fs::read_to_string(&args).unwrap(), "--version|");
        assert!(!marker.exists(), "the rule file's command ran");
        assert_eq!(recognise(&proc("/usr/bin/evil", &["evil"])), None);
        let relative = Program { agent: "codex".into(), pid: 1, executable: None };
        assert!(matches!(super::version(&relative, &agent), Version::Known(None)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_process_reads_its_own_arguments() {
        let me = process(std::process::id() as i32).unwrap();
        assert!(me.executable.is_some());
        assert!(!me.argv.is_empty());
    }
}
