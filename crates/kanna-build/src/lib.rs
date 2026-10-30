use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const DEFAULT_INSTANCE: &str = "default";

pub const SOCKET: &str = "kanna.sock";

pub const RECORD: &str = "instance.json";

pub const DAEMON: &str = "kanna-daemon";

pub const VERSION: &str = env!("KANNA_BUILD_VERSION");

pub const PROTOCOLS: [u32; 2] = [2, 2];

pub const COMMIT: &str = env!("KANNA_BUILD_COMMIT");

pub fn build_id(binary: &Path) -> Option<String> {
    let mut file = std::fs::File::open(binary).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(_) => return None,
        }
    }
    Some(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn own_build_id() -> Option<String> {
    build_id(&std::env::current_exe().ok()?)
}

pub fn sibling_daemon_build_id() -> Option<String> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    build_id(&exe.parent()?.join(DAEMON))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Production,
    Staging,
    Dev,
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Production, Channel::Staging, Channel::Dev];

    pub fn parse(id: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.id() == id)
    }

    pub const fn id(self) -> &'static str {
        match self {
            Channel::Production => "production",
            Channel::Staging => "staging",
            Channel::Dev => "dev",
        }
    }

    pub const fn folder(self) -> &'static str {
        match self {
            Channel::Production => "kanna-prod",
            Channel::Staging => "kanna-staging",
            Channel::Dev => "kanna-dev",
        }
    }

    pub const fn bundle_id(self) -> &'static str {
        match self {
            Channel::Production => "dev.kanna.app",
            Channel::Staging => "dev.kanna.app.staging",
            Channel::Dev => "dev.kanna.app.dev",
        }
    }

    pub const fn app_name(self) -> &'static str {
        match self {
            Channel::Production => "Kanna",
            Channel::Staging => "Kanna Staging",
            Channel::Dev => "Kanna Dev",
        }
    }

    pub fn instance_folder(self, name: &str) -> String {
        if name == DEFAULT_INSTANCE {
            self.folder().to_string()
        } else {
            format!("{}-{name}", self.folder())
        }
    }
}

pub fn parse_folder(folder: &str) -> Option<(Channel, String)> {
    Channel::ALL.into_iter().find_map(|channel| {
        if folder == channel.folder() {
            return Some((channel, DEFAULT_INSTANCE.to_string()));
        }
        let name = folder.strip_prefix(channel.folder())?.strip_prefix('-')?;
        valid_instance(name).then(|| (channel, name.to_string()))
    })
}

pub fn valid_instance(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn instance_for_worktree(folder: &str) -> String {
    let name: String = folder.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).take(32).collect();
    if name.is_empty() || name == DEFAULT_INSTANCE {
        return "worktree".into();
    }
    name
}

#[derive(Clone, Debug, PartialEq)]
pub struct Build {
    pub channel: Channel,
    pub checkout: PathBuf,
    pub worktree: Option<String>,
}

impl Build {
    pub fn current() -> Build {
        let worktree = env!("KANNA_BUILD_WORKTREE");
        Build {
            channel: Channel::parse(env!("KANNA_BUILD_CHANNEL")).unwrap_or(Channel::Dev),
            checkout: PathBuf::from(env!("KANNA_BUILD_CHECKOUT")),
            worktree: (!worktree.is_empty()).then(|| worktree.to_string()),
        }
    }

    pub fn instance(&self) -> Option<String> {
        match (self.channel, &self.worktree) {
            (Channel::Dev, Some(folder)) => Some(instance_for_worktree(folder)),
            _ => None,
        }
    }

    pub fn default_instance(&self) -> String {
        self.instance().unwrap_or_else(|| DEFAULT_INSTANCE.to_string())
    }

    pub fn app_name(&self) -> String {
        match self.instance() {
            Some(name) => format!("{} {name}", self.channel.app_name()),
            None => self.channel.app_name().to_string(),
        }
    }

    pub fn bundle_id(&self) -> String {
        match self.instance() {
            Some(name) => format!("{}.{}", self.channel.bundle_id(), name.to_ascii_lowercase().replace('_', "-")),
            None => self.channel.bundle_id().to_string(),
        }
    }

    pub fn app(&self) -> Option<PathBuf> {
        (self.channel == Channel::Dev).then(|| self.checkout.join("target/Kanna.app"))
    }

    pub fn instance_folder(&self, support: &Path, name: &str) -> PathBuf {
        support.join(self.channel.instance_folder(name))
    }

