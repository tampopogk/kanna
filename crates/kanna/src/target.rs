use kanna_build::{Build, Channel, SOCKET};
use kanna_client::{exit, fail, valid_instance, Brand, Error, Failure, Kmux, DEFAULT_INSTANCE};
use serde_json::{json, Value};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const NO_APP: u8 = 6;

pub const KANNA: Brand = Brand { id: "kanna", name: "Kanna", bundle_id: Channel::Dev.bundle_id() };

const APP_START: Duration = Duration::from_secs(20);

pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Backend {
    Kanna,
    Kmux,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub build: Build,
    pub backend: Backend,
    pub instance: String,
    pub socket: PathBuf,
    pub pinned: Option<String>,
    pub background: bool,
    pub fresh: bool,
}

pub struct Choice {
    pub mux: Option<String>,
    pub instance: Option<String>,
    pub background: bool,
    pub fresh: bool,
}

impl Target {
    pub fn resolve(build: &Build, choice: Choice, env: Env) -> Result<Target, Failure> {
        let backend = match choice.mux {
            Some(mux) => backend(&mux, "")?,
            None => match env("KANNA_MUX") {
                Some(mux) => backend(&mux, " (from KANNA_MUX)")?,
                None => Backend::Kanna,
            },
        };
        let brand = match backend {
            Backend::Kanna => KANNA,
            Backend::Kmux => Brand::KMUX,
        };
        let variable = |name: &str| env(&brand.variable(name));
        let background = choice.background || variable("BG").as_deref() == Some("1");
        let fresh = choice.fresh || variable("FRESH").as_deref() == Some("1");
        let socket_variable = brand.variable("SOCKET");
        let (instance, socket, pinned) = match (choice.instance, env(&socket_variable)) {
            (Some(name), _) => (checked(name, "")?, None, None),
            (None, Some(socket)) => (variable("INSTANCE").unwrap_or_else(|| DEFAULT_INSTANCE.into()), Some(PathBuf::from(socket)), Some(socket_variable)),
            (None, None) => match variable("INSTANCE") {
                Some(name) => (checked(name, &format!(" (from {})", brand.variable("INSTANCE")))?, None, None),
                None => (match backend {
                    Backend::Kanna => build.default_instance(),
                    Backend::Kmux => DEFAULT_INSTANCE.to_string(),
                }, None, None),
            },
        };
        let socket = socket.unwrap_or_else(|| match backend {
            Backend::Kanna => instance_socket(build.channel, &instance, env),
            Backend::Kmux => brand_socket(Brand::KMUX, &instance, env),
        });
        Ok(Target { build: build.clone(), backend, instance, socket, pinned, background, fresh })
    }

    pub fn brand(&self) -> Brand {
        match self.backend {
            Backend::Kanna => KANNA,
            Backend::Kmux => Brand::KMUX,
        }
    }

    pub fn name(&self) -> &'static str {
        self.brand().name
    }

    fn client(&self) -> kanna_client::Target {
        kanna_client::Target { brand: self.brand(), instance: self.instance.clone(), socket: self.socket.clone(), background: self.background }
    }

    pub fn try_connect(&self) -> Option<Kmux> {
        Kmux::try_connect(&self.client())
    }

    pub fn running(&self, env: Env) -> Vec<Target> {
        let names: Vec<String> = match self.backend {
            Backend::Kanna => instances(env).into_iter().filter(|i| i.channel == self.build.channel).map(|i| i.name).collect(),
            Backend::Kmux => kmux_names(env),
        };
        names
            .into_iter()
            .map(|name| {
                let socket = match self.backend {
                    Backend::Kanna => instance_socket(self.build.channel, &name, env),
                    Backend::Kmux => brand_socket(Brand::KMUX, &name, env),
                };
                Target { build: self.build.clone(), backend: self.backend, instance: name, socket, pinned: None, background: false, fresh: false }
            })
            .filter(|t| UnixStream::connect(&t.socket).is_ok())
            .collect()
    }
}

fn backend(mux: &str, from: &str) -> Result<Backend, Failure> {
    match mux {
        "kanna" | "kanna-dev" => Ok(Backend::Kanna),
        "kmux" => Ok(Backend::Kmux),
        other => Err(fail(exit::UNSUPPORTED, format!("the {other} backend{from} isn't supported yet; use kanna or kmux"))),
    }
}

