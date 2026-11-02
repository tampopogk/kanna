use super::*;
use crate::replica::Peer;
use crate::transfer::{self, Homes, Packed, Saved};

pub(super) const OUT: &str = "out";

const IN: &str = "in";

const TAB_FIELDS: &[&str] = &["type", "path", "cwd", "url", "cmd", "line", "args", "view"];

pub(super) struct Moving {
    entry: Entry,
    peer: Peer,
    session: String,
    transcript: (PathBuf, String),
    tabs: Vec<Value>,
    responder: Responder,
}

pub(super) struct Sending {
    pub(super) id: String,
    responder: Option<Responder>,
}

pub(super) struct Taking {
    id: String,
    responder: Responder,
}

fn agent_name(command: &str) -> Option<&'static str> {
    agent(command).map(|a| a.names[0]).filter(|name| transfer::moves(name))
}

fn tabs_of(listed: &Value, workspace: Option<&str>, agent_pane: Option<&str>) -> Vec<Value> {
    let Some(workspace) = workspace else { return vec![] };
    let panes = listed.get("panes").and_then(Value::as_array).cloned().unwrap_or_default();
    panes
        .iter()
        .filter(|p| string(p, "workspace") == Some(workspace) && string(p, "id") != agent_pane)
        .map(|p| Value::Object(TAB_FIELDS.iter().filter_map(|key| p.get(*key).filter(|v| !v.is_null()).map(|v| (key.to_string(), v.clone()))).collect()))
        .collect()
}

fn moved_path(value: &Value, from: &str, to: &str) -> Value {
    match value {
        Value::String(text) if !from.is_empty() && (text == from || text.starts_with(&format!("{from}/"))) => json!(format!("{to}{}", &text[from.len()..])),
        Value::Array(items) => Value::Array(items.iter().map(|v| moved_path(v, from, to)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(k, v)| (k.clone(), moved_path(v, from, to))).collect()),
        other => other.clone(),
    }
}

impl Tasks {
    fn peer_named(&self, to: &str) -> Result<Peer, KmuxError> {
        let machines = self.machines();
        if to == machines.me.name || to == machines.me.key {
            return Err(bad(format!("{to} is this machine")));
        }
        machines.peers.into_iter().find(|p| p.name == to || p.key == to).ok_or_else(|| KmuxError::new("not_found", format!("no machine \"{to}\" among your machines (see kanna machine)")))
    }