    pub fn record(&self, app: Option<&Path>) -> Value {
        json!({
            "channel": self.channel.id(),
            "name": self.app_name(),
            "bundle": self.bundle_id(),
            "checkout": self.checkout,
            "worktree": self.instance().map(|_| &self.checkout),
            "app": app,
        })
    }

    pub fn shell(&self) -> String {
        [
            ("channel", self.channel.id().to_string()),
            ("name", self.app_name()),
            ("bundle_id", self.bundle_id()),
            ("instance", self.default_instance()),
            ("folder", self.channel.folder().to_string()),
            ("version", VERSION.to_string()),
            ("commit", COMMIT.to_string()),
        ]
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect()
    }
}

pub fn read_record(folder: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(folder.join(RECORD)).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_speaks_the_daemons_protocols() {
        let swift = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/kanna/Sources/KannaMachines/Protocols.swift")).unwrap();
        assert!(swift.contains(&format!("public static let mine: [Int] = [{}, {}]", PROTOCOLS[0], PROTOCOLS[1])), "Protocols.swift's mine isn't {PROTOCOLS:?}");
    }

    fn build(channel: Channel, worktree: Option<&str>) -> Build {
        Build { channel, checkout: PathBuf::from("/r/kanna"), worktree: worktree.map(String::from) }
    }

    #[test]
    fn a_build_id_is_the_binarys_sha256() {
        let file = std::env::temp_dir().join(format!("kanna-build-id-{}", std::process::id()));
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(build_id(&file).as_deref(), Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        std::fs::remove_file(&file).unwrap();
        assert_eq!(build_id(&file), None);
    }

    #[test]
    fn each_channel_is_a_separate_app() {
        let names: Vec<(String, String, String)> = Channel::ALL.iter().map(|c| build(*c, None)).map(|b| (b.app_name(), b.bundle_id(), b.default_instance())).collect();
        assert_eq!(
            names,
            [
                ("Kanna".into(), "dev.kanna.app".into(), "default".into()),
                ("Kanna Staging".into(), "dev.kanna.app.staging".into(), "default".into()),
                ("Kanna Dev".into(), "dev.kanna.app.dev".into(), "default".into()),
            ]
        );
        assert_eq!(build(Channel::Dev, None).app(), Some(PathBuf::from("/r/kanna/target/Kanna.app")));
        assert_eq!(build(Channel::Production, None).app(), None);
    }

    #[test]
    fn a_dev_build_in_a_worktree_is_its_own_app_and_instance() {
        let dev = build(Channel::Dev, Some("task-3F9a_1c2e"));
        assert_eq!(dev.app_name(), "Kanna Dev task-3F9a_1c2e");
        assert_eq!(dev.bundle_id(), "dev.kanna.app.dev.task-3f9a-1c2e");
        assert_eq!(dev.default_instance(), "task-3F9a_1c2e");
        assert_eq!(dev.instance_folder(Path::new("/s"), &dev.default_instance()), PathBuf::from("/s/kanna-dev-task-3F9a_1c2e"));
        let production = build(Channel::Production, Some("task-1"));
        assert_eq!((production.bundle_id().as_str(), production.default_instance().as_str()), ("dev.kanna.app", "default"));
        assert_eq!(dev.record(None)["worktree"], "/r/kanna");
        assert_eq!(production.record(None)["worktree"], Value::Null);
        assert_eq!(instance_for_worktree("my feature.x"), "my-feature-x");
        assert_eq!(instance_for_worktree("default"), "worktree");
        assert_eq!(instance_for_worktree(&"a".repeat(40)).len(), 32);
    }

    #[test]
    fn folders_name_their_channel_and_instance() {
        assert_eq!(parse_folder("kanna-prod"), Some((Channel::Production, "default".into())));
        assert_eq!(parse_folder("kanna-staging-work"), Some((Channel::Staging, "work".into())));
        assert_eq!(parse_folder("kanna-dev-task-3f9a1c2e"), Some((Channel::Dev, "task-3f9a1c2e".into())));
        for other in ["kanna", "Kanna", "kanna-devx", "kanna-dev-bad name", "kanna-dev-", "kanna3"] {
            assert_eq!(parse_folder(other), None, "{other}");
        }
        for channel in Channel::ALL {
            assert_eq!(parse_folder(&channel.instance_folder("x")), Some((channel, "x".into())));
        }
    }
}
