use super::*;
use crate::agents::home;
use crate::messages::{KANNA as FROM_KANNA, PHONE};
use kanna_detect::last_words::{self, Words};
use std::io::{Read, Seek, SeekFrom};

const DONE_LATELY: usize = 5;

const WORDS_AGAIN: [Duration; 2] = [Duration::ZERO, Duration::from_secs(2)];

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: Option<String>,
    pub from: String,
    pub at: f64,
    pub text: String,
}

impl Note {
    pub fn values(notes: &[Note]) -> Value {
        Value::Array(notes.iter().map(|n| json!({ "id": n.id, "from": n.from, "at": num(n.at), "text": n.text })).collect())
    }

    pub fn list(value: Option<&Value>) -> Vec<Note> {
        value.and_then(Value::as_array).into_iter().flatten().filter_map(|n| Some(Note { id: string(n, "id").map(String::from), from: string(n, "from")?.to_string(), at: n.get("at").and_then(Value::as_f64).unwrap_or(0.0), text: string(n, "text")?.to_string() })).collect()
    }
}

fn tail(file: &Path) -> Option<String> {
    let mut file = fs::File::open(file).ok()?;
    let size = file.metadata().ok()?.len();
    let from = size.saturating_sub(last_words::READ);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = vec![];
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(match from {
        0 => text,
        _ => text.split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string()),
    })
}

fn newest_in(dir: &Path, wanted: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| wanted(p)).max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
}

fn claude_transcript(folder: &Path, session: Option<&str>) -> Option<PathBuf> {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()).map_or_else(|| home().join(".claude"), PathBuf::from);
    let real = fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    let slug: String = real.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let dir = config.join("projects").join(slug);
    let named = session.map(|id| dir.join(format!("{id}.jsonl"))).filter(|p| p.is_file());
    named.or_else(|| newest_in(&dir, |p| p.extension().is_some_and(|e| e == "jsonl")))
}

fn codex_rollout(session: &str) -> Option<PathBuf> {
    let codex = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()).map_or_else(|| home().join(".codex"), PathBuf::from);
    let mut days = vec![codex.join("sessions")];
    for _ in 0..3 {
        days = days.iter().flat_map(|dir| fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())).collect();
    }
    days.iter().find_map(|day| newest_in(day, |p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&format!("{session}.jsonl")))))
}

fn opencode_words(session: &str) -> Option<Words> {
    let data = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map_or_else(|| home().join(".local/share"), PathBuf::from);
    let db = data.join("opencode/opencode.db");
    if !db.is_file() || !session.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    let query = format!("select p.data as data from part p join message m on m.id = p.message_id where p.session_id = '{session}' and json_extract(m.data, '$.role') = 'assistant' and json_extract(p.data, '$.type') = 'text' order by p.id desc limit 1");
    let out = Command::new("/usr/bin/sqlite3").arg("-readonly").arg("-json").arg(&db).arg(query).output().ok().filter(|o| o.status.success())?;
    let rows = serde_json::from_slice::<Value>(&out.stdout).ok()?;
    let part: Value = serde_json::from_str(rows.as_array()?.first()?.get("data")?.as_str()?).ok()?;
    let text = part["text"].as_str()?.trim();
    (!text.is_empty()).then(|| Words { text: text.to_string(), asking: false })
}

fn read_words(entry: &Entry) -> Option<Words> {
    match program(&entry.agent).rsplit('/').next().unwrap_or_default() {
        "claude" => last_words::claude(&tail(&claude_transcript(Path::new(entry.folder.as_deref()?), entry.session.as_deref())?)?),
        "codex" => {
            let found = entry.session.clone().or_else(|| codex_session(Path::new(entry.folder.as_deref()?)))?;
            last_words::codex(&tail(&codex_rollout(&found)?)?)
        }
        "opencode" => {
            let found = entry.session.clone().or_else(|| opencode_session(Path::new(entry.folder.as_deref()?)))?;
            opencode_words(&found)
        }
        _ => None,
    }
}

impl Tasks {
    pub fn revision(&self) -> u64 {
        self.revision.get()
    }

    pub(super) fn revised(&self) {
        self.revision.set(self.revision.get() + 1);
    }

    fn me(&self) -> String {
        self.machines().me.name
    }

    pub fn desk_of(&self, app: &str) -> String {
        if app == MAC_APP { self.me() } else { app.to_string() }
    }

    pub fn app_of(&self, desk: &str) -> String {
        if desk == self.me() { MAC_APP.to_string() } else { desk.to_string() }
    }

    pub(super) fn claims_elsewhere(&self, entry: &Entry, app: &str, freed: Option<&str>) -> usize {
        let (desk, project) = (self.desk_of(app), identity(&entry.repo));
        let replica = self.replica.borrow();
        replica.records().filter(|r| r.task != entry.id && Some(r.task.as_str()) != freed && r.text("state") == Some("started") && r.text("owner").is_some_and(|o| o != replica.me()) && r.text("claim") == Some(desk.as_str()) && project.is_some() && r.text("project") == project.as_deref()).count()
    }