fn checked(name: String, from: &str) -> Result<String, Failure> {
    if valid_instance(&name) {
        return Ok(name);
    }
    Err(fail(exit::USAGE, format!("bad instance name \"{name}\"{from}: use letters, digits, - and _ (up to 32)")))
}

pub fn support_folder(env: Env) -> PathBuf {
    let home = PathBuf::from(env("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        return home.join("Library/Application Support");
    }
    env("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"))
}

pub fn instance_socket(channel: Channel, name: &str, env: Env) -> PathBuf {
    support_folder(env).join(channel.instance_folder(name)).join(SOCKET)
}

fn brand_socket(brand: Brand, name: &str, env: Env) -> PathBuf {
    let id = brand.id;
    let file = if name == DEFAULT_INSTANCE { format!("{id}.sock") } else { format!("{id}-{name}.sock") };
    support_folder(env).join(id).join(file)
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort_by_key(|n| (n != DEFAULT_INSTANCE, n.clone()));
    names
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub channel: Channel,
    pub name: String,
    pub folder: PathBuf,
    pub socket: PathBuf,
    pub worktree: Option<PathBuf>,
}

impl Instance {
    pub fn answers(&self) -> bool {
        UnixStream::connect(&self.socket).is_ok()
    }
}

pub fn keeper_pid(folder: &Path) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(folder.join("keeper.pid")).ok()?.trim().parse().ok()?;
    (pid > 0 && unsafe { libc::kill(pid, 0) } == 0).then_some(pid)
}

pub fn instances(env: Env) -> Vec<Instance> {
    let support = support_folder(env);
    let mut found: Vec<Instance> = std::fs::read_dir(&support)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let (channel, name) = kanna_build::parse_folder(&entry.file_name().into_string().ok()?)?;
            let folder = entry.path();
            let worktree = kanna_build::read_record(&folder).and_then(|r| r["worktree"].as_str().map(PathBuf::from));
            Some(Instance { channel, name, socket: folder.join(SOCKET), folder, worktree })
        })
        .collect();
    found.sort_by_key(|i| (Channel::ALL.iter().position(|c| *c == i.channel), i.name != DEFAULT_INSTANCE, i.name.clone()));
    found
}

fn kmux_names(env: Env) -> Vec<String> {
    let names = std::fs::read_dir(support_folder(env).join("kmux"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name().into_string().ok()?;
            if file == "kmux.sock" {
                return Some(DEFAULT_INSTANCE.to_string());
            }
            Some(file.strip_prefix("kmux-")?.strip_suffix(".sock")?.to_string())
        })
        .collect();
    sorted(names)
}

pub fn launch_command(target: &Target, env: Env) -> Command {
    let mut open = Command::new("/usr/bin/open");
    open.arg("-n");
    if target.background {
        open.arg("-g");
    }
    open.arg("--env").arg(format!("KANNA_SOCKET={}", target.socket.display()));
    if let Some(folder) = env("KANNA_STATE_DIR") {
        open.arg("--env").arg(format!("KANNA_STATE_DIR={folder}"));
    }
    if let Some(owner) = env("KANNA_E2E_OWNER") {
        open.arg("--env").arg(format!("KANNA_E2E_OWNER={owner}"));
    }
    if let Some(folder) = env("PWD").filter(|f| f.starts_with('/')) {
        open.arg("--env").arg(format!("KANNA_START_DIR={folder}"));
    }
    match (env("KANNA_APP"), target.build.app()) {
        (Some(app), _) => open.arg("-a").arg(app),
        (None, Some(app)) => open.arg("-a").arg(app),
        (None, None) => open.arg("-b").arg(target.build.bundle_id()),
    };
    let mut flags = vec![];
    if target.background {
        flags.push("--bg");
    }
    if target.fresh {
        flags.push("--fresh");
    }
    if !flags.is_empty() {
        open.arg("--args").args(flags);
    }
    open
}