    /// Move: stops a Shelf task's agent at its turn's end and sends the task to another machine, where it carries on.
    pub(super) fn move_task(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let Some(reference) = string(args, "task") else { return responder.reply(Err(bad("say which task"))) };
        let Some(to) = string(args, "to").filter(|t| !t.is_empty()) else { return responder.reply(Err(bad("say which machine: kanna task move TASK MACHINE"))) };
        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == reference).cloned() else {
            let on_desk = self.running.borrow().iter().any(|r| r.entry.id == reference);
            return responder.reply(Err(match on_desk {
                true => KmuxError::new("on_desk", format!("{reference} is on a Desk: move it to the Shelf first (kanna task shelve {reference})")),
                false => not_found(reference),
            }));
        };
        let peer = match self.peer_named(to) {
            Ok(peer) => peer,
            Err(error) => return responder.reply(Err(error)),
        };
        if entry.birth.is_none() {
            return responder.reply(Err(KmuxError::new("not_shared", format!("{} isn't on the replicated Shelf: this machine has no endpoint", entry.id))));
        }
        if agent_name(&entry.agent).is_none() {
            return responder.reply(Err(KmuxError::new("agent", format!("{}'s agent is {}: only claude's and codex's conversations move between machines", entry.id, program(&entry.agent)))));
        }
        if !entry.folder.as_deref().is_some_and(|f| Path::new(f).is_dir()) {
            return responder.reply(Err(KmuxError::new("not_started", format!("{} hasn't started: it runs on whichever machine takes it (choose with kanna task new --on)", entry.id))));
        }
        if transfer::under_way(&self.db, OUT).iter().any(|t| t.task == entry.id) {
            return responder.reply(Err(KmuxError::new("moving", format!("{} is moving already", entry.id))));
        }
        let id = entry.id.clone();
        let Some(pane) = entry.held.clone().filter(|pane| alive(&core, pane)) else { return self.stopped_to_move(id, peer, responder) };
        let working = self.ready.borrow().get(&pane).and_then(|r| r.status()) == Some(Status::Working);
        if working || self.quitting.borrow().contains_key(&pane) {
            return responder.reply(Err(KmuxError::new("busy", format!("{id} is in the middle of a turn: move it when its turn is over"))));
        }
        log!("kanna-daemon: {id} moves to {}, so its agent quits", peer.name);
        let this = Rc::downgrade(self);
        self.end_agent(&pane.clone(), &entry.agent, move |session| {
            let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
            this.keep_session(&pane, session);
            this.note(&pane, Change::new("stopped").by(Some("daemon")).with("why", json!(format!("moving to {}", peer.name))));
            let Some(core) = this.core.upgrade() else { return responder.reply(Err(gone())) };
            let again = Rc::downgrade(&this);
            core.hang_up(&pane.clone(), move |_| match again.upgrade() {
                Some(this) => this.stopped_to_move(id, peer, responder),
                None => responder.reply(Err(gone())),
            });
        });
    }

    fn stopped_to_move(self: &Rc<Self>, id: String, peer: Peer, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else { return responder.reply(Err(not_found(&id))) };
        let agent = agent_name(&entry.agent).unwrap_or_default();
        let folder = PathBuf::from(entry.folder.clone().unwrap_or_default());
        let Some(session) = entry.session.clone().or_else(|| entry.clone().resumable().session) else {
            return responder.reply(Err(KmuxError::new("no_session", format!("{id}'s {agent} session isn't known, so its conversation can't move"))));
        };
        let transcript = match transfer::transcript(&Homes::here(), agent, &folder, &session) {
            Ok(found) => found,
            Err(why) => return responder.reply(Err(KmuxError::new("no_transcript", format!("{id} can't move: {why}")))),
        };
        let project = identity(&entry.repo);
        let this = Rc::downgrade(self);
        core.handle(json!({ "cmd": "list" }), move |listed| {
            let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
            let tabs = tabs_of(&listed, entry.workspace.as_deref(), entry.held.as_deref());
            let key = peer.key.clone();
            let moving = Moving { entry, peer, session, transcript, tabs, responder };
            this.send_to(&key, json!({ "action": "heads", "project": project }), Asked::Heads(Box::new(moving)));
        });
    }

    pub(super) fn heads_came(self: &Rc<Self>, moving: Moving, reply: Result<Value, String>) {
        let theirs = match reply {
            Ok(reply) => strings(&reply, "heads"),
            Err(why) => {
                let code = if why.ends_with("[unreachable]") { "unreachable" } else { "refused" };
                return moving.responder.reply(Err(KmuxError::new(code, format!("{} didn't take {}: {why}", moving.peer.name, moving.entry.id))));
            }
        };
        let id = format!("{}-{}", moving.entry.id, replica::wall());
        let folder = PathBuf::from(moving.entry.folder.clone().unwrap_or_default());
        let outbox = transfer::outbox(&self.state);
        let (task, transcript) = (moving.entry.id.clone(), moving.transcript.0.clone());
        let token = self.next.get() + 1;
        self.next.set(token);
        self.asked.borrow_mut().insert(token, Asked::Packed(Box::new(moving), id.clone()));
        let events = self.events.clone();
        std::thread::spawn(move || {
            let packed = (|| {
                fs::create_dir_all(&outbox).map_err(|e| format!("can't make {}: {e}", outbox.display()))?;
                let packed = transfer::pack(&folder, &task)?;
                let bundle = outbox.join(format!("{id}.bundle"));
                let joined = transfer::bundle(&folder, &packed.branch, &theirs, &bundle).and_then(|_| transfer::join(&outbox.join(format!("{id}.bin")), &bundle, &transcript));
                let _ = fs::remove_file(&bundle);
                match joined {
                    Ok((bundle, transcript)) => Ok(json!({ "branch": packed.branch, "pre": packed.pre, "wip": packed.wip, "bundle": bundle, "transcript": transcript })),
                    Err(why) => {
                        let _ = transfer::unpack(&folder, &packed);
                        Err(why)
                    }
                }
            })();
            let _ = events.send(Event::Replica { token, reply: packed });
        });
    }

    pub(super) fn packed(self: &Rc<Self>, moving: Moving, id: String, reply: Result<Value, String>) {
        let Moving { mut entry, peer, session, transcript, tabs, responder } = moving;
        let made = match reply {
            Ok(made) => made,
            Err(why) => return responder.reply(Err(KmuxError::new("failed", format!("couldn't pack {} to move: {why}", entry.id)))),
        };
        let birth = entry.birth.clone().unwrap_or_default();
        let doc = json!({
            "task": entry.id,
            "birth": birth,
            "name": opt(entry.name.as_deref()),
            "message": opt(entry.message.as_deref()),
            "by": opt(entry.by.as_deref()),
            "madeAt": num(entry.made_at),
            "agent": entry.agent,
            "after": entry.after,
            "on": entry.on,
            "project": opt(identity(&entry.repo).as_deref()),
            "session": session,
            "transcriptName": transcript.1,
            "folder": opt(entry.folder.as_deref()),
            "tabs": tabs,
            "to": peer.name,
            "branch": made["branch"],
            "pre": made["pre"],
            "wip": made["wip"],
            "bundle": made["bundle"],
            "transcript": made["transcript"],
        });
        let saved = Saved { id: id.clone(), task: entry.id.clone(), role: OUT.into(), peer: peer.key.clone(), state: "sending".into(), doc };
        entry.stopped = Some(format!("moving to {}", peer.name));
        let row = entry.row(Place::Shelf, entry.held.as_deref());
        let change = Change::new("moving").by(Some("daemon")).with("to", json!(peer.name));
        let handed = self.replica.borrow_mut().hand(&self.db, &entry.id, &birth, &peer.key, |conn| {
            transfer::save(conn, &saved)?;
            db::put_row(conn, &row, Some(&change))
        });
        if let Err(why) = handed {
            if let Some(packed) = saved.packed() {
                let _ = transfer::unpack(Path::new(entry.folder.as_deref().unwrap_or_default()), &packed);
            }
            return responder.reply(Err(KmuxError::new("not_owner", format!("{} can't move from this machine: {why}", entry.id))));
        }
        log!("kanna-daemon: {} is packed to move to {} ({id})", entry.id, peer.name);
        if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
            found.stopped = entry.stopped.clone();
        }
        self.changed();
        self.sync_at.set(Instant::now());
        self.send_transfer(saved, Some(responder));
    }

    fn send_transfer(&self, saved: Saved, responder: Option<Responder>) {
        let path = transfer::outbox(&self.state).join(format!("{}.bin", saved.id));
        let epoch = Replica::authority(&self.db, &saved.task, saved.text("birth").unwrap_or_default()).ok().flatten().map_or(0, |a| a.epoch);
        let mut args = saved.doc.clone();
        args["epoch"] = json!(epoch);
        let token = self.next.get() + 1;
        self.next.set(token);
        self.asked.borrow_mut().insert(token, Asked::Transfer(Sending { id: saved.id.clone(), responder }));
        match self.payload.borrow().as_ref() {
            Some(send) => send(saved.peer.clone(), saved.id.clone(), args, path, token),
            None => {
                if let Some(Asked::Transfer(sending)) = self.asked.borrow_mut().remove(&token) {
                    if let Some(responder) = sending.responder {
                        responder.reply(Err(KmuxError::new("unavailable", "this daemon has no endpoint")));
                    }
                }
            }
        }
    }

    /// Sends again the transfers the receiver hasn't said it took, after either side restarted or the receiver was away.
    pub(super) fn resend_transfers(&self) {
        let sending: HashSet<String> = self.asked.borrow().values().filter_map(|a| if let Asked::Transfer(s) = a { Some(s.id.clone()) } else { None }).collect();
        let machines = self.machines();
        let now = Instant::now();
        for saved in transfer::under_way(&self.db, OUT) {
            if !sending.contains(&saved.id) && machines.peers.iter().any(|p| p.key == saved.peer) && !self.is_away(&saved.peer, now) {
                self.send_transfer(saved, None);
            }
        }
    }

    pub(super) fn transferred(self: &Rc<Self>, sending: Sending, reply: Result<Value, String>) {
        let Some(saved) = transfer::saved(&self.db, &sending.id).filter(|s| s.state == "sending") else { return };
        let name = self.machines().name_of(&saved.peer);
        match reply {
            Ok(reply) => {
                let out = self.moved_away(&saved, &name, &reply);
                if let Some(responder) = sending.responder {
                    responder.reply(Ok(out));
                }
            }
            Err(why) if why.ends_with("[unreachable]") => {
                log!("kanna-daemon: {} waits to move to {name}: {why}", saved.task);
                self.went_away(saved.peer.clone());
                if let Some(responder) = sending.responder {
                    let out = Map::from_iter([("task".to_string(), json!(saved.task)), ("to".to_string(), json!(name)), ("waiting".to_string(), json!(true)), ("why".to_string(), json!(why))]);
                    responder.reply(Ok(out));
                }
            }
            Err(why) => {
                self.call_off(&saved, &name, &why);
                if let Some(responder) = sending.responder {
                    responder.reply(Err(KmuxError::new("refused", format!("{name} didn't take {}: {why}", saved.task))));
                }
            }
        }
    }

    fn moved_away(&self, saved: &Saved, name: &str, reply: &Value) -> Map {
        let birth = saved.text("birth").unwrap_or_default().to_string();
        let epoch = Replica::authority(&self.db, &saved.task, &birth).ok().flatten().map_or(0, |a| a.epoch);
        let entry = self.entries.borrow().iter().find(|e| e.id == saved.task).cloned();
        let done = Saved { state: "done".into(), ..saved.clone() };
        let change = Change::new("moved").moved(Place::Shelf, Place::Done).by(Some("daemon")).with("to", json!(name)).with("folder", reply.get("folder").cloned().unwrap_or(Value::Null));
        let stored = match &entry {
            Some(entry) => self.db.put_with(&Entry { stopped: None, ..entry.clone() }.row(Place::Done, None), Some(&change), |conn| transfer::save(conn, &done)),
            None => self.db.atomically(|conn| transfer::save(conn, &done)),
        };
        if let Err(error) = stored.and_then(|_| Replica::acked(&self.db, &saved.task, &birth, epoch)) {
            log!("kanna-daemon: couldn't save that {} moved to {name}: {error}", saved.task);
        }
        log!("kanna-daemon: {} carries on on {name}, in {}", saved.task, string(reply, "folder").unwrap_or("its worktree there"));
        if let Some(entry) = entry {
            self.take(&entry.id);
            if let Some(pane) = &entry.held {
                self.turns.borrow_mut().remove(pane);
                self.quitting.borrow_mut().remove(pane);
                if let Some(core) = self.core.upgrade().filter(|core| core.pane(pane).is_some()) {
                    core.handle(json!({ "cmd": "close", "args": { "task": pane } }), |_| {});
                }
            }
            let (repo, folder, branch, payload) = (PathBuf::from(&entry.repo), entry.folder.clone().map(PathBuf::from), saved.text("branch").map(String::from), transfer::outbox(&self.state).join(format!("{}.bin", saved.id)));
            std::thread::spawn(move || {
                if let (Some(folder), Some(branch)) = (folder, branch) {
                    if let Err(why) = transfer::remove(&repo, &folder, &branch) {
                        log!("kanna-daemon: kept the worktree {} of a task that moved: {why}", folder.display());
                    }
                }
                let _ = fs::remove_file(payload);
            });
            self.changed();
            self.refill();
        }
        let mut out = Map::from_iter([("task".to_string(), json!(saved.task)), ("to".to_string(), json!(name))]);
        for key in ["folder", "branch", "pane"] {
            if let Some(value) = reply.get(key).filter(|v| !v.is_null()) {
                out.insert(key.to_string(), value.clone());
            }
        }
        out
    }

    fn call_off(&self, saved: &Saved, name: &str, why: &str) {
        log!("kanna-daemon: {} stays here: {name} didn't take it: {why}", saved.task);
        let birth = saved.text("birth").unwrap_or_default().to_string();
        let epoch = Replica::authority(&self.db, &saved.task, &birth).ok().flatten().map_or(0, |a| a.epoch);
        let entry = self.entries.borrow().iter().find(|e| e.id == saved.task).cloned();
        if let (Some(folder), Some(packed)) = (entry.as_ref().and_then(|e| e.folder.clone()), saved.packed()) {
            if let Err(error) = transfer::unpack(Path::new(&folder), &packed) {
                log!("kanna-daemon: couldn't take the work-in-progress commit off {}: {error}", saved.task);
            }
        }
        let failed = Saved { state: "failed".into(), doc: { let mut d = saved.doc.clone(); d["why"] = json!(why); d }, ..saved.clone() };
        if let Err(error) = self.db.atomically(|conn| transfer::save(conn, &failed)).and_then(|_| self.replica.borrow_mut().reclaim(&self.db, &saved.task, &birth, epoch)) {
            log!("kanna-daemon: couldn't save that {} stays here: {error}", saved.task);
        }
        if let Some(mut entry) = entry {
            entry.stopped = Some(format!("didn't move to {name}: {why}"));
            let _ = self.store(&entry, Place::Shelf, entry.held.clone().as_deref(), Some(Change::new("stayed").by(Some("daemon")).with("to", json!(name)).with("why", json!(why))));
            if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
                *found = entry;
            }
            self.changed();
        }
        let _ = fs::remove_file(transfer::outbox(&self.state).join(format!("{}.bin", saved.id)));
    }

    pub(super) fn heads_here(&self, args: &Value) -> Result<Map, KmuxError> {
        let project = string(args, "project").unwrap_or_default();
        let repo = self.clone_of(project).ok_or_else(|| KmuxError::new("no_clone", format!("{} has no clone of {project}", self.machines().me.name)))?;
        Ok(Map::from_iter([("heads".to_string(), json!(transfer::heads(Path::new(&repo))))]))
    }

    /// Takes a task another machine sent: its branch into this machine's clone, its transcript where its agent looks, and it carries on here.
    pub(super) fn take_transfer(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(peer) = string(args, kanna_net::PEER).map(String::from) else { return responder.reply(Err(bad("a transfer comes from another machine's daemon, over a transfer stream"))) };
        self.came_back(&peer);
        let (id, file) = (string(args, "id").unwrap_or_default().to_string(), string(args, "file").unwrap_or_default().to_string());
        if let Some(saved) = transfer::saved(&self.db, &id).filter(|s| s.role == IN) {
            let _ = fs::remove_file(&file);
            return match saved.state.as_str() {
                "started" => responder.reply(Ok(Self::taken_out(&saved, None))),
                "installed" => self.carry_on(saved, responder),
                _ => responder.reply(Err(KmuxError::new("failed", format!("this machine couldn't take {}", saved.task)))),
            };
        }
        let task = string(args, "task").unwrap_or_default().to_string();
        let project = string(args, "project").unwrap_or_default();
        let Some(repo) = self.clone_of(project) else { return responder.reply(Err(KmuxError::new("no_clone", format!("{} has no clone of {project}", self.machines().me.name)))) };
        if self.held_here(&task) {
            return responder.reply(Err(KmuxError::new("there", format!("{task} is on this machine already"))));
        }
        let agent = agent_name(string(args, "agent").unwrap_or_default());
        let (Some(agent), Some(branch), Some(packed)) = (agent, string(args, "branch").map(String::from), Saved { doc: args.clone(), ..Default::default() }.packed()) else {
            return responder.reply(Err(bad("a transfer needs the task's agent, branch and commits")));
        };
        let name = self.machines().name_of(&peer);
        let mut doc = args.clone();
        if let Some(fields) = doc.as_object_mut() {
            for key in [kanna_net::PEER, "file", "action", "id"] {
                fields.remove(key);
            }
        }
        doc["repo"] = json!(repo);
        doc["from"] = json!(name);
        doc["senderFolder"] = args["folder"].clone();
        let saved = Saved { id: id.clone(), task: task.clone(), role: IN.into(), peer: peer.clone(), state: "receiving".into(), doc };
        if let Err(error) = self.db.atomically(|conn| transfer::save(conn, &saved)) {
            return responder.reply(Err(KmuxError::new("failed", format!("couldn't save the transfer of {task}: {error}"))));
        }
        let worktree = if Path::new(&repo).join(WORKTREES).join(&task).exists() && !self.ours(&repo, &task, &packed) { format!("{task}-{}", slug(&name)) } else { task.clone() };
        let branch = if git(Path::new(&repo), &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok() && !self.ours(&repo, &worktree, &packed) { format!("{branch}-{}", slug(&name)) } else { branch };
        let (sizes, session) = ((args["bundle"].as_u64().unwrap_or(0), args["transcript"].as_u64().unwrap_or(0)), string(args, "transcriptName").unwrap_or_default().to_string());
        let token = self.next.get() + 1;
        self.next.set(token);
        self.asked.borrow_mut().insert(token, Asked::Unbundled(Taking { id: id.clone(), responder }));
        let events = self.events.clone();
        log!("kanna-daemon: {task} comes from {name} into {repo}");
        std::thread::spawn(move || {
            let taken = (|| {
                let bundle = PathBuf::from(format!("{file}.bundle"));
                let bytes = transfer::split(Path::new(&file), sizes.0, sizes.1, &bundle)?;
                let (folder, branch) = transfer::unbundle(Path::new(&repo), &id, &bundle, &worktree, &branch, &packed)?;
                let placed = transfer::install(&Homes::here(), agent, Path::new(&folder), &session, &bytes)?;
                let _ = fs::remove_file(&bundle);
                let _ = fs::remove_file(&file);
                Ok(json!({ "folder": folder, "branch": branch, "placed": placed.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>() }))
            })();
            let _ = events.send(Event::Replica { token, reply: taken });
        });
    }

    fn held_here(&self, task: &str) -> bool {
        self.entries.borrow().iter().any(|e| e.id == task) || self.running.borrow().iter().any(|r| r.entry.id == task)
    }

    fn ours(&self, repo: &str, worktree: &str, packed: &Packed) -> bool {
        let folder = Path::new(repo).join(WORKTREES).join(worktree);
        folder.is_dir() && git(&folder, &["rev-parse", "--verify", "HEAD"]).ok().as_deref() == Some(packed.pre.as_str())
    }

    pub(super) fn unbundled(self: &Rc<Self>, taking: Taking, reply: Result<Value, String>) {
        let Some(mut saved) = transfer::saved(&self.db, &taking.id) else { return taking.responder.reply(Err(gone())) };
        match reply {
            Ok(made) => {
                saved.doc["folder"] = made["folder"].clone();
                saved.doc["branch"] = made["branch"].clone();
                saved.state = "installed".into();
                if let Err(error) = self.db.atomically(|conn| transfer::save(conn, &saved)) {
                    return taking.responder.reply(Err(KmuxError::new("failed", format!("couldn't save the transfer of {}: {error}", saved.task))));
                }
                log!("kanna-daemon: {}'s worktree is {} and its transcript is in {}", saved.task, string(&made, "folder").unwrap_or(""), made["placed"]);
                if std::env::var("KANNA_TRANSFER_EXIT").as_deref() == Ok("installed") {
                    log!("kanna-daemon: exits before it answers, as KANNA_TRANSFER_EXIT says");
                    std::process::exit(75);
                }
                self.carry_on(saved, taking.responder);
            }
            Err(why) => {
                log!("kanna-daemon: couldn't take {} here: {why}", saved.task);
                saved.state = "failed".into();
                saved.doc["why"] = json!(why);
                let _ = self.db.atomically(|conn| transfer::save(conn, &saved));
                taking.responder.reply(Err(KmuxError::new("failed", why)));
            }
        }
    }

    fn taken_out(saved: &Saved, pane: Option<&str>) -> Map {
        let mut out = Map::from_iter([("folder".to_string(), json!(saved.text("folder"))), ("branch".to_string(), json!(saved.text("branch")))]);
        if let Some(pane) = pane {
            out.insert("pane".to_string(), json!(pane));
        }
        out
    }

    fn carry_on(self: &Rc<Self>, saved: Saved, responder: Responder) {
        let doc = saved.doc.clone();
        let text = |key: &str| string(&doc, key).filter(|s| !s.is_empty()).map(String::from);
        let birth = text("birth").unwrap_or_default();
        if let Err(error) = self.replica.borrow_mut().receive(&self.db, &saved.task, &birth, doc["epoch"].as_i64().unwrap_or(0)) {
            return responder.reply(Err(KmuxError::new("failed", format!("couldn't take {} over: {error}", saved.task))));
        }
        if let Some(entry) = self.entries.borrow().iter().find(|e| e.id == saved.task).cloned() {
            if let Some(pane) = entry.held.filter(|pane| self.core.upgrade().is_some_and(|core| alive(&core, pane))) {
                return responder.reply(Ok(Self::taken_out(&saved, Some(&pane))));
            }
        }
        let entry = Entry {
            id: saved.task.clone(),
            name: text("name"),
            message: text("message"),
            repo: text("repo").unwrap_or_default(),
            by: text("by"),
            made_at: doc["madeAt"].as_f64().unwrap_or(0.0),
            agent: text("agent").unwrap_or_else(|| DEFAULT_AGENT.into()),
            folder: text("folder"),
            session: text("session"),
            after: strings(&doc, "after"),
            on: strings(&doc, "on"),
            birth: Some(birth),
            ..Default::default()
        };
        if !self.held_here(&entry.id) {
            let change = Change::new("moved").moved(Place::Done, Place::Shelf).by(Some("daemon")).with("from", json!(text("from"))).with("session", opt(entry.session.as_deref()));
            if let Err(error) = self.store(&entry, Place::Shelf, None, Some(change)) {
                return responder.reply(Err(error));
            }
            insert_in_order(&mut self.entries.borrow_mut(), entry.clone());
            self.changed();
        }
        let this = Rc::downgrade(self);
        let (tabs, sender_folder, to) = (doc.get("tabs").and_then(Value::as_array).cloned().unwrap_or_default(), text("senderFolder"), entry.folder.clone().unwrap_or_default());
        log!("kanna-daemon: {} carries on here, resuming its {} session {}", entry.id, program(&entry.agent), entry.session.as_deref().unwrap_or("?"));
        self.prepare(entry, None, Some("daemon".into()), Answer::Then(Box::new(move |result| {
            let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
            let pane = match result {
                Ok(out) => string(&Value::Object(out), "pane").map(String::from),
                Err(error) => return responder.reply(Err(error)),
            };
            if let Some(pane) = &pane {
                this.reopen_tabs(pane, &tabs, sender_folder.as_deref().unwrap_or_default(), &to);
            }
            let mut saved = saved;
            saved.state = "started".into();
            let _ = this.db.atomically(|conn| transfer::save(conn, &saved));
            responder.reply(Ok(Self::taken_out(&saved, pane.as_deref())))
        })));
    }

    fn reopen_tabs(&self, pane: &str, tabs: &[Value], from: &str, to: &str) {
        let Some(core) = self.core.upgrade() else { return };
        for tab in tabs {
            let mut args = moved_path(tab, from, to);
            args["from"] = json!(pane);
            args["tab"] = json!(true);
            args["wait"] = json!(false);
            let kind = string(&args, "type").unwrap_or("?").to_string();
            core.handle(json!({ "cmd": "open", "args": args }), move |reply| {
                if let Some(error) = failed(&reply) {
                    log!("kanna-daemon: couldn't reopen a {kind} tab of a task that moved here: {}", error.message);
                }
            });
        }
    }
}