    pub(super) fn saw_status(&self, pane: &str, status: Option<Status>) {
        let Some(status) = status.map(|s| s.as_str().to_string()) else { return };
        let mut statuses = self.statuses.borrow_mut();
        if statuses.get(pane).is_none_or(|(old, _)| *old != status) {
            statuses.insert(pane.to_string(), (status, now()));
            self.revised();
        }
    }

    pub(super) fn keep_last_words(self: &Rc<Self>, pane: &str) {
        let wanted = self.running.borrow().iter().find(|r| r.pane == pane).map(|r| r.entry.clone());
        let Some(entry) = wanted.or_else(|| self.entries.borrow().iter().find(|e| e.held.as_deref() == Some(pane)).cloned()) else { return };
        self.read_last_words(entry);
    }

    fn read_last_words(self: &Rc<Self>, entry: Entry) {
        let this = Rc::downgrade(self);
        crate::runtime::spawn("a task's last words", async move {
            let id = entry.id.clone();
            for pause in WORDS_AGAIN {
                tokio::time::sleep(pause).await;
                let reading = entry.clone();
                let read = crate::runtime::off_loop(move || read_words(&reading)).await.flatten();
                let Some(this) = this.upgrade() else { return };
                match read {
                    Some(words) => match this.db.put_last_words(&id, &words.text, words.asking) {
                        Ok(()) => this.revised(),
                        Err(error) => log!("kanna-daemon: couldn't keep {id}'s last words in {}: {error}", db::FILE),
                    },
                    None if pause == WORDS_AGAIN[1] => log!("kanna-daemon: found no last words for {id} in its agent's session"),
                    None => {}
                }
            }
        });
    }

    pub(super) fn sent_already(&self, client: &str) -> Option<Map> {
        if let Ok(Some(message)) = self.db.message_of(client) {
            let to = json!({ "id": message.recipient, "short": self.short(&message.recipient) });
            return Some(Map::from_iter([("message".to_string(), json!(message.id)), ("to".to_string(), to), ("state".to_string(), json!("sent")), ("warning".to_string(), Value::Null)]));
        }
        let noted = self.entries.borrow().iter().find(|e| e.notes.iter().any(|n| n.id.as_deref() == Some(client))).map(|e| e.id.clone())?;
        Some(Map::from_iter([("message".to_string(), Value::Null), ("to".to_string(), json!({ "id": noted, "short": self.short(&noted) })), ("state".to_string(), json!("waiting")), ("warning".to_string(), Value::Null)]))
    }

    pub(super) fn unstarted(&self, id: &str) -> bool {
        let starting = self.pending.borrow().values().any(|p| p.entry.id == id) || self.launching.borrow().contains(id);
        !starting && self.entries.borrow().iter().any(|e| e.id == id && e.folder.is_none() && e.held.is_none())
    }

