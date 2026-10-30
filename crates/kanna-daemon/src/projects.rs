use crate::db::{self, Db};
use kanna_mux::json::{json, num, string, Map, Value};
use kanna_mux::{project, CoreHandle, KmuxError};
use std::path::{Path, PathBuf};
use std::rc::Rc;

thread_local! {
    static FIRST_COMMITS: std::cell::RefCell<std::collections::HashMap<PathBuf, String>> = std::cell::RefCell::default();
}

pub fn identity(main: &Path) -> Option<String> {
    if let Some(url) = origin(main) {
        return Some(format!("url:{url}"));
    }
    let known = FIRST_COMMITS.with(|ids| ids.borrow().get(main).cloned());
    let sha = known.or_else(|| {
        let found = first_commit(main)?;
        FIRST_COMMITS.with(|ids| ids.borrow_mut().insert(main.to_path_buf(), found.clone()));
        Some(found)
    })?;
    Some(format!("commit:{sha}"))
}

fn origin(main: &Path) -> Option<String> {
    normalized(&git_origin(main)?)
}

pub fn normalized(url: &str) -> Option<String> {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, url),
    };
    if scheme == Some("file") || (scheme.is_none() && (rest.starts_with('/') || rest.starts_with('.') || !rest.contains(':'))) {
        return None;
    }
    let rest = rest.rsplit_once('@').filter(|(user, _)| !user.contains('/')).map_or(rest, |(_, host)| host);
    let (host, path) = match scheme {
        Some(_) => rest.split_once('/')?,
        None => rest.split_once(':')?,
    };
    let host = host.split(':').next().unwrap_or(host).to_lowercase();
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    (!host.is_empty() && !path.is_empty()).then(|| format!("{host}/{path}"))
}

pub const REPOS: &str = ".kanna-v4/repos";

pub const LOCAL: &str = "local";

pub fn repos() -> PathBuf {
    crate::agents::home().join(REPOS)
}

fn segment(part: &str) -> bool {
    !part.is_empty() && part != "." && part != ".." && !part.starts_with('.') && part.chars().all(|c| !c.is_control() && c != '\\')
}

pub fn place_of(url: Option<&str>, name: &str) -> PathBuf {
    let local = || Path::new(LOCAL).join(name);
    let Some(remote) = url.and_then(normalized) else { return local() };
    let Some((host, path)) = remote.split_once('/') else { return local() };
    let parts: Vec<&str> = path.split('/').collect();
    if !parts.iter().all(|p| segment(p)) {
        return local();
    }
    match parts.as_slice() {
        [only] if segment(host) => Path::new(host).join(only),
        [_] => local(),
        _ => parts.iter().collect(),
    }
}

pub fn home_place(main: &Path) -> PathBuf {
    let name = main.file_name().map_or_else(|| "project".to_string(), |n| n.to_string_lossy().into_owned());
    let url = git_origin(main);
    place_of(url.as_deref(), &name)
}

fn git_origin(main: &Path) -> Option<String> {
    let config = std::fs::read_to_string(main.join(".git/config")).ok()?;
    let mut inside = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line.replace(' ', "") == "[remote\"origin\"]";
        } else if let Some(url) = line.strip_prefix("url").map(str::trim_start).and_then(|rest| rest.strip_prefix('=')).filter(|_| inside) {
            return Some(url.trim().to_string());
        }
    }
    None
}

pub fn tilde(path: &Path) -> String {
    let home = crate::agents::home();
    let home = home.canonicalize().unwrap_or(home);
    match path.strip_prefix(&home) {
        Ok(inside) if inside.as_os_str().is_empty() => "~".into(),
        Ok(inside) => format!("~/{}", inside.display()),
        Err(_) => path.display().to_string(),
    }
}

pub fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub fn misplaced(repos: &Path, folder: &Path) -> Option<(PathBuf, PathBuf)> {
    let main = project(folder.to_str())?;
    let wanted = canonical(repos).join(home_place(&main));
    (main != wanted).then_some((main, wanted))
}