fn slug(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tasks_tabs_are_its_workspaces_panes_but_its_agent_with_paths_moved_to_the_receivers_worktree() {
        let listed = json!({ "panes": [
            { "id": "term-1", "type": "term", "workspace": "ws-1", "cmd": "claude --resume s", "cwd": "/Users/me/notes/.kanna-worktrees/task-1" },
            { "id": "md-2", "type": "md", "workspace": "ws-1", "path": "/Users/me/notes/.kanna-worktrees/task-1/docs/a.md", "agent": null },
            { "id": "term-3", "type": "term", "workspace": "ws-1", "cmd": null, "cwd": "/Users/me/notes/.kanna-worktrees/task-1/src" },
            { "id": "web-4", "type": "web", "workspace": "ws-1", "url": "http://localhost:3000/" },
            { "id": "md-5", "type": "md", "workspace": "ws-2", "path": "/elsewhere.md" },
        ] });
        let tabs = tabs_of(&listed, Some("ws-1"), Some("term-1"));
        assert_eq!(tabs, vec![
            json!({ "type": "md", "path": "/Users/me/notes/.kanna-worktrees/task-1/docs/a.md" }),
            json!({ "type": "term", "cwd": "/Users/me/notes/.kanna-worktrees/task-1/src" }),
            json!({ "type": "web", "url": "http://localhost:3000/" }),
        ]);
        let moved: Vec<Value> = tabs.iter().map(|t| moved_path(t, "/Users/me/notes/.kanna-worktrees/task-1", "/home/me/repos/notes/.kanna-worktrees/task-1")).collect();
        assert_eq!(moved[2], tabs[2]);
        assert_eq!(moved[0]["path"], "/home/me/repos/notes/.kanna-worktrees/task-1/docs/a.md");
        assert_eq!(moved[1]["cwd"], "/home/me/repos/notes/.kanna-worktrees/task-1/src");
        assert_eq!(moved_path(&json!("/Users/me/notes/.kanna-worktrees/task-10/x"), "/Users/me/notes/.kanna-worktrees/task-1", "/b"), json!("/Users/me/notes/.kanna-worktrees/task-10/x"), "only paths inside the worktree move");
        assert!(tabs_of(&listed, None, None).is_empty());
    }
}
