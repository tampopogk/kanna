use crate::db::{self, Db};
use kanna_mux::json::{json, num, string, Map, Value};
use kanna_mux::{project, CoreHandle, KmuxError};
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct Projects {
    db: Rc<Db>,
    home: Option<PathBuf>,
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn failed(what: &str, error: rusqlite::Error) -> KmuxError {
    eprintln!("kanna-daemon: couldn't {what} in {}: {error}", db::FILE);
    KmuxError::new("failed", format!("couldn't {what}: {error}"))
}

impl Projects {
    pub fn new(db: Rc<Db>, home: Option<PathBuf>) -> Projects {
        Projects { db, home: home.map(|h| h.canonicalize().unwrap_or(h)) }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let this = Rc::downgrade(self);
        core.add_command(
            "projects",
            Rc::new(move |args, responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                responder.reply(this.handle(&args))
            }),
        );
    }

    pub fn handle(&self, args: &Value) -> Result<Map, KmuxError> {
        match args.get("action").and_then(Value::as_str).unwrap_or("list") {
            "list" => self.list(),
            "touch" => self.touch(args),
            "clear" => {
                self.db.clear_projects().map_err(|e| failed("clear the recent projects", e))?;
                Ok(Map::from_iter([("projects".to_string(), json!([]))]))
            }
            other => Err(bad(format!("unknown projects action \"{other}\" (list, touch or clear)"))),
        }
    }

    fn touch(&self, args: &Value) -> Result<Map, KmuxError> {
        let folder = string(args, "folder").filter(|f| Path::new(f).is_absolute()).ok_or_else(|| bad("say which folder: an absolute path"))?;
        let root = project(Some(folder)).filter(|root| root.is_dir()).ok_or_else(|| KmuxError::new("not_found", format!("no folder {folder}")))?;
        let mut reply = Map::new();
        if Some(&root) == self.home.as_ref() || root.parent().is_none() {
            reply.insert("project".into(), Value::Null);
            return Ok(reply);
        }
        let path = root.to_string_lossy();
        self.db.touch_project(&path).map_err(|e| failed("keep a recent project", e))?;
        reply.insert("project".into(), json!(path));
        Ok(reply)
    }

    fn list(&self) -> Result<Map, KmuxError> {
        let mut kept = vec![];
        for (path, at) in self.db.projects().map_err(|e| failed("read the recent projects", e))? {
            if Path::new(&path).is_dir() {
                kept.push(json!({ "path": path, "at": num(at.floor()) }));
            } else {
                self.db.forget_project(&path).map_err(|e| failed("forget a recent project", e))?;
            }
        }
        Ok(Map::from_iter([("projects".to_string(), Value::Array(kept))]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-projects-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("state")).unwrap();
        dir.canonicalize().unwrap()
    }

    fn listed(projects: &Projects) -> Vec<String> {
        let reply = Value::Object(projects.handle(&json!({})).unwrap());
        reply["projects"].as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap().to_string()).collect()
    }

    fn touch(projects: &Projects, folder: &Path) -> Value {
        Value::Object(projects.handle(&json!({ "action": "touch", "folder": folder })).unwrap())["project"].clone()
    }

    fn git(dir: &Path, args: &[&str]) {
        assert!(Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(dir).status().unwrap().success());
    }

    #[test]
    fn the_latest_project_comes_first_and_ten_are_kept() {
        let dir = folder("order");
        let projects = Projects::new(Rc::new(Db::open(&dir.join("state"))), Some(dir.join("home")));
        let made: Vec<PathBuf> = (0..12).map(|n| dir.join(format!("p{n}"))).collect();
        for p in &made {
            fs::create_dir_all(p).unwrap();
        }
        for p in &made[..3] {
            touch(&projects, p);
        }
        touch(&projects, &made[0]);
        assert_eq!(listed(&projects), [&made[0], &made[2], &made[1]].map(|p| p.to_string_lossy().to_string()), "touching one again moves it to the top");
        for p in &made[3..] {
            touch(&projects, p);
        }
        let all = listed(&projects);
        assert_eq!(all.len(), db::RECENT);
        assert_eq!((all[0].clone(), all[9].clone()), (made[11].to_string_lossy().to_string(), made[0].to_string_lossy().to_string()), "the oldest, p1 and p2, fall off");
        projects.handle(&json!({ "action": "clear" })).unwrap();
        assert!(listed(&projects).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_that_is_gone_drops_off_and_home_is_not_a_project() {
        let dir = folder("gone");
        fs::create_dir_all(dir.join("home")).unwrap();
        let projects = Projects::new(Rc::new(Db::open(&dir.join("state"))), Some(dir.join("home")));
        let (kept, gone) = (dir.join("kept"), dir.join("gone"));
        fs::create_dir_all(&kept).unwrap();
        fs::create_dir_all(&gone).unwrap();
        touch(&projects, &kept);
        touch(&projects, &gone);
        fs::remove_dir_all(&gone).unwrap();
        assert_eq!(listed(&projects), [kept.to_string_lossy().to_string()]);
        fs::create_dir_all(&gone).unwrap();
        assert_eq!(listed(&projects).len(), 1, "it was forgotten, not hidden");
        assert_eq!(touch(&projects, &dir.join("home")), Value::Null);
        assert!(projects.handle(&json!({ "action": "touch", "folder": dir.join("nowhere") })).is_err());
        assert_eq!(listed(&projects).len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_worktree_or_a_subfolder_is_its_repo() {
        let dir = folder("worktree");
        let projects = Projects::new(Rc::new(Db::open(&dir.join("state"))), None);
        let repo = dir.join("app");
        fs::create_dir_all(repo.join("src")).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(&repo, &["worktree", "add", "-q", ".kanna-worktrees/task-1", "-b", "task-1"]);
        let path = repo.to_string_lossy().to_string();
        assert_eq!(touch(&projects, &repo.join(".kanna-worktrees/task-1")), json!(path));
        assert_eq!(touch(&projects, &repo.join("src")), json!(path));
        assert_eq!(listed(&projects), [path]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