pub fn outside(repos: &Path, folder: &Path) -> Result<(), KmuxError> {
    let Some((main, wanted)) = misplaced(repos, folder) else { return Ok(()) };
    let (shown, root) = (tilde(&main), canonical(repos));
    let why = match main.starts_with(&root) {
        true => format!("{shown} isn't at its place in {}, which its origin decides", tilde(&root)),
        false => format!("{shown} isn't in {}, where Kanna keeps its projects so each has the same place on every machine", tilde(&root)),
    };
    Err(KmuxError::new("outside", format!("{why}: import a copy there with `kanna project import {shown}` (to {})", tilde(&wanted))))
}

pub fn desk_here(folder: &str) -> Result<(), KmuxError> {
    let here = canonical(Path::new(folder));
    if [crate::agents::home()].into_iter().chain(std::env::var_os("HOME").map(PathBuf::from)).any(|home| canonical(&home) == here) {
        return Ok(());
    }
    outside(&repos(), Path::new(folder))
}

fn first_commit(main: &Path) -> Option<String> {
    let out = std::process::Command::new("git").arg("-C").arg(main).args(["rev-list", "--max-parents=0", "HEAD"]).stderr(std::process::Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut roots: Vec<&str> = text.lines().filter(|l| l.len() >= 40).collect();
    roots.sort();
    roots.first().map(|r| r.to_string())
}

pub fn place(cwd: &str) -> Value {
    let Some(main) = project(Some(cwd)) else { return Value::Null };
    let resolved = std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd));
    let Some(root) = resolved.ancestors().find(|dir| dir.join(".git").exists()) else { return json!({ "main": main, "root": null, "branch": null, "project": null }) };
    json!({ "main": main, "root": root, "branch": branch(root), "project": identity(&main) })
}

fn branch(root: &Path) -> Option<String> {
    let marker = root.join(".git");
    let dir = if marker.is_dir() {
        marker
    } else {
        let text = std::fs::read_to_string(&marker).ok().filter(|t| t.len() <= 4096)?;
        root.join(text.lines().next()?.strip_prefix("gitdir: ")?.trim())
    };
    let head = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(name) => Some(name.to_string()),
        None => (head.len() >= 40 && head.bytes().all(|b| b.is_ascii_hexdigit())).then(|| head[..7].to_string()),
    }
}

pub struct Projects {
    db: Rc<Db>,
    home: Option<PathBuf>,
    repos: Option<PathBuf>,
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn failed(what: &str, error: rusqlite::Error) -> KmuxError {
    log!("kanna-daemon: couldn't {what} in {}: {error}", db::FILE);
    KmuxError::new("failed", format!("couldn't {what}: {error}"))
}

impl Projects {
    pub fn new(db: Rc<Db>, home: Option<PathBuf>, repos: Option<PathBuf>) -> Projects {
        Projects { db, home: home.map(|h| h.canonicalize().unwrap_or(h)), repos }
    }