    pub(super) fn note_for(&self, id: &str, from: &str, text: &str, client: Option<&str>) -> Result<Map, KmuxError> {
        let Some(mut entry) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else { return Err(not_found(id)) };
        entry.notes.push(Note { id: client.map(String::from), from: from.to_string(), at: now(), text: text.to_string() });
        self.store(&entry, Place::Shelf, None, None)?;
        if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == id) {
            *found = entry;
        }
        self.changed();
        log!("kanna-daemon: a message from {from} goes with {id}'s first message");
        Ok(Map::from_iter([("message".to_string(), Value::Null), ("to".to_string(), json!({ "id": id, "short": self.short(id) })), ("state".to_string(), json!("waiting")), ("warning".to_string(), Value::Null)]))
    }

    pub(super) fn notes_listed(&self, entry: &Entry, named: &dyn Fn(&str) -> Value) -> Vec<Value> {
        let read = (entry.folder.is_some() || entry.held.is_some()).then_some(entry.made_at);
        entry.notes.iter().map(|n| json!({ "id": Value::Null, "at": num(n.at), "from": named(&n.from), "to": named(&entry.id), "repo": entry.repo, "delivered": read.map(|_| num(n.at)), "warning": Value::Null, "text": n.text })).collect()
    }

    fn last_event(&self, entry: &Entry) -> f64 {
        self.db.events(&entry.id).ok().and_then(|events| events.last().map(|e| e.at)).unwrap_or(entry.made_at)
    }

    pub fn fill_last_words(self: &Rc<Self>, task: &str) {
        if !matches!(self.db.last_words(task), Ok(None)) {
            return;
        }
        let Some(entry) = self.open_tasks().into_iter().find(|e| e.id == task && e.folder.is_some()) else { return };
        self.read_last_words(entry);
    }

    fn status_of(&self, entry: &Entry, pane: Option<&str>) -> (String, f64) {
        let core = self.core.upgrade();
        let live = pane.zip(core.as_ref()).filter(|(pane, core)| alive(core, pane)).map(|(pane, _)| pane);
        if let Some(pane) = live {
            let seen = self.statuses.borrow().get(pane).cloned();
            let status = core.as_ref().and_then(|core| core.pane(pane)).and_then(|p| p.status().map(String::from));
            let since = seen.map_or_else(|| self.last_event(entry), |(_, at)| at);
            return match status.as_deref() {
                Some("needs-you") => ("needs".into(), since),
                Some("working") => ("working".into(), since),
                _ if !entry.stale.is_empty() => ("stale".into(), since),
                _ if entry.ready.is_some() => ("ready".into(), since),
                _ => ("idle".into(), since),
            };
        }
        let last = self.last_event(entry);
        let status = if entry.folder.is_none() && entry.held.is_none() {
            if self.waits(entry).is_empty() { "waiting" } else { "blocked" }
        } else if !entry.stale.is_empty() {
            "stale"
        } else if entry.ready.is_some() {
            "ready"
        } else if entry.stopped.as_deref() == Some(Status::NeedsYou.as_str()) {
            "needs"
        } else {
            "stopped"
        };
        (status.to_string(), last)
    }

    fn after_of(&self, entry: &Entry) -> Vec<Value> {
        let waits = self.waits(entry);
        let stale = entry.stale.iter();
        let bases = entry.bases.iter().map(|b| &b.id);
        let ids: Vec<&String> = if waits.is_empty() { stale.chain(bases).collect() } else { waits.iter().collect() };
        let replica = self.replica.borrow();
        let name = |id: &str| self.known(id).and_then(|e| e.name).or_else(|| replica.births(id).first().and_then(|r| r.text("name").map(String::from)));
        ids.into_iter().map(|id| json!({ "id": id, "name": opt(name(id).as_deref()) })).collect()
    }

    fn item(&self, entry: &Entry, pane: Option<&str>, desk: Option<String>) -> Value {
        let (status, since) = self.status_of(entry, pane);
        json!({
            "id": entry.id,
            "name": entry.name.clone().unwrap_or_else(|| self.short(&entry.id)),
            "status": status,
            "since": num(since),
            "desk": opt(desk.as_deref()),
            "machine": self.me(),
            "agent": program(&entry.agent).rsplit('/').next().unwrap_or_default(),
            "branch": opt(entry.folder.as_deref().and_then(branch_of).as_deref()),
            "after": self.after_of(entry),
            "madeAt": num(entry.made_at),
        })
    }

    pub fn shelf_here(&self, project: &str) -> Vec<Value> {
        let here = |repo: &str| identity(repo).as_deref() == Some(project);
        let mut tasks: Vec<Value> = vec![];
        for r in self.running.borrow().iter().filter(|r| here(&r.entry.repo)) {
            let desk = r.entry.claim.clone().unwrap_or_else(|| self.me());
            tasks.push(self.item(&r.entry, Some(&r.pane), Some(desk)));
        }
        for e in self.entries.borrow().iter().filter(|e| here(&e.repo)) {
            tasks.push(self.item(e, e.held.as_deref(), None));
        }
        tasks
    }

    pub fn shelf_records(&self, project: &str, shown: &HashSet<String>) -> Vec<Value> {
        let machines = self.machines();
        let replica = self.replica.borrow();
        let mut records: Vec<&replica::Record> = replica.records().filter(|r| r.text("project") == Some(project) && r.text("state").is_some_and(|s| s != "done") && !shown.contains(&r.task)).collect();
        records.sort_by_key(|r| r.born());
        records
            .into_iter()
            .map(|r| {
                let status = if r.text("state") == Some("waiting") { "waiting" } else { "stopped" };
                json!({ "id": r.task, "name": r.text("name").unwrap_or(&r.task), "status": status, "since": num(r.born().ms as f64 / 1000.0), "desk": opt(r.text("claim")), "machine": machines.name_of(r.text("owner").unwrap_or_default()), "agent": program(r.text("agent").unwrap_or(DEFAULT_AGENT)).rsplit('/').next().unwrap_or_default(), "branch": Value::Null, "after": [], "madeAt": num(r.born().ms as f64 / 1000.0), "unreachable": true })
            })
            .collect()
    }

    pub fn done_lately(&self, project: &str) -> Vec<Value> {
        let done = self.db.done_tasks().unwrap_or_default();
        done.iter().filter(|(row, _)| identity(&row.repo).as_deref() == Some(project)).take(DONE_LATELY).map(|(row, _)| json!(Entry::from_row(row).name.unwrap_or_else(|| row.id.clone()))).collect()
    }

    pub fn owner_of(&self, task: &str) -> Option<Option<String>> {
        if self.resolve(task).ok().flatten().is_some_and(|id| self.is_open(&id)) {
            return Some(None);
        }
        let machines = self.machines();
        let replica = self.replica.borrow();
        let record = replica.births(task).into_iter().find(|r| r.text("state").is_some_and(|s| s != "done"))?;
        Some(Some(machines.name_of(record.text("owner")?)))
    }

    pub fn task_here(&self, task: &str) -> Result<Map, KmuxError> {
        let id = self.resolve(task)?.ok_or_else(|| not_found(task))?;
        let running = self.running.borrow().iter().find(|r| r.entry.id == id).map(|r| (r.entry.clone(), Some(r.pane.clone()), Some(r.entry.claim.clone().unwrap_or_else(|| self.me()))));
        let found = running.or_else(|| self.entries.borrow().iter().find(|e| e.id == id).map(|e| (e.clone(), e.held.clone(), None)));
        let (entry, pane, desk) = found.ok_or_else(|| not_found(task))?;
        let mut item = self.item(&entry, pane.as_deref(), desk);
        item["message"] = opt(entry.message.as_deref());
        let you = |sender: &str| sender == USER || sender == PHONE;
        let from = |sender: &str| match sender {
            sender if you(sender) => "you".to_string(),
            PLANNER | FROM_KANNA => sender.to_string(),
            sender if sender == id => entry.name.clone().unwrap_or_else(|| self.short(&id)),
            sender if db::is_task_id(sender) => self.known(sender).and_then(|e| e.name).unwrap_or_else(|| self.short(sender)),
            sender => sender.to_string(),
        };
        let started = entry.folder.is_some() || entry.held.is_some();
        let notes = entry.notes.iter().map(|n| json!({ "at": num(n.at), "from": from(&n.from), "you": you(&n.from), "text": n.text, "read": started.then(|| num(n.at)), "client": n.id }));
        let told = self.db.messages(Some(&id)).unwrap_or_default();
        let mut thread: Vec<Value> = notes.chain(told.iter().map(|m| json!({ "at": num(m.at), "from": from(&m.sender), "you": you(&m.sender), "text": m.text, "read": m.delivered.map(num) }))).collect();
        thread.sort_by(|a, b| a["at"].as_f64().partial_cmp(&b["at"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
        let last = self.db.last_words(&id).ok().flatten().map_or(Value::Null, |w| json!({ "at": num(w.at), "asking": w.asking, "text": w.text }));
        Ok(Map::from_iter([("task".to_string(), item), ("thread".to_string(), Value::Array(thread)), ("last".to_string(), last)]))
    }

    pub fn task_record(&self, task: &str) -> Option<Map> {
        let shown = HashSet::new();
        let replica = self.replica.borrow();
        let record = replica.births(task).into_iter().find(|r| r.text("state").is_some_and(|s| s != "done"))?;
        let project = record.text("project")?.to_string();
        let message = record.text("message").map(String::from);
        drop(replica);
        let mut item = self.shelf_records(&project, &shown).into_iter().find(|t| t["id"] == task)?;
        item["message"] = opt(message.as_deref());
        Some(Map::from_iter([("task".to_string(), item), ("thread".to_string(), json!([])), ("last".to_string(), Value::Null)]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_go_with_the_first_message_and_it_stays_as_written() {
        let entry = Entry { message: Some("Do the thing".into()), notes: vec![Note { id: None, from: "user".into(), at: 1.0, text: "Don't forget X".into() }, Note { id: None, from: "task-1".into(), at: 2.0, text: "And Y".into() }], ..Default::default() };
        assert_eq!(entry.brief().as_deref(), Some("Do the thing\n\n[message from user] Don't forget X\n\n[message from task-1] And Y"));
        assert_eq!(entry.message.as_deref(), Some("Do the thing"));
        assert_eq!(Note::list(Some(&Note::values(&entry.notes))), entry.notes);
    }

    #[test]
    fn a_claude_transcript_is_found_by_its_folder_and_session() {
        let dir = std::env::temp_dir().join(format!("kanna-words-{}", std::process::id()));
        let config = dir.join("config");
        let folder = dir.join("a.b/work");
        fs::create_dir_all(&folder).unwrap();
        let real = fs::canonicalize(&folder).unwrap();
        let slug: String = real.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let projects = config.join("projects").join(slug);
        fs::create_dir_all(&projects).unwrap();
        fs::write(projects.join("one.jsonl"), "{}").unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", &config);
        assert_eq!(claude_transcript(&folder, Some("one")), Some(projects.join("one.jsonl")));
        assert_eq!(claude_transcript(&folder, Some("gone")), Some(projects.join("one.jsonl")), "a resumed session may have a new ID");
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        fs::remove_dir_all(&dir).unwrap();
    }
}
