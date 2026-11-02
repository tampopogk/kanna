use super::*;
use crate::replica::Peer;
use crate::transfer::{self, Saved};

pub(super) const OUT: &str = "out";

const IN: &str = "in";

const TAB_FIELDS: &[&str] = &["type", "path", "cwd", "url", "cmd", "line", "args", "view"];

pub(super) struct Asking {
    id: String,
    peer: Peer,
    responder: Responder,
}

pub(super) struct Moving {
    entry: Entry,
    peer: Peer,
    session: String,
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

const HANDOFF_WAIT: Duration = Duration::from_secs(300);

const HANDOFF: &str = "This task is about to move to another machine, where a new agent carries it on from what you write now. Write it a handoff: the goal and where it stands; the decisions made and why; what's done and what's left; the next step; open questions; the key files. Reply with only the handoff, in markdown, and change nothing.";

fn agent_name(command: &str) -> Option<&'static str> {
    agent(command).map(|a| a.names[0]).filter(|name| transfer::moves(name))
}

fn tabs_of(listed: &Value, workspace: Option<&str>, agent_pane: Option<&str>, folder: &str) -> Vec<Value> {
    let Some(workspace) = workspace else { return vec![] };
    let inside = |p: &Value, key: &str| string(p, key).is_none_or(|path| path == folder || path.starts_with(&format!("{folder}/")));
    let panes = listed.get("panes").and_then(Value::as_array).cloned().unwrap_or_default();
    panes
        .iter()
        .filter(|p| string(p, "workspace") == Some(workspace) && string(p, "id") != agent_pane)
        .filter(|p| ["term", "md", "code", "diff", "web"].contains(&string(p, "type").unwrap_or_default()) && inside(p, "path") && inside(p, "cwd"))
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

fn valid_task(id: &str) -> bool {
    id.len() == 13 && id.starts_with("task-") && id[5..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_branch(repo: &Path, branch: &str) -> bool {
    !branch.starts_with('-') && git(repo, &["check-ref-format", "--branch", branch]).is_ok()
}

fn local_agent(command: &str) -> String {
    let mut words = command.split_whitespace();
    let Some(first) = words.next() else { return command.to_string() };
    if !first.starts_with('/') || Path::new(first).is_file() {
        return command.to_string();
    }
    std::iter::once(program(first)).chain(words).collect::<Vec<_>>().join(" ")
}

fn handoff_args(command: &str, session: &str) -> Option<Vec<String>> {
    let mut words: Vec<String> = command.split_whitespace().map(String::from).collect();
    match agent_name(command)? {
        "claude" => words.extend(["-p".into(), "--resume".into(), session.into(), HANDOFF.into()]),
        "codex" => words.extend(["exec".into(), "resume".into(), session.into(), HANDOFF.into()]),
        _ => return None,
    }
    Some(words)
}

fn write_handoff(command: &str, session: &str, folder: &Path) -> Result<String, String> {
    let args = handoff_args(command, session).ok_or("no handoff for this agent")?;
    let mut child = Command::new(&args[0]).args(&args[1..]).current_dir(folder).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().map_err(|e| format!("can't run {}: {e}", args[0]))?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let reading = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut text);
        text
    });
    let deadline = Instant::now() + HANDOFF_WAIT;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => break,
            Some(status) => return Err(format!("it exited with {status}")),
            None if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no handoff in {}s", HANDOFF_WAIT.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    let text = reading.join().unwrap_or_default().trim().to_string();
    if text.is_empty() {
        return Err("it wrote nothing".into());
    }
    Ok(text)
}

fn handed_over(doc: &Value) -> Option<String> {
    let handoff = string(doc, "handoff")?;
    let from = string(doc, "from").unwrap_or("another machine");
    let branch = string(doc, "branch").unwrap_or("its branch");
    let pre = string(doc, "pre").unwrap_or("?");
    let first = string(doc, "message").map_or(String::new(), |m| format!(" The task as it was first given: {m}"));
    Some(format!("This task moved here from {from}, and you carry it on from the handoff its previous agent wrote before the move. You're on {branch} at {pre}, with its uncommitted work staged.{first}\n\n{handoff}"))
}