fn launch_app(target: &Target, code: u8, env: Env) -> Result<(), Failure> {
    if !cfg!(target_os = "macos") {
        return Err(fail(code, format!("Kanna's daemon isn't running on {} (start kanna-daemon)", target.socket.display())));
    }
    if target.pinned.is_none() {
        if let Some(folder) = target.socket.parent() {
            make_folder(folder);
        }
    }
    let status = launch_command(target, env).status().map_err(|e| fail(code, format!("could not start Kanna.app: {e}")))?;
    if status.success() {
        return Ok(());
    }
    let remedy = match target.build.app() {
        Some(app) => format!("build {} with scripts/build.sh", app.display()),
        None => format!("install {}", target.build.bundle_id()),
    };
    Err(fail(code, format!("Kanna isn't running on {} and Kanna.app ({}) couldn't be started (set KANNA_APP to it, or {remedy})", target.socket.display(), target.build.app_name())))
}

fn make_folder(folder: &Path) {
    use std::os::unix::fs::DirBuilderExt;
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(folder);
}

pub struct Session<'a> {
    mux: Kmux,
    target: &'a Target,
    started: Option<Instant>,
    env: Env<'a>,
}

impl<'a> Session<'a> {
    pub fn connect(target: &'a Target, env: Env<'a>) -> Result<Session<'a>, Failure> {
        if let Some(mux) = target.try_connect() {
            return Ok(Session { mux, target, started: None, env });
        }
        if let Some(variable) = &target.pinned {
            return Err(fail(exit::UNREACHABLE, format!("nothing answers on {} ({variable} is set, so kanna doesn't start an app for it)", target.socket.display())));
        }
        if target.backend == Backend::Kmux {
            if target.fresh {
                std::env::set_var("KMUX_FRESH", "1");
            }
            let mux = Kmux::connect_to(&target.client()).map_err(Failure::from)?;
            return Ok(Session { mux, target, started: None, env });
        }
        launch_app(target, exit::UNREACHABLE, env)?;
        let started = Instant::now();
        loop {
            if let Some(mux) = target.try_connect() {
                return Ok(Session { mux, target, started: Some(started), env });
            }
            if started.elapsed() > APP_START {
                return Err(fail(exit::UNREACHABLE, format!("Kanna.app was started, but nothing answered on {} within {}s", target.socket.display(), APP_START.as_secs())));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn call(&mut self, cmd: &str, args: Value) -> Result<Value, Failure> {
        loop {
            match self.mux.call(cmd, args.clone()) {
                Err(Error::Mux { code, message }) if code == "no_app" => {
                    if let Some(variable) = &self.target.pinned {
                        return Err(fail(NO_APP, format!("{message} [no_app] ({variable} is set, so kanna doesn't start Kanna.app for it)")));
                    }
                    match self.started {
                        None => {
                            launch_app(self.target, NO_APP, self.env)?;
                            self.started = Some(Instant::now());
                        }
                        Some(at) if at.elapsed() > APP_START => {
                            return Err(fail(NO_APP, format!("{message} [no_app] (Kanna.app was started but didn't attach to the daemon on {} within {}s)", self.target.socket.display(), APP_START.as_secs())));
                        }
                        Some(_) => {}
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(Error::Mux { code, message }) if code == "unsupported" => return Err(fail(exit::UNSUPPORTED, format!("{message} [{code}]"))),
                Err(error @ Error::Mux { .. }) => {
                    let mut failure = Failure::from(error);
                    if let Some(note) = self.other_build() {
                        failure.message = format!("{}\n{note}", failure.message);
                    }
                    return Err(failure);
                }
                other => return other.map_err(Failure::from),
            }
        }
    }

    fn other_build(&mut self) -> Option<String> {
        if self.target.backend != Backend::Kanna {
            return None;
        }
        let ours = kanna_build::sibling_daemon_build_id()?;
        let reply = self.mux.call("capabilities", json!({})).ok()?;
        let theirs = reply.get("build")?.as_str()?;
        other_build_note(&ours, theirs)
    }
}

pub fn other_build_note(ours: &str, theirs: &str) -> Option<String> {
    (ours != theirs).then(|| {
        let short = |id: &str| id.chars().take(12).collect::<String>();
        format!("(the daemon on this socket is from another build than this kanna ({} here, {} there); quit and relaunch Kanna.app to replace it)", short(ours), short(theirs))
    })
}

pub fn capabilities(target: &Target) -> Option<Value> {
    target.try_connect()?.call("capabilities", json!({})).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    fn dev() -> Build {
        Build { channel: Channel::Dev, checkout: PathBuf::from("/r/kanna"), worktree: None }
    }

    fn resolve(choice: Choice, env: Env) -> Result<Target, Failure> {
        Target::resolve(&dev(), choice, env)
    }

    fn choice() -> Choice {
        Choice { mux: None, instance: None, background: false, fresh: false }
    }

    fn support(home: &str) -> String {
        if cfg!(target_os = "macos") { format!("{home}/Library/Application Support") } else { format!("{home}/.local/share") }
    }

    #[test]
    fn a_daemon_of_another_build_gets_a_note() {
        assert_eq!(other_build_note("abc", "abc"), None);
        let note = other_build_note("0123456789abcdef", "fedcba9876543210").unwrap();
        assert!(note.contains("0123456789ab here, fedcba987654 there") && note.contains("relaunch Kanna.app"), "{note}");
    }

    #[test]
    fn the_default_is_the_default_instance_of_kanna() {
        let target = resolve(choice(), &env(&[("HOME", "/Users/x")])).unwrap();
        assert_eq!(target.backend, Backend::Kanna);
        assert_eq!(target.instance, "default");
        assert_eq!(target.socket, PathBuf::from(format!("{}/kanna-dev/kanna.sock", support("/Users/x"))));
        assert_eq!(target.pinned, None);
    }

    #[test]
    fn a_build_targets_its_own_channel_and_its_worktrees_instance() {
        let home = env(&[("HOME", "/Users/x")]);
        let worktree = Build { worktree: Some("task-3f9a1c2e".into()), ..dev() };
        let target = Target::resolve(&worktree, choice(), &home).unwrap();
        assert_eq!(target.instance, "task-3f9a1c2e");
        assert_eq!(target.socket, PathBuf::from(format!("{}/kanna-dev-task-3f9a1c2e/kanna.sock", support("/Users/x"))));
        let production = Build { channel: Channel::Production, ..worktree.clone() };
        let target = Target::resolve(&production, choice(), &home).unwrap();
        assert_eq!((target.instance.as_str(), target.socket.clone()), ("default", PathBuf::from(format!("{}/kanna-prod/kanna.sock", support("/Users/x")))));
        let named = Target::resolve(&production, Choice { instance: Some("work".into()), ..choice() }, &home).unwrap();
        assert_eq!(named.socket, PathBuf::from(format!("{}/kanna-prod-work/kanna.sock", support("/Users/x"))));
        let pinned = Target::resolve(&worktree, choice(), &env(&[("HOME", "/Users/x"), ("KANNA_SOCKET", "/p/kanna.sock"), ("KANNA_INSTANCE", "default")])).unwrap();
        assert_eq!((pinned.instance.as_str(), pinned.socket.clone()), ("default", PathBuf::from("/p/kanna.sock")));
    }

    #[test]
    fn instance_beats_socket_beats_instance_variable() {
        let all = env(&[("HOME", "/h"), ("KANNA_SOCKET", "/tmp/s.sock"), ("KANNA_INSTANCE", "env")]);
        let named = resolve(Choice { instance: Some("work".into()), ..choice() }, &all).unwrap();
        assert_eq!(named.socket, PathBuf::from(format!("{}/kanna-dev-work/kanna.sock", support("/h"))));
        assert_eq!(named.pinned, None);
        let pinned = resolve(choice(), &all).unwrap();
        assert_eq!(pinned.socket, PathBuf::from("/tmp/s.sock"));
        assert_eq!(pinned.instance, "env");
        assert_eq!(pinned.pinned.as_deref(), Some("KANNA_SOCKET"));
        let by_name = resolve(choice(), &env(&[("HOME", "/h"), ("KANNA_INSTANCE", "env")])).unwrap();
        assert_eq!(by_name.socket, PathBuf::from(format!("{}/kanna-dev-env/kanna.sock", support("/h"))));
    }

    #[test]
    fn bad_instance_names_are_bad_usage() {
        let failure = resolve(Choice { instance: Some("../x".into()), ..choice() }, &env(&[])).unwrap_err();
        assert_eq!(failure.code, exit::USAGE);
        let failure = resolve(choice(), &env(&[("KANNA_INSTANCE", "a/b")])).unwrap_err();
        assert!(failure.message.contains("KANNA_INSTANCE"), "{}", failure.message);
    }

    #[test]
    fn the_kmux_backend_uses_kmux_sockets_and_variables() {
        let target = resolve(Choice { mux: Some("kmux".into()), ..choice() }, &env(&[("HOME", "/h"), ("KANNA_SOCKET", "/x.sock")])).unwrap();
        assert_eq!(target.socket, PathBuf::from(format!("{}/kmux/kmux.sock", support("/h"))));
        assert_eq!(target.pinned, None);
        let pinned = resolve(choice(), &env(&[("KANNA_MUX", "kmux"), ("KMUX_SOCKET", "/k.sock"), ("KMUX_BG", "1")])).unwrap();
        assert_eq!(pinned.pinned.as_deref(), Some("KMUX_SOCKET"));
        assert!(pinned.background);
        assert_eq!(resolve(Choice { mux: Some("kanna-dev".into()), ..choice() }, &env(&[])).unwrap().backend, Backend::Kanna);
        assert_eq!(resolve(Choice { mux: Some("tmux".into()), ..choice() }, &env(&[])).unwrap_err().code, exit::UNSUPPORTED);
    }

    #[test]
    fn the_app_started_is_the_new_app_never_the_branded_kmux() {
        let target = Target { build: dev(), backend: Backend::Kanna, instance: "default".into(), socket: "/tmp/d/kanna.sock".into(), pinned: None, background: true, fresh: false };
        let args = |target: &Target, env: &dyn Fn(&str) -> Option<String>| -> Vec<String> {
            launch_command(target, env).get_args().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        let none = args(&target, &|_| None);
        assert_eq!(none, ["-n", "-g", "--env", "KANNA_SOCKET=/tmp/d/kanna.sock", "-a", "/r/kanna/target/Kanna.app", "--args", "--bg"]);
        let staging = Target { build: Build { channel: Channel::Staging, ..dev() }, ..target.clone() };
        assert_eq!(args(&staging, &|_| None)[4..6], ["-b", "dev.kanna.app.staging"]);
        let production = Target { build: Build { channel: Channel::Production, ..dev() }, ..target.clone() };
        assert_eq!(args(&production, &|_| None)[4..6], ["-b", "dev.kanna.app"]);
        assert!(!none.iter().any(|a| a == "dev.kanna.kanna"));
        let app = args(&Target { background: false, fresh: true, ..target.clone() }, &|name| match name {
            "KANNA_APP" => Some("/x/Kanna.app".into()),
            "KANNA_STATE_DIR" => Some("/tmp/d".into()),
            "PWD" => Some("/work/app".into()),
            _ => None,
        });
        assert_eq!(app, ["-n", "--env", "KANNA_SOCKET=/tmp/d/kanna.sock", "--env", "KANNA_STATE_DIR=/tmp/d", "--env", "KANNA_START_DIR=/work/app", "-a", "/x/Kanna.app", "--args", "--fresh"]);
    }

    #[test]
    fn instances_are_the_folders_of_every_channel() {
        let home = std::env::temp_dir().join(format!("kanna-instances-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let home_text = home.to_string_lossy().into_owned();
        let env = move |name: &str| (name == "HOME").then(|| home_text.clone());
        let support = support_folder(&env);
        for folder in ["kanna-dev", "kanna-dev-work", "kanna-dev-b", "kanna-devx", "kanna-dev-bad name", "kanna", "kanna-prod", "kanna-staging-s"] {
            std::fs::create_dir_all(support.join(folder)).unwrap();
        }
        std::fs::write(support.join("kanna-dev-b").join(kanna_build::RECORD), r#"{"channel":"dev","worktree":"/r/kanna/.kanna-worktrees/b"}"#).unwrap();
        let found: Vec<String> = instances(&env).iter().map(|i| format!("{} {} {:?}", i.channel.id(), i.name, i.worktree)).collect();
        assert_eq!(found, ["production default None", "staging s None", "dev default None", "dev b Some(\"/r/kanna/.kanna-worktrees/b\")", "dev work None"]);
        let _ = std::fs::remove_dir_all(&home);
    }
}
