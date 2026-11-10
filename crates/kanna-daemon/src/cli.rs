use kanna_build::{Build, Channel};
use std::path::{Path, PathBuf};

pub const VAR: &str = "KANNA_CLI";

pub const LINK: &str = ".local/bin/kanna";

pub fn beside(daemon: &Path) -> Option<PathBuf> {
    let cli = daemon.parent()?.join("kanna");
    cli.is_file().then_some(cli)
}

pub fn env(cli: Option<&Path>) -> Option<(String, String)> {
    cli.map(|cli| (VAR.to_string(), cli.to_string_lossy().into_owned()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Owner {
    Other,
    Dev,
    Staging,
    Production,
}

impl Owner {
    pub fn of(build: &Build) -> Owner {
        match (build.channel, build.instance()) {
            (_, Some(_)) => Owner::Other,
            (Channel::Production, None) => Owner::Production,
            (Channel::Staging, None) => Owner::Staging,
            (Channel::Dev, None) => Owner::Dev,
        }
    }

    pub fn of_cli(cli: &Path) -> Owner {
        let plist = cli.parent().filter(|helpers| helpers.file_name().is_some_and(|n| n == "Helpers")).and_then(Path::parent).map(|contents| contents.join("Info.plist"));
        let Some(plist) = plist.and_then(|file| std::fs::read_to_string(file).ok()) else { return Owner::Other };
        let instance = plist_string(&plist, "KannaInstance").unwrap_or_default();
        if !instance.is_empty() && instance != kanna_build::DEFAULT_INSTANCE {
            return Owner::Other;
        }
        match plist_string(&plist, "KannaChannel").as_deref().and_then(Channel::parse) {
            Some(Channel::Production) => Owner::Production,
            Some(Channel::Staging) => Owner::Staging,
            Some(Channel::Dev) => Owner::Dev,
            None => Owner::Other,
        }
    }
}

fn plist_string(plist: &str, key: &str) -> Option<String> {
    let rest = plist.split_once(&format!("<key>{key}</key>"))?.1.trim_start();
    let value = rest.strip_prefix("<string>")?.split_once("</string>")?.0;
    Some(value.to_string())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    Linked,
    Mine,
    Higher(PathBuf),
    NotALink,
    NotOffered,
    Failed(String),
}

pub fn claim(home: &Path, cli: &Path, owner: Owner) -> Claim {
    if owner == Owner::Other {
        return Claim::NotOffered;
    }
    let link = home.join(LINK);
    match std::fs::symlink_metadata(&link) {
        Ok(meta) if !meta.file_type().is_symlink() => return Claim::NotALink,
        Ok(_) => {
            if std::fs::read_link(&link).ok().as_deref() == Some(cli) {
                return Claim::Mine;
            }
            if let Ok(target) = std::fs::canonicalize(&link) {
                if Owner::of_cli(&target) > owner {
                    return Claim::Higher(target);
                }
            }
        }
        Err(_) => {}
    }
    match point(&link, cli) {
        Ok(()) => Claim::Linked,
        Err(error) => Claim::Failed(format!("{}: {error}", link.display())),
    }
}

fn point(link: &Path, cli: &Path) -> std::io::Result<()> {
    let dir = link.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(".kanna.{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    std::os::unix::fs::symlink(cli, &temporary)?;
    std::fs::rename(&temporary, link).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Home {
            let dir = std::env::temp_dir().join(format!("kanna-cli-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Home(dir.canonicalize().unwrap())
        }

        fn app(&self, name: &str, channel: &str, instance: &str) -> PathBuf {
            let contents = self.0.join(format!("{name}.app/Contents"));
            fs::create_dir_all(contents.join("Helpers")).unwrap();
            fs::write(contents.join("Info.plist"), format!("<plist version=\"1.0\"><dict>\n<key>KannaChannel</key><string>{channel}</string>\n<key>KannaInstance</key><string>{instance}</string>\n</dict></plist>\n")).unwrap();
            for binary in ["kanna", "kanna-daemon"] {
                fs::write(contents.join("Helpers").join(binary), "").unwrap();
            }
            contents.join("Helpers/kanna")
        }

        fn link(&self) -> PathBuf {
            self.0.join(LINK)
        }

        fn linked(&self, to: &Path) {
            fs::create_dir_all(self.link().parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(to, self.link()).unwrap();
        }

        fn linked_again(&self, to: &Path) {
            fs::remove_file(self.link()).unwrap();
            self.linked(to);
        }

        fn target(&self) -> PathBuf {
            fs::read_link(self.link()).unwrap()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn build(channel: Channel, worktree: Option<&str>) -> Build {
        Build { channel, checkout: PathBuf::from("/r/kanna"), worktree: worktree.map(String::from) }
    }

    #[test]
    fn a_terminal_is_told_the_kanna_beside_its_daemon() {
        let home = Home::new("beside");
        let cli = home.app("Kanna", "production", "default");
        let found = beside(&cli.with_file_name("kanna-daemon"));
        assert_eq!(found.as_deref(), Some(cli.as_path()));
        assert_eq!(env(found.as_deref()), Some((VAR.to_string(), cli.to_string_lossy().into_owned())));
        fs::remove_file(&cli).unwrap();
        assert_eq!(env(beside(&cli.with_file_name("kanna-daemon")).as_deref()), None, "a daemon with no kanna beside it names none");
    }

    #[test]
    fn a_build_ranks_by_its_channel_and_a_worktrees_last() {
        assert!(Owner::of(&build(Channel::Production, None)) > Owner::of(&build(Channel::Staging, None)));
        assert!(Owner::of(&build(Channel::Staging, None)) > Owner::of(&build(Channel::Dev, None)));
        assert_eq!(Owner::of(&build(Channel::Dev, Some("task-1"))), Owner::Other);
        let home = Home::new("ranks");
        assert_eq!(Owner::of_cli(&home.app("Kanna", "production", "default")), Owner::Production);
        assert_eq!(Owner::of_cli(&home.app("Kanna Staging", "staging", "default")), Owner::Staging);
        assert_eq!(Owner::of_cli(&home.app("Dev", "dev", "default")), Owner::Dev);
        assert_eq!(Owner::of_cli(&home.app("Task", "dev", "task-1")), Owner::Other);
        fs::write(home.0.join("kanna"), "").unwrap();
        assert_eq!(Owner::of_cli(&home.0.join("kanna")), Owner::Other, "a kanna outside an app");
    }

    #[test]
    fn with_no_link_the_app_installs_one() {
        let home = Home::new("none");
        let staging = home.app("Kanna Staging", "staging", "default");
        assert_eq!(claim(&home.0, &staging, Owner::Staging), Claim::Linked);
        assert_eq!(home.target(), staging);
        assert_eq!(claim(&home.0, &staging, Owner::Staging), Claim::Mine);
    }

    #[test]
    fn a_lower_channels_link_is_repointed() {
        let home = Home::new("lower");
        let (dev, staging) = (home.app("Dev", "dev", "default"), home.app("Kanna Staging", "staging", "default"));
        home.linked(&dev);
        assert_eq!(claim(&home.0, &staging, Owner::Staging), Claim::Linked);
        assert_eq!(home.target(), staging);
        let moved = home.app("Elsewhere/Kanna Staging", "staging", "default");
        assert_eq!(claim(&home.0, &moved, Owner::Staging), Claim::Linked, "the same channel's app at another path");
        assert_eq!(home.target(), moved);
        home.linked_again(&home.app("Task", "dev", "task-1"));
        assert_eq!(claim(&home.0, &dev, Owner::Dev), Claim::Linked, "a worktree's build, linked by hand, yields to Dev");
        assert_eq!(home.target(), dev);
    }

    #[test]
    fn a_higher_channels_link_is_kept() {
        let home = Home::new("higher");
        let (production, staging, dev) = (home.app("Kanna", "production", "default"), home.app("Kanna Staging", "staging", "default"), home.app("Dev", "dev", "default"));
        home.linked(&production);
        assert_eq!(claim(&home.0, &staging, Owner::Staging), Claim::Higher(production.clone()));
        assert_eq!(claim(&home.0, &dev, Owner::Dev), Claim::Higher(production.clone()));
        assert_eq!(home.target(), production);
    }

    #[test]
    fn a_dangling_link_is_repointed() {
        let home = Home::new("dangling");
        let (production, dev) = (home.app("Kanna", "production", "default"), home.app("Dev", "dev", "default"));
        home.linked(&production);
        fs::remove_dir_all(home.0.join("Kanna.app")).unwrap();
        assert_eq!(claim(&home.0, &dev, Owner::Dev), Claim::Linked);
        assert_eq!(home.target(), dev);
    }

    #[test]
    fn a_worktrees_dev_build_never_takes_the_link() {
        let home = Home::new("worktree");
        let task = home.app("Task", "dev", "task-1");
        let owner = Owner::of(&build(Channel::Dev, Some("task-1")));
        assert_eq!(claim(&home.0, &task, owner), Claim::NotOffered);
        assert!(fs::symlink_metadata(home.link()).is_err(), "no link is made");
        let dev = home.app("Dev", "dev", "default");
        home.linked(&dev);
        fs::remove_dir_all(home.0.join("Dev.app")).unwrap();
        assert_eq!(claim(&home.0, &task, owner), Claim::NotOffered);
        assert_eq!(home.target(), dev, "even a dangling one is left");
    }

    #[test]
    fn a_file_that_isnt_a_link_is_left_alone() {
        let home = Home::new("file");
        let staging = home.app("Kanna Staging", "staging", "default");
        fs::create_dir_all(home.link().parent().unwrap()).unwrap();
        fs::write(home.link(), "mine").unwrap();
        assert_eq!(claim(&home.0, &staging, Owner::Staging), Claim::NotALink);
        assert_eq!(fs::read_to_string(home.link()).unwrap(), "mine");
    }
}