impl Tasks {
    fn peer_named(&self, to: &str) -> Result<Peer, KmuxError> {
        let machines = self.machines();
        if to == machines.me.name || to == machines.me.key {
            return Err(bad(format!("{to} is this machine")));
        }
        machines.peers.into_iter().find(|p| p.name == to || p.key == to).ok_or_else(|| KmuxError::new("not_found", format!("no machine \"{to}\" among your machines (see kanna machine)")))
    }

    pub(super) fn moving_to(&self, task: &str) -> Option<String> {
        let saved = transfer::under_way(&self.db, OUT).into_iter().find(|t| t.task == task)?;
        Some(saved.text("to").unwrap_or("another machine").to_string())
    }

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
        if let Err(error) = self.movable(&entry, &core) {
            return responder.reply(Err(error));
        }
        let project = identity(&entry.repo);
        let key = peer.key.clone();
        self.send_to(&key, json!({ "action": "heads", "project": project }), Asked::Heads(Box::new(Asking { id: entry.id, peer, responder })));
    }

    fn movable(&self, entry: &Entry, core: &CoreHandle) -> Result<(), KmuxError> {
        if entry.birth.is_none() {
            return Err(KmuxError::new("not_shared", format!("{} isn't on the replicated Shelf: this machine has no endpoint", entry.id)));
        }
        if agent_name(&entry.agent).is_none() {
            return Err(KmuxError::new("agent", format!("{}'s agent is {}: only claude's and codex's conversations move between machines", entry.id, program(&entry.agent))));
        }
        if !entry.folder.as_deref().is_some_and(|f| Path::new(f).is_dir()) {
            return Err(KmuxError::new("not_started", format!("{} hasn't started: it runs on whichever machine takes it (choose with kanna task new --on)", entry.id)));
        }
        if let Some(to) = self.moving_to(&entry.id) {
            return Err(KmuxError::new("moving", format!("{} is moving to {to} already", entry.id)));
        }
        if let Some(pane) = entry.held.as_deref().filter(|pane| alive(core, pane)) {
            let working = self.ready.borrow().get(pane).and_then(|r| r.status()) == Some(Status::Working);
            if working || self.quitting.borrow().contains_key(pane) || self.pending.borrow().values().any(|p| p.entry.id == entry.id) || self.launching.borrow().contains(&entry.id) {
                return Err(KmuxError::new("busy", format!("{} is in the middle of a turn: move it when its turn is over", entry.id)));
            }
        }
        Ok(())
    }

    pub(super) fn heads_came(self: &Rc<Self>, asking: Asking, reply: Result<Value, String>) {
        let Asking { id, peer, responder } = asking;
        let theirs = match reply {
            Ok(reply) => strings(&reply, "heads"),
            Err(why) => {
                let code = if why.ends_with("[unreachable]") { "unreachable" } else { "refused" };
                return responder.reply(Err(KmuxError::new(code, format!("{} didn't take {id}: {why}", peer.name))));
            }
        };
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else { return responder.reply(Err(not_found(&id))) };
        if let Err(error) = self.movable(&entry, &core) {
            return responder.reply(Err(error));
        }
        let Some(pane) = entry.held.clone().filter(|pane| alive(&core, pane)) else { return self.stopped_to_move(id, peer, theirs, responder) };
        log!("kanna-daemon: {id} moves to {}, so its agent quits", peer.name);
        let this = Rc::downgrade(self);
        self.end_agent(&pane.clone(), &entry.agent, move |session| {
            let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
            this.keep_session(&pane, session);
            this.note(&pane, Change::new("stopped").by(Some("daemon")).with("why", json!(format!("moving to {}", peer.name))));
            let Some(core) = this.core.upgrade() else { return responder.reply(Err(gone())) };
            let again = Rc::downgrade(&this);
            core.hang_up(&pane.clone(), move |_| match again.upgrade() {
                Some(this) => this.stopped_to_move(id, peer, theirs, responder),
                None => responder.reply(Err(gone())),
            });
        });
    }

    fn stopped_to_move(self: &Rc<Self>, id: String, peer: Peer, theirs: Vec<String>, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else { return responder.reply(Err(not_found(&id))) };
        let agent = agent_name(&entry.agent).unwrap_or_default();
        let folder = PathBuf::from(entry.folder.clone().unwrap_or_default());
        let Some(session) = entry.session.clone().or_else(|| entry.clone().resumable().session) else {
            return responder.reply(Err(KmuxError::new("no_session", format!("{id}'s {agent} session isn't known, so its agent can't write a handoff"))));
        };
        let this = Rc::downgrade(self);
        core.handle(json!({ "cmd": "list" }), move |listed| {
            let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
            let tabs = tabs_of(&listed, entry.workspace.as_deref(), entry.held.as_deref(), &folder.to_string_lossy());
            this.pack_to_move(Moving { entry, peer, session, tabs, responder }, theirs);
        });
    }

    fn pack_to_move(self: &Rc<Self>, moving: Moving, theirs: Vec<String>) {
        let id = format!("{}-{}", moving.entry.id, replica::wall());
        let folder = PathBuf::from(moving.entry.folder.clone().unwrap_or_default());
        let outbox = transfer::outbox(&self.state);
        let (task, agent, session) = (moving.entry.id.clone(), moving.entry.agent.clone(), moving.session.clone());
        let token = self.next.get() + 1;
        self.next.set(token);
        self.asked.borrow_mut().insert(token, Asked::Packed(Box::new(moving), id.clone()));
        let events = self.events.clone();
        std::thread::spawn(move || {
            let base = crate::task_scripts::base(&folder);
            let packed = (|| {
                let handoff = write_handoff(&agent, &session, &folder).map_err(|why| format!("its agent wrote no handoff: {why}"))?;
                fs::create_dir_all(&outbox).map_err(|e| format!("can't make {}: {e}", outbox.display()))?;
                let packed = transfer::pack(&folder, &task)?;
                match transfer::bundle(&folder, &packed.branch, &theirs, &outbox.join(format!("{id}.bin"))) {
                    Ok(()) => Ok(json!({ "branch": packed.branch, "pre": packed.pre, "wip": packed.wip, "base": base, "handoff": handoff })),
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
        let Moving { mut entry, peer, session: _, tabs, responder } = moving;
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
            "folder": opt(entry.folder.as_deref()),
            "tabs": tabs,
            "to": peer.name,
            "branch": made["branch"],
            "base": made["base"],
            "handoff": made["handoff"],
            "pre": made["pre"],
            "wip": made["wip"],
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

    pub(super) fn take_transfer(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(peer) = string(args, kanna_net::PEER).map(String::from) else { return responder.reply(Err(bad("a transfer comes from another machine's daemon, over a transfer stream"))) };
        self.came_back(&peer);
        let (id, file) = (string(args, "id").unwrap_or_default().to_string(), string(args, "file").unwrap_or_default().to_string());
        if let Some(saved) = transfer::saved(&self.db, &id).filter(|s| s.role == IN) {
            match saved.state.as_str() {
                "started" => {
                    let _ = fs::remove_file(&file);
                    return responder.reply(Ok(Self::taken_out(&saved, None)));
                }
                "installed" => {
                    let _ = fs::remove_file(&file);
                    return self.carry_on(saved, responder);
                }
                _ => {}
            }
        }
        let task = string(args, "task").unwrap_or_default().to_string();
        let project = string(args, "project").unwrap_or_default();
        if !valid_task(&task) {
            return responder.reply(Err(bad(format!("\"{task}\" isn't a task ID"))));
        }
        let Some(repo) = self.clone_of(project) else { return responder.reply(Err(KmuxError::new("no_clone", format!("{} has no clone of {project}", self.machines().me.name)))) };
        if self.held_here(&task) {
            return responder.reply(Err(KmuxError::new("there", format!("{task} is on this machine already"))));
        }
        let agent = agent_name(string(args, "agent").unwrap_or_default());
        let (Some(_), Some(branch), Some(packed)) = (agent, string(args, "branch").map(String::from), Saved { doc: args.clone(), ..Default::default() }.packed()) else {
            return responder.reply(Err(bad("a transfer needs the task's agent, branch and commits")));
        };
        let Some(_) = handed_over(args) else {
            return responder.reply(Err(bad("a transfer needs its agent's handoff")));
        };
        if !valid_branch(Path::new(&repo), &branch) {
            return responder.reply(Err(bad(format!("\"{branch}\" isn't a branch name"))));
        }
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
        let base = string(args, "base").map(String::from);
        let token = self.next.get() + 1;
        self.next.set(token);
        self.asked.borrow_mut().insert(token, Asked::Unbundled(Taking { id: id.clone(), responder }));
        let events = self.events.clone();
        log!("kanna-daemon: {task} comes from {name} into {repo}");
        std::thread::spawn(move || {
            let taken = transfer::unbundle(Path::new(&repo), &id, Path::new(&file), &task, &branch, &packed, base.as_deref()).map(|(folder, branch)| json!({ "folder": folder, "branch": branch }));
            let _ = fs::remove_file(&file);
            let _ = events.send(Event::Replica { token, reply: taken });
        });
    }

    fn held_here(&self, task: &str) -> bool {
        self.entries.borrow().iter().any(|e| e.id == task) || self.running.borrow().iter().any(|r| r.entry.id == task)
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
                log!("kanna-daemon: {}'s worktree is {}", saved.task, string(&made, "folder").unwrap_or(""));
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
        for key in ["why"] {
            if let Some(text) = saved.text(key) {
                out.insert(key.to_string(), json!(text));
            }
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
        let Some(first) = handed_over(&doc) else { return responder.reply(Err(bad("a transfer needs its agent's handoff"))) };
        let entry = Entry {
            id: saved.task.clone(),
            name: text("name"),
            message: text("message"),
            repo: text("repo").unwrap_or_default(),
            by: text("by"),
            made_at: doc["madeAt"].as_f64().unwrap_or(0.0),
            agent: local_agent(&text("agent").unwrap_or_else(|| DEFAULT_AGENT.into())),
            folder: text("folder"),
            after: strings(&doc, "after"),
            on: strings(&doc, "on"),
            birth: Some(birth),
            ..Default::default()
        };
        if !self.held_here(&entry.id) {
            let change = Change::new("moved").moved(Place::Done, Place::Shelf).by(Some("daemon")).with("from", json!(text("from")));
            if let Err(error) = self.store(&entry, Place::Shelf, None, Some(change)) {
                return responder.reply(Err(error));
            }
            insert_in_order(&mut self.entries.borrow_mut(), entry.clone());
            self.changed();
        }
        let this = Rc::downgrade(self);
        let (tabs, sender_folder, to) = (doc.get("tabs").and_then(Value::as_array).cloned().unwrap_or_default(), text("senderFolder"), entry.folder.clone().unwrap_or_default());
        let then = Answer::Then(Box::new(move |result: Result<Map, KmuxError>| {
            let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
            let mut saved = saved;
            saved.state = "started".into();
            let pane = match result {
                Ok(out) => string(&Value::Object(out), "pane").map(String::from),
                Err(error) => {
                    log!("kanna-daemon: {} moved here but didn't start: {}", saved.task, error.message);
                    this.alter(&saved.task, Change::new("stopped").by(Some("daemon")).with("why", json!(error.message)), |e| e.stopped = Some(format!("moved here, but didn't start: {}", error.message)));
                    saved.doc["why"] = json!(format!("it's here, stopped: {}", error.message));
                    None
                }
            };
            if let Some(pane) = &pane {
                this.reopen_tabs(pane, &tabs, sender_folder.as_deref().unwrap_or_default(), &to);
            }
            let _ = this.db.atomically(|conn| transfer::save(conn, &saved));
            responder.reply(Ok(Self::taken_out(&saved, pane.as_deref())))
        }));
        if let Some(folder) = &entry.folder {
            let scratch = Path::new(folder).join(SCRATCH);
            let linked = [Path::new(folder).join(".kanna"), scratch.clone(), scratch.join("handoff.md")].iter().any(|p| fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()));
            if linked {
                log!("kanna-daemon: {}'s handoff isn't saved in its worktree: a link is in the way", entry.id);
            } else {
                let _ = fs::create_dir_all(&scratch).and_then(|_| fs::write(scratch.join("handoff.md"), format!("{}\n", text("handoff").unwrap_or_default())));
            }
        }
        log!("kanna-daemon: {} carries on here from its handoff", entry.id);
        self.start_handed_over(entry, first, then);
    }

    fn start_handed_over(self: &Rc<Self>, entry: Entry, first: String, then: Answer) {
        let folder = entry.folder.clone().unwrap_or_default();
        let message = entry.message.clone();
        let id = entry.id.clone();
        let this = Rc::downgrade(self);
        let starting = Starting {
            entry: Entry { folder: None, session: None, message: Some(first), ..entry },
            app: None,
            by: Some("daemon".into()),
            responder: Answer::Then(Box::new(move |result| {
                if let Some(this) = this.upgrade() {
                    this.alter(&id, Change::new("handed over").by(Some("daemon")), |e| e.message = message.clone());
                }
                then.reply(result)
            })),
        };
        self.launch(starting, folder, None);
    }

    fn reopen_tabs(&self, pane: &str, tabs: &[Value], from: &str, to: &str) {
        let Some(core) = self.core.upgrade() else { return };
        for tab in tabs {
            let mut args = moved_path(tab, from, to);
            let inside = |key: &str| string(&args, key).is_none_or(|path| (path == to || path.starts_with(&format!("{to}/"))) && !Path::new(path).components().any(|c| c == std::path::Component::ParentDir));
            let kind = string(&args, "type").unwrap_or("?").to_string();
            if !["term", "md", "code", "diff", "web"].contains(&kind.as_str()) || !inside("path") || !inside("cwd") || (kind == "web" && !string(&args, "url").is_some_and(|u| u.starts_with("http://") || u.starts_with("https://"))) {
                continue;
            }
            if let Some(fields) = args.as_object_mut() {
                fields.remove("cmd");
                if let Some(Value::Array(diff)) = fields.get("args") {
                    let safe = diff.iter().all(|a| a.as_str().is_some_and(|a| ["--merge-base", "--cached", "--staged"].contains(&a) || !a.starts_with('-')));
                    if !safe {
                        fields.remove("args");
                    }
                }
            }
            args["from"] = json!(pane);
            args["tab"] = json!(true);
            args["wait"] = json!(false);
            core.handle(json!({ "cmd": "open", "args": args }), move |reply| {
                if let Some(error) = failed(&reply) {
                    log!("kanna-daemon: couldn't reopen a {kind} tab of a task that moved here: {}", error.message);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_tasks_agent_runs_from_this_machines_path_and_its_handoff_says_where_it_stands() {
        assert_eq!(local_agent("/Users/alice/bin/claude --model opus"), "claude --model opus", "a path that isn't here becomes the program's name");
        assert_eq!(local_agent("/bin/sh -c x"), "/bin/sh -c x");
        assert_eq!(local_agent("codex"), "codex");
        assert!(valid_task("task-0000beef") && !valid_task("../../x") && !valid_task("task-0000beef/.."));
        assert_eq!(handoff_args("claude", "s1").unwrap()[..4], ["claude", "-p", "--resume", "s1"]);
        assert_eq!(handoff_args("codex --yolo", "s1").unwrap()[..5], ["codex", "--yolo", "exec", "resume", "s1"]);
        assert!(handoff_args("opencode", "s1").is_none());
        let doc = json!({ "handoff": "# Handoff\nnext: tests", "from": "mbp", "branch": "fix-notes", "pre": "abc123", "message": "Fix the notes" });
        let first = handed_over(&doc).unwrap();
        for part in ["moved here from mbp", "fix-notes at abc123", "Fix the notes", "next: tests"] {
            assert!(first.contains(part), "{part}: {first}");
        }
        assert!(handed_over(&json!({ "from": "mbp" })).is_none(), "no handoff, no move");
    }

    #[test]
    fn a_tasks_tabs_are_its_workspaces_panes_but_its_agent_with_paths_moved_to_the_receivers_worktree() {
        let listed = json!({ "panes": [
            { "id": "term-1", "type": "term", "workspace": "ws-1", "cmd": "claude --resume s", "cwd": "/Users/me/notes/.kanna-worktrees/task-1" },
            { "id": "md-2", "type": "md", "workspace": "ws-1", "path": "/Users/me/notes/.kanna-worktrees/task-1/docs/a.md", "agent": null },
            { "id": "term-3", "type": "term", "workspace": "ws-1", "cmd": null, "cwd": "/Users/me/notes/.kanna-worktrees/task-1/src" },
            { "id": "web-4", "type": "web", "workspace": "ws-1", "url": "http://localhost:3000/" },
            { "id": "md-5", "type": "md", "workspace": "ws-2", "path": "/elsewhere.md" },
        ] });
        let tabs = tabs_of(&listed, Some("ws-1"), Some("term-1"), "/Users/me/notes/.kanna-worktrees/task-1");
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
        assert!(tabs_of(&listed, None, None, "/x").is_empty());
    }
}