    fn label(&self, path: &Path) -> String {
        if let Some(inside) = self.repos.as_deref().map(canonical).and_then(|repos| path.strip_prefix(repos).ok().map(Path::to_path_buf)) {
            return inside.display().to_string();
        }
        match self.home.as_ref().and_then(|home| path.strip_prefix(home).ok()) {
            Some(inside) => format!("~/{}", inside.display()),
            None => path.display().to_string(),
        }
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

    fn misplaced(&self, folder: &Path) -> bool {
        self.repos.as_deref().is_some_and(|repos| misplaced(repos, folder).is_some())
    }

    fn touch(&self, args: &Value) -> Result<Map, KmuxError> {
        let folder = string(args, "folder").filter(|f| Path::new(f).is_absolute()).ok_or_else(|| bad("say which folder: an absolute path"))?;
        let root = project(Some(folder)).filter(|root| root.is_dir()).ok_or_else(|| KmuxError::new("not_found", format!("no folder {folder}")))?;
        let mut reply = Map::new();
        if Some(&root) == self.home.as_ref() || root.parent().is_none() || self.misplaced(&root) {
            reply.insert("project".into(), Value::Null);
            return Ok(reply);
        }
        let path = root.to_string_lossy();
        self.db.touch_project(&path).map_err(|e| failed("keep a recent project", e))?;
        reply.insert("project".into(), json!(path));
        Ok(reply)
    }

    fn list(&self) -> Result<Map, KmuxError> {
        let (mut kept, mut outside) = (vec![], vec![]);
        for (path, at) in self.db.projects().map_err(|e| failed("read the recent projects", e))? {
            if !Path::new(&path).is_dir() {
                self.db.forget_project(&path).map_err(|e| failed("forget a recent project", e))?;
            } else if let Some(entry) = self.outside(&path, Some(at)) {
                outside.push(entry);
            } else {
                kept.push(json!({ "path": path, "label": self.label(Path::new(&path)), "at": num(at.floor()), "project": identity(Path::new(&path)) }));
            }
        }
        let mut repos: Vec<String> = self.db.tasks().map_err(|e| failed("read the tasks", e))?.into_iter().filter(|t| t.place != db::Place::Done).map(|t| t.repo).collect();
        repos.sort();
        repos.dedup();
        for repo in repos {
            let known = |entry: &Value| entry["path"].as_str() == Some(repo.as_str());
            if Path::new(&repo).is_dir() && !outside.iter().any(known) {
                outside.extend(self.outside(&repo, None));
            }
        }
        kept.extend(outside);
        Ok(Map::from_iter([("projects".to_string(), Value::Array(kept))]))
    }

    fn outside(&self, path: &str, at: Option<f64>) -> Option<Value> {
        let (main, wanted) = misplaced(self.repos.as_deref()?, Path::new(path))?;
        let at = at.map_or(Value::Null, |at| num(at.floor()));
        Some(json!({ "path": main, "label": format!("{} (outside)", self.label(&main)), "at": at, "project": identity(&main), "outside": true, "place": wanted }))
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
    fn a_project_is_known_by_its_origin_wherever_it_is_cloned() {
        for url in ["git@github.com:tampopogk/kanna.git", "https://github.com/tampopogk/kanna", "ssh://git@GitHub.com:22/tampopogk/kanna.git/", "https://token@github.com/tampopogk/kanna.git"] {
            assert_eq!(normalized(url).as_deref(), Some("github.com/tampopogk/kanna"), "{url}");
        }
        for url in ["/srv/git/kanna", "../kanna", "file:///srv/git/kanna"] {
            assert_eq!(normalized(url), None, "{url}");
        }
        let dir = folder("identity");
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        assert_eq!(identity(&repo), None, "a repo with no commit and no origin has no identity yet");
        git(&repo, &["-c", "user.name=k", "-c", "user.email=k@k", "commit", "-q", "--allow-empty", "-m", "first"]);
        let commit = identity(&repo).unwrap();
        assert!(commit.starts_with("commit:") && commit.len() == 47, "{commit}");
        git(&repo, &["worktree", "add", "-q", "-b", "task-1", dir.join("wt").to_str().unwrap()]);
        let found = place(dir.join("wt").to_str().unwrap());
        assert_eq!((found["branch"].clone(), found["project"].clone(), found["main"].clone()), (json!("task-1"), json!(commit), json!(repo.canonicalize().unwrap())));
        git(&repo, &["remote", "add", "origin", "git@github.com:tampopogk/kanna.git"]);
        assert_eq!(identity(&repo).as_deref(), Some("url:github.com/tampopogk/kanna"), "an origin added later is the identity");
    }

    #[test]
    fn a_project_s_place_in_repos_is_its_origin_s_owner_and_name() {
        for url in ["https://github.com/tampopogk/kanna.git", "https://github.com/tampopogk/kanna", "ssh://git@github.com:22/tampopogk/kanna.git", "git@github.com:tampopogk/kanna.git", "git@gitlab.example.com:tampopogk/kanna", "https://token@github.com/tampopogk/kanna/"] {
            assert_eq!(place_of(Some(url), "folder"), Path::new("tampopogk/kanna"), "{url}");
        }
        assert_eq!(place_of(Some("https://gitlab.com/group/sub/app.git"), "app"), Path::new("group/sub/app"), "a nested group keeps its path");
        assert_eq!(place_of(Some("git@myserver:app.git"), "app"), Path::new("myserver/app"), "a remote with no owner goes under its host");
        for url in [None, Some("/srv/git/app"), Some("../app"), Some("file:///srv/git/app"), Some("https://github.com/../app"), Some("https://github.com/a/.hidden")] {
            assert_eq!(place_of(url, "app"), Path::new("local/app"), "{url:?}");
        }
        let dir = folder("place");
        let repos = dir.join("home").join(REPOS);
        let app = dir.join("work/app");
        fs::create_dir_all(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        assert_eq!(home_place(&app), Path::new("local/app"));
        let refused = outside(&repos, &app.join("sub")).unwrap_err();
        assert_eq!(refused.code, "outside");
        assert!(refused.message.contains(&format!("import a copy there with `kanna project import {}`", app.display())) && refused.message.contains("repos/local/app)"), "{}", refused.message);
        git(&app, &["remote", "add", "origin", "git@github.com:someone/app.git"]);
        assert_eq!(misplaced(&repos, &app).unwrap().1, repos.join("someone/app"));
        let there = repos.join("someone/app");
        fs::create_dir_all(there.parent().unwrap()).unwrap();
        fs::rename(&app, &there).unwrap();
        assert!(outside(&repos, &there).is_ok() && outside(&repos, &there.join("src")).is_ok());
        git(&there, &["worktree", "add", "-q", "-b", "task-1", ".kanna-worktrees/task-1"]);
        assert!(outside(&repos, &there.join(".kanna-worktrees/task-1")).is_ok(), "a worktree is its repo");
        git(&there, &["remote", "set-url", "origin", "https://github.com/other/app"]);
        assert_eq!(outside(&repos, &there).unwrap_err().code, "outside", "a new origin is a new place");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_latest_project_comes_first_and_ten_are_kept() {
        let dir = folder("order");
        let projects = Projects::new(Rc::new(Db::open(&dir.join("state"))), Some(dir.join("home")), None);
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
        let projects = Projects::new(Rc::new(Db::open(&dir.join("state"))), Some(dir.join("home")), None);
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
    fn a_project_in_repos_goes_by_its_place_and_one_outside_with_work_in_kanna_comes_last_marked_outside() {
        let dir = folder("labels");
        let home = dir.join("home");
        let repos = home.join(REPOS);
        let projects = Projects::new(Rc::new(Db::open(&dir.join("state"))), Some(home.clone()), Some(repos.clone()));
        for folder in [repos.join("local/kojo"), repos.join("local/recipe-box"), home.join("work/app"), dir.join("elsewhere"), repos.join("local/kojo/sub")] {
            fs::create_dir_all(&folder).unwrap();
            touch(&projects, &folder);
        }
        let reply = Value::Object(projects.handle(&json!({})).unwrap());
        let labels: Vec<&str> = reply["projects"].as_array().unwrap().iter().map(|p| p["label"].as_str().unwrap()).collect();
        assert_eq!(labels, ["local/recipe-box", "local/kojo"], "a subfolder of a project that isn't a repo is a project of its own, out of place");
        projects.db.touch_project(&home.join("work/app").to_string_lossy()).unwrap();
        let row = |id: &str, repo: &Path, place: db::Place| db::Row { id: id.into(), repo: repo.to_string_lossy().into(), place, terminal: None, made_at: 1.0, doc: json!({}) };
        for (id, repo, place) in [("task-00000001", dir.join("elsewhere"), db::Place::Backlog), ("task-00000002", dir.join("elsewhere"), db::Place::Shelf), ("task-00000003", home.join("work/app"), db::Place::Backlog), ("task-00000004", dir.join("finished"), db::Place::Done)] {
            fs::create_dir_all(&repo).unwrap();
            projects.db.put(&row(id, &repo, place), None).unwrap();
        }
        let reply = Value::Object(projects.handle(&json!({})).unwrap());
        let shown: Vec<(String, Value, Value)> = reply["projects"].as_array().unwrap().iter().map(|p| (p["label"].as_str().unwrap().to_string(), p["outside"].clone(), p["place"].clone())).collect();
        let elsewhere = dir.join("elsewhere").display().to_string();
        let wanted = [
            ("local/recipe-box".to_string(), Value::Null, Value::Null),
            ("local/kojo".to_string(), Value::Null, Value::Null),
            ("~/work/app (outside)".to_string(), json!(true), json!(repos.join("local/app"))),
            (format!("{elsewhere} (outside)"), json!(true), json!(repos.join("local/elsewhere"))),
        ];
        assert_eq!(shown, wanted, "an outside project with a recent entry or open tasks is listed once, after the rest, with its place; a done task's isn't");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_worktree_or_a_subfolder_is_its_repo() {
        let dir = folder("worktree");
        let projects = Projects::new(Rc::new(Db::open(&dir.join("state"))), None, None);
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
