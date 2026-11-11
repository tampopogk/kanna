use super::*;
use crate::replica::{Peer, MACHINE, WORKER};
use crate::transfer::{self, Saved};

const OUT: &str = "work-out";

const IN: &str = "work-in";

const BACK: &str = "work-back";

const SETTLE: f64 = 10.0;

const OUTPUT: usize = 64 << 10;

const POLL: Duration = Duration::from_secs(1);

pub struct Offer {
    pub repo: String,
    pub worktree: String,
    pub on: String,
    pub spec: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Remote {
    pub state: String,
    pub machine: Option<String>,
    pub terminal: Option<String>,
    pub result: Option<Value>,
    pub landed: bool,
    pub waits: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub name: String,
    pub roles: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Placed {
    Here,
    There,
    Nowhere(String),
}

pub(super) struct Taking {
    id: String,
    responder: Responder,
}

#[derive(Default)]
pub(super) struct Hosted {
    live: Cell<usize>,
    opening: RefCell<HashSet<String>>,
    ending: RefCell<HashSet<String>>,
    rested: RefCell<HashSet<String>>,
    pub(super) on_worker: RefCell<Option<Box<dyn Fn()>>>,
}

pub fn admits(on: &str, machine: &Machine) -> bool {
    on == "any" || on == machine.name || machine.roles.iter().any(|role| role == on)
}

pub fn place(on: Option<&str>, me: &Machine, peers: &[Machine], free: bool) -> Placed {
    let Some(on) = on.filter(|on| *on != "here" && *on != me.name) else { return Placed::Here };
    if peers.iter().any(|p| p.name == on) {
        return Placed::There;
    }
    match (admits(on, me), peers.iter().any(|p| admits(on, p))) {
        (true, false) => Placed::Here,
        (true, true) if free => Placed::Here,
        (_, true) => Placed::There,
        (false, false) => {
            let mut names: Vec<&str> = std::iter::once(me).chain(peers).map(|m| m.name.as_str()).collect();
            names.sort();
            let mut roles: Vec<&str> = std::iter::once(me).chain(peers).flat_map(|m| m.roles.iter().map(String::as_str)).collect();
            roles.sort();
            roles.dedup();
            Placed::Nowhere(format!("no such machine: none of your machines is named {on} or declares the role {on} (machines: {}; roles: {})", names.join(", "), if roles.is_empty() { "none".to_string() } else { roles.join(", ") }))
        }
    }
}

pub fn declared_roles(mine: &Path) -> Vec<String> {
    let read = |value: &Value| value.as_array().map(|list| list.iter().filter_map(Value::as_str).map(str::trim).filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')).map(String::from).collect::<Vec<_>>());
    let mut roles = configured(std::slice::from_ref(&mine.to_path_buf()), "machine", "roles", read, "a list of role names").unwrap_or_default();
    roles.push(if cfg!(target_os = "macos") { "mac" } else { "linux" }.to_string());
    roles.sort();
    roles.dedup();
    roles
}

fn valid_worker(id: &str) -> bool {
    id.len() == WORKER.len() + 8 && id.starts_with(WORKER) && id[WORKER.len()..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn new_id() -> Result<String, String> {
    let mut bytes = [0u8; 4];
    std::io::Read::read_exact(&mut fs::File::open("/dev/urandom").map_err(|e| e.to_string())?, &mut bytes).map_err(|e| e.to_string())?;
    Ok(format!("{WORKER}{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}

fn tail(text: &str, most: usize) -> String {
    let mut start = text.len().saturating_sub(most);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

fn wall_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn session_id() -> Option<String> {
    let mut b = [0u8; 16];
    std::io::Read::read_exact(&mut fs::File::open("/dev/urandom").ok()?, &mut b).ok()?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

impl Tasks {
    fn machine_of(&self, peer: &Peer) -> Machine {
        let roles = self.replica.borrow().record(MACHINE, &peer.key).map(|r| r.list("roles")).unwrap_or_default();
        Machine { name: peer.name.clone(), roles }
    }

    fn this_machine(&self) -> Machine {
        Machine { name: self.machines().me.name, roles: declared_roles(&my_config()) }
    }

    fn peers(&self) -> Vec<Machine> {
        self.machines().peers.iter().map(|p| self.machine_of(p)).collect()
    }

    pub(super) fn publish_roles(&self) {
        let machines = self.machines();
        if machines.me.key.is_empty() {
            return;
        }
        let fields = [("name", json!(machines.me.name)), ("roles", json!(declared_roles(&my_config())))];
        match self.replica.borrow_mut().write(&self.db, MACHINE, &machines.me.key, &fields) {
            Ok(0) => {}
            Ok(_) => self.sync_at.set(Instant::now()),
            Err(error) => log!("kanna-daemon: couldn't save this machine's roles: {error}"),
        }
    }

    fn busy_runners(&self, core: &CoreHandle) -> usize {
        let starting = self.pending.borrow().len() + self.launching.borrow().len();
        self.running_on_shelf(core).len() + starting
    }

    pub(super) fn opening_worker(&self, id: &str) -> bool {
        self.hosted.opening.borrow().contains(id)
    }

    pub(super) fn hosted_workers(&self) -> usize {
        self.hosted.live.get()
    }

    fn free_runner(&self) -> bool {
        let Some(core) = self.core.upgrade() else { return false };
        self.busy_runners(&core) + self.hosted_workers() < machine_runners(&my_config())
    }

    pub fn placement(&self, on: Option<&str>) -> Placed {
        place(on, &self.this_machine(), &self.peers(), self.free_runner())
    }

    pub fn placements(&self) -> (Vec<String>, Vec<String>) {
        let peers = self.peers();
        let me = self.this_machine();
        (peers.iter().map(|p| p.name.clone()).collect(), std::iter::once(&me).chain(&peers).flat_map(|m| m.roles.clone()).collect())
    }

    pub fn on_worker(&self, changed: Box<dyn Fn()>) {
        *self.hosted.on_worker.borrow_mut() = Some(changed);
    }

    pub(super) fn worker_changed(&self) {
        if let Some(changed) = self.hosted.on_worker.borrow().as_ref() {
            changed();
        }
    }

    pub fn offer_worker(&self, offer: Offer) -> Result<String, String> {
        let folder = Path::new(&offer.worktree);
        if self.replica.borrow().me().is_empty() || self.payload.borrow().is_none() {
            return Err("this daemon has no endpoint, so its stages run here".into());
        }
        if git(folder, &["rev-parse", "--quiet", "--verify", "MERGE_HEAD"]).is_ok() {
            return Err("a merge is in progress in its worktree, which can't travel: finish the merge in a stage on this machine first".into());
        }
        let project = identity(&offer.repo).ok_or("the repo has neither a remote nor a first commit to know it by on another machine")?;
        let id = new_id()?;
        let packed = transfer::pack(folder, &id)?;
        let mut replica = self.replica.borrow_mut();
        let (birth, me) = (replica.tick().text(), replica.me().to_string());
        let mut spec = offer.spec;
        spec["branch"] = json!(packed.branch);
        spec["pre"] = json!(packed.pre);
        spec["wip"] = json!(packed.wip);
        let fields = [("kind", json!("worker")), ("project", json!(project)), ("on", json!(offer.on)), ("state", json!("waiting")), ("owner", json!(me)), ("spec", spec)];
        let saved = Saved { id: id.clone(), task: id.clone(), role: OUT.into(), peer: String::new(), state: "offered".into(), doc: json!({ "birth": birth, "worktree": offer.worktree, "repo": offer.repo, "project": project, "on": offer.on, "branch": packed.branch, "pre": packed.pre, "wip": packed.wip }) };
        let ops = replica.stage(&id, &birth, &fields);
        let stored = self.db.atomically(|conn| {
            replica.commit(conn, &ops)?;
            replica::hold_unless_held(conn, &id, &birth, Permit::Ready)?;
            transfer::save(conn, &saved)
        });
        if let Err(error) = stored {
            let _ = transfer::unpack(folder, &packed);
            return Err(format!("couldn't save the worker: {error}"));
        }
        replica.applied(&ops);
        self.sync_at.set(Instant::now());
        log!("kanna-daemon: {id} is offered to {} from {}", offer.on, offer.worktree);
        Ok(id)
    }

    pub fn worker(&self, id: &str) -> Option<Remote> {
        let saved = transfer::saved(&self.db, id).filter(|s| s.role == OUT)?;
        let replica = self.replica.borrow();
        let record = replica.record(id, saved.text("birth")?)?;
        let machines = self.machines();
        let now = Instant::now();
        let host = record.text("owner").filter(|owner| *owner != replica.me());
        let state = if saved.state == "failed" { "failed" } else { record.text("state").unwrap_or("waiting") }.to_string();
        let on = saved.text("on").unwrap_or("any");
        let waits = match (state.as_str(), host) {
            ("waiting", _) => Some(match machines.peers.iter().find(|p| p.name == on) {
                Some(peer) if self.is_away(&peer.key, now) => format!("waits for {on}, which is offline"),
                Some(_) => format!("waits for {on} to take it"),
                None => {
                    let able: Vec<&Peer> = machines.peers.iter().filter(|p| admits(on, &self.machine_of(p))).collect();
                    let offline = !able.is_empty() && able.iter().all(|p| self.is_away(&p.key, now));
                    format!("waits for {}{}", if on == "any" { "one of your machines".to_string() } else { format!("a machine with the role {on}") }, if offline { ", all offline" } else { " to take it" })
                }
            }),
            ("started", Some(host)) if self.is_away(host, now) => Some(format!("runs on {}, which is offline", machines.name_of(host))),
            _ => None,
        };
        Some(Remote {
            state,
            machine: record.text("machine").map(String::from).or_else(|| host.map(|h| machines.name_of(h))),
            terminal: record.text("terminal").map(String::from),
            result: Some(record.get("result").clone()).filter(Value::is_object).or_else(|| saved.text("why").map(|why| json!({ "stopped": why }))),
            landed: saved.state == "landed",
            waits,
        })
    }

    pub fn drop_worker(&self, id: &str) {
        let Some(mut saved) = transfer::saved(&self.db, id).filter(|s| s.role == OUT && s.state != "done" && s.state != "failed") else { return };
        let birth = saved.text("birth").unwrap_or_default().to_string();
        let fields: Vec<(&str, Value)> = if saved.state == "landed" {
            saved.state = "done".into();
            vec![("closed", json!(true))]
        } else {
            if let (Some(worktree), Some(packed)) = (saved.text("worktree"), saved.packed()) {
                if let Err(error) = transfer::unpack(Path::new(worktree), &packed) {
                    log!("kanna-daemon: couldn't take {id}'s work-in-progress commit off {worktree}: {error}");
                }
            }
            saved.state = "failed".into();
            log!("kanna-daemon: {id} is called off");
            vec![("cancelled", json!(true))]
        };
        if let Err(error) = self.replica.borrow_mut().write(&self.db, id, &birth, &fields).and_then(|_| self.db.atomically(|conn| transfer::save(conn, &saved))) {
            log!("kanna-daemon: couldn't save that {id} is over: {error}");
        }
        let _ = fs::remove_file(transfer::outbox(&self.state).join(format!("{id}.bin")));
        self.sync_at.set(Instant::now());
    }

    pub(super) fn grant_worker(&self, peer: &str, id: &str, birth: &str, args: &Value) -> Result<Map, KmuxError> {
        let machines = self.machines();
        let refuse = |code: &str, why: String| Err(KmuxError::new(code, why));
        let Some(mut saved) = transfer::saved(&self.db, id).filter(|s| s.role == OUT && s.state == "offered" && s.text("birth") == Some(birth)) else {
            return refuse("not_owner", format!("{id} isn't waiting for a machine here"));
        };
        let Some(asker) = machines.peers.iter().find(|p| p.key == peer).map(|p| self.machine_of(p)) else {
            return refuse("not_there", format!("{id} runs on one of this machine's own machines"));
        };
        let on = saved.text("on").unwrap_or("any").to_string();
        if !admits(&on, &asker) {
            return refuse("not_there", format!("{id} runs on {on}, which {} isn't", asker.name));
        }
        saved.state = "sending".into();
        saved.peer = peer.to_string();
        saved.doc["heads"] = json!(strings(args, "heads"));
        let grant = self.replica.borrow_mut().grant(&self.db, id, birth, peer, |conn| transfer::save(conn, &saved)).map_err(|why| KmuxError::new("refused", format!("{id}: {why}")))?;
        log!("kanna-daemon: granted {id} to {}", asker.name);
        self.sync_at.set(Instant::now());
        self.send_worker(saved);
        Ok(Map::from_iter([("epoch".to_string(), json!(grant.epoch))]))
    }

    fn take_back(&self, saved: &Saved, why: &str) {
        let birth = saved.text("birth").unwrap_or_default().to_string();
        let offered = Saved { state: "offered".into(), peer: String::new(), ..saved.clone() };
        if let Err(error) = self.replica.borrow_mut().retake(&self.db, &saved.id, &birth).and_then(|_| self.db.atomically(|conn| transfer::save(conn, &offered))) {
            log!("kanna-daemon: couldn't take {} back: {error}", saved.id);
        }
        log!("kanna-daemon: {} didn't take {}: {why}", self.machines().name_of(&saved.peer), saved.id);
        let _ = fs::remove_file(transfer::outbox(&self.state).join(format!("{}.bin", saved.id)));
        self.sync_at.set(Instant::now());
    }

    fn send_worker(&self, saved: Saved) {
        let path = transfer::outbox(&self.state).join(format!("{}.bin", saved.id));
        if !path.is_file() {
            let made = fs::create_dir_all(transfer::outbox(&self.state)).map_err(|e| e.to_string()).and_then(|_| transfer::bundle(Path::new(saved.text("worktree").unwrap_or_default()), saved.text("branch").unwrap_or_default(), &strings(&saved.doc, "heads"), &path));
            if let Err(why) = made {
                return self.take_back(&saved, &format!("its branch couldn't be bundled: {why}"));
            }
        }
        let birth = saved.text("birth").unwrap_or_default();
        let epoch = Replica::authority(&self.db, &saved.id, birth).ok().flatten().map_or(0, |a| a.epoch);
        let args = json!({ "worker": saved.id, "birth": birth, "epoch": epoch, "project": saved.doc["project"], "branch": saved.doc["branch"], "pre": saved.doc["pre"], "wip": saved.doc["wip"] });
        self.send_payload(&saved.peer, &saved.id, args, path, Asked::WorkSent(saved.id.clone()));
    }

    fn send_payload(&self, peer: &str, id: &str, args: Value, path: PathBuf, asked: Asked) {
        let token = self.next.get() + 1;
        self.next.set(token);
        self.asked.borrow_mut().insert(token, asked);
        match self.payload.borrow().as_ref() {
            Some(send) => send(peer.to_string(), id.to_string(), args, path, token),
            None => {
                self.asked.borrow_mut().remove(&token);
            }
        }
    }

    pub(super) fn resend_workers(&self) {
        let sending: HashSet<String> = self.asked.borrow().values().filter_map(|a| if let Asked::WorkSent(id) | Asked::WorkBack(id) = a { Some(id.clone()) } else { None }).collect();
        let machines = self.machines();
        let now = Instant::now();
        let reachable = |peer: &str| machines.peers.iter().any(|p| p.key == peer) && !self.is_away(peer, now);
        for saved in transfer::under_way(&self.db, OUT) {
            if saved.state == "sending" && !sending.contains(&saved.id) && reachable(&saved.peer) {
                self.send_worker(saved);
            }
        }
        for saved in transfer::under_way(&self.db, BACK) {
            if !sending.contains(&saved.id) && reachable(&saved.peer) {
                self.send_back(saved);
            }
        }
    }

    pub(super) fn worker_sent(&self, id: String, reply: Result<Value, String>) {
        let Some(mut saved) = transfer::saved(&self.db, &id).filter(|s| s.role == OUT && s.state == "sending") else { return };
        match reply {
            Ok(_) => {
                let birth = saved.text("birth").unwrap_or_default().to_string();
                let epoch = Replica::authority(&self.db, &id, &birth).ok().flatten().map_or(0, |a| a.epoch);
                saved.state = "sent".into();
                if let Err(error) = Replica::acked(&self.db, &id, &birth, epoch).and_then(|_| self.db.atomically(|conn| transfer::save(conn, &saved))) {
                    log!("kanna-daemon: couldn't save that {id} was taken: {error}");
                }
                let _ = fs::remove_file(transfer::outbox(&self.state).join(format!("{id}.bin")));
                self.came_back(&saved.peer);
                self.worker_changed();
            }
            Err(why) if why.ends_with("[unreachable]") => self.went_away(saved.peer.clone()),
            Err(why) => self.take_back(&saved, &why),
        }
    }

    pub(super) fn seek_workers(&self) {
        let Some(core) = self.core.upgrade() else { return };
        let machines = self.machines();
        let me = self.this_machine();
        let now = Instant::now();
        let seeking: HashSet<String> = self.asked.borrow().values().filter_map(|a| if let Asked::Seek { id, .. } = a { Some(id.clone()) } else { None }).collect();
        let mut free = machine_runners(&my_config()).saturating_sub(self.busy_runners(&core) + self.hosted_workers() + seeking.len());
        let mut asks = vec![];
        {
            let replica = self.replica.borrow();
            for record in replica.workers() {
                if free == 0 {
                    break;
                }
                let owner = record.text("owner").unwrap_or_default();
                let open = record.text("state") == Some("waiting") && record.get("cancelled") != &Value::Bool(true);
                if !open || owner == replica.me() || !machines.peers.iter().any(|p| p.key == owner) || self.is_away(owner, now) || seeking.contains(&record.task) || self.refused.borrow().get(&record.task).is_some_and(|until| *until > now) {
                    continue;
                }
                if !record.text("on").is_some_and(|on| admits(on, &me)) || transfer::saved(&self.db, &record.task).is_some() {
                    continue;
                }
                let spec = record.get("spec");
                if string(spec, "kind") == Some("agent") && !self.runs_agent(string(spec, "agent").unwrap_or_default()) {
                    continue;
                }
                let Some(repo) = record.text("project").and_then(|project| self.clone_of(project)) else { continue };
                free -= 1;
                asks.push((record.task.clone(), record.birth.clone(), owner.to_string(), repo));
            }
        }
        for (id, birth, owner, repo) in asks {
            log!("kanna-daemon: asks {} for {id}", machines.name_of(&owner));
            let args = json!({ "action": "ask", "task": id, "birth": birth, "heads": transfer::heads(Path::new(&repo)) });
            self.send_to(&owner.clone(), args, Asked::Seek { id, owner });
        }
    }

    fn runs_agent(&self, agent: &str) -> bool {
        let read = |value: &Value| value.as_object().map(|map| map.get(agent).and_then(Value::as_str).is_some_and(|c| !c.trim().is_empty()));
        crate::accounts::has_credential(&self.state, agent) || configured(std::slice::from_ref(&my_config()), "pipelines", "agents", read, "an object of agent names to commands") == Some(true)
    }

    pub(super) fn sought(&self, id: String, owner: String, reply: Result<Value, String>) {
        match reply {
            Ok(_) => {
                self.came_back(&owner);
                self.refused.borrow_mut().insert(id, Instant::now() + REFUSED_WAIT);
            }
            Err(why) if why.ends_with("[unreachable]") => self.went_away(owner),
            Err(why) => {
                log!("kanna-daemon: {} didn't grant {id}: {why}", self.machines().name_of(&owner));
                self.refused.borrow_mut().insert(id, Instant::now() + REFUSED_WAIT);
            }
        }
    }

    pub(super) fn take_worker(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(peer) = string(args, kanna_net::PEER).map(String::from) else { return responder.reply(Err(bad("a worker comes from another machine's daemon, over a transfer stream"))) };
        self.came_back(&peer);
        let (id, file) = (string(args, "worker").unwrap_or_default().to_string(), string(args, "file").unwrap_or_default().to_string());
        if !valid_worker(&id) {
            let _ = fs::remove_file(&file);
            return responder.reply(Err(bad(format!("\"{id}\" isn't a worker's ID"))));
        }
        if args.get("back") == Some(&Value::Bool(true)) {
            return self.take_result(&id, &peer, args, &file, responder);
        }
        if let Some(saved) = transfer::saved(&self.db, &id).filter(|s| s.role == IN && s.state != "receiving") {
            let _ = fs::remove_file(&file);
            return responder.reply(match saved.state.as_str() {
                "failed" => Err(KmuxError::new("failed", saved.text("why").unwrap_or("it couldn't be taken").to_string())),
                _ => Ok(Map::from_iter([("folder".to_string(), json!(saved.text("folder")))])),
            });
        }
        let Some(repo) = string(args, "project").and_then(|project| self.clone_of(project)) else {
            let _ = fs::remove_file(&file);
            return responder.reply(Err(KmuxError::new("no_clone", format!("{} has no clone of {}", self.machines().me.name, string(args, "project").unwrap_or("the project")))));
        };
        let (Some(branch), Some(packed)) = (string(args, "branch").map(String::from).filter(|b| moving::valid_branch(Path::new(&repo), b)), Saved { doc: args.clone(), ..Default::default() }.packed()) else {
            let _ = fs::remove_file(&file);
            return responder.reply(Err(bad("a worker needs its branch and commits")));
        };
        let doc = json!({ "birth": string(args, "birth"), "epoch": args["epoch"], "repo": repo, "branch": branch, "pre": packed.pre, "wip": packed.wip });
        let saved = Saved { id: id.clone(), task: id.clone(), role: IN.into(), peer: peer.clone(), state: "receiving".into(), doc };
        if let Err(error) = self.db.atomically(|conn| transfer::save(conn, &saved)) {
            return responder.reply(Err(KmuxError::new("failed", format!("couldn't save the worker {id}: {error}"))));
        }
        self.hosted.live.set(self.hosted.live.get() + 1);
        let token = self.next.get() + 1;
        self.next.set(token);
        self.asked.borrow_mut().insert(token, Asked::WorkUnbundled(Taking { id: id.clone(), responder }));
        let events = self.events.clone();
        log!("kanna-daemon: {id} comes from {} into {repo}", self.machines().name_of(&peer));
        std::thread::spawn(move || {
            let taken = transfer::unbundle(Path::new(&repo), &id, Path::new(&file), &id, &branch, &packed, None).map(|(folder, _)| json!({ "folder": folder }));
            let _ = fs::remove_file(&file);
            let _ = events.send(Event::Replica { token, reply: taken });
        });
    }

    pub(super) fn work_unbundled(self: &Rc<Self>, taking: Taking, reply: Result<Value, String>) {
        let Some(mut saved) = transfer::saved(&self.db, &taking.id) else { return taking.responder.reply(Err(gone())) };
        let birth = saved.text("birth").unwrap_or_default().to_string();
        let taken = reply.and_then(|made| {
            saved.doc["folder"] = made["folder"].clone();
            saved.state = "installed".into();
            self.replica.borrow_mut().receive(&self.db, &saved.id, &birth, saved.doc["epoch"].as_i64().unwrap_or(0)).and_then(|_| self.db.atomically(|conn| transfer::save(conn, &saved))).map_err(|e| e.to_string())
        });
        match taken {
            Ok(()) => {
                log!("kanna-daemon: {}'s worktree is {}", saved.id, saved.text("folder").unwrap_or(""));
                taking.responder.reply(Ok(Map::from_iter([("folder".to_string(), saved.doc["folder"].clone())])));
                self.sync_at.set(Instant::now());
                self.start_worker(&saved.id);
            }
            Err(why) => {
                log!("kanna-daemon: couldn't take {} here: {why}", saved.id);
                saved.state = "failed".into();
                saved.doc["why"] = json!(why);
                let _ = self.db.atomically(|conn| transfer::save(conn, &saved));
                self.hosted.live.set(self.hosted.live.get().saturating_sub(1));
                taking.responder.reply(Err(KmuxError::new("failed", why)));
            }
        }
    }

    fn start_worker(self: &Rc<Self>, id: &str) {
        let Some(core) = self.core.upgrade() else { return };
        let Some(mut saved) = transfer::saved(&self.db, id).filter(|s| s.role == IN && s.state == "installed") else { return };
        let birth = saved.text("birth").unwrap_or_default().to_string();
        let Some(record) = self.replica.borrow().record(id, &birth).cloned() else { return };
        if record.get("cancelled") == &Value::Bool(true) {
            return self.clear_worker(&saved);
        }
        if !self.hosted.opening.borrow_mut().insert(id.to_string()) {
            return;
        }
        let spec = record.get("spec").clone();
        let text = |key: &str| string(&spec, key).unwrap_or_default().to_string();
        let folder = PathBuf::from(saved.text("folder").unwrap_or_default());
        let repo = saved.text("repo").unwrap_or_default().to_string();
        let scratch = folder.join(SCRATCH);
        let n = spec["n"].as_u64().unwrap_or(1);
        let mut vars = vec![
            ("KANNA_TASK", text("task")),
            ("KANNA_TASK_NAME", text("taskName")),
            ("KANNA_REPO", repo.clone()),
            ("KANNA_WORKTREE", folder.to_string_lossy().into_owned()),
            ("KANNA_BRANCH", saved.text("branch").unwrap_or_default().to_string()),
            ("KANNA_BASE", text("base")),
            ("KANNA_RUN", text("run")),
            ("KANNA_STAGE", text("stage")),
        ];
        if let Some(instance) = string(&spec, "instance") {
            vars.push(("KANNA_INSTANCE", instance.to_string()));
        }
        let files = config_files(Path::new(&repo));
        let made = fs::create_dir_all(&scratch).map_err(|e| format!("can't make {}: {e}", scratch.display())).and_then(|_| match string(&spec, "kind") {
            Some("run") => {
                let (log, exit, feedback) = (scratch.join(format!("stage-{n}.log")), scratch.join(format!("stage-{n}.exit")), scratch.join(format!("feedback-{n}.txt")));
                let _ = fs::remove_file(&exit);
                let _ = fs::write(&feedback, text("feedback"));
                vars.push(("KANNA_FEEDBACK", feedback.to_string_lossy().into_owned()));
                saved.doc["log"] = json!(log.to_string_lossy());
                saved.doc["exit"] = json!(exit.to_string_lossy());
                Ok(task_scripts::stage_command(&vars, &text("command"), spec["within"].as_u64().unwrap_or(0), &log, &exit))
            }
            Some("agent") => {
                let agent = text("agent");
                let answer = scratch.join(format!("answer-{n}.md"));
                fs::write(scratch.join(format!("stage-{n}.md")), text("prompt")).map_err(|e| format!("can't write the stage's prompt: {e}"))?;
                vars.push(("KANNA_ANSWER", answer.to_string_lossy().into_owned()));
                vars.push(("KANNA_CLI", cli_path()));
                let session = names_its_session(&agent).then(session_id).flatten();
                let first = format!("\"$(cat {})\"", quote(&format!("{SCRATCH}/stage-{n}.md")));
                let exports: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect();
                saved.doc["agent"] = json!(agent);
                saved.doc["answer"] = json!(answer.to_string_lossy());
                Ok(format!("export {}; {}", exports.join(" "), stage_agent(&pipeline_agent(&files, &agent), string(&spec, "effort"), string(&spec, "model"), prompts(&files), session.as_deref(), false, &first)))
            }
            other => Err(format!("{other:?} isn't a kind of stage this machine runs")),
        });
        let line = match made.and_then(|line| Replica::start(&self.db, id, &birth).map(|_| line)) {
            Ok(line) => line,
            Err(why) => {
                self.hosted.opening.borrow_mut().remove(id);
                return self.end_worker(&saved, json!({ "stopped": why }));
            }
        };
        let (this, id) = (Rc::downgrade(self), id.to_string());
        let args = json!({ "type": "term", "dock": "left", "frozen": true, "shelf": true, "task": id, "cwd": folder.to_string_lossy(), "cmd": line, "wait": false });
        core.handle(json!({ "cmd": "open", "args": args }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            this.hosted.opening.borrow_mut().remove(&id);
            let Some(pane) = reply.get("pane").and_then(|p| string(p, "id")).filter(|_| reply.get("ok") == Some(&Value::Bool(true))).map(String::from) else {
                let why = reply.get("error").and_then(|e| string(e, "message")).unwrap_or("it failed").to_string();
                return this.end_worker(&saved, json!({ "stopped": format!("can't open a terminal on {}: {why}", this.machines().me.name) }));
            };
            saved.state = "started".into();
            saved.doc["pane"] = json!(pane);
            let fields = [("state", json!("started")), ("machine", json!(this.machines().me.name)), ("terminal", json!(pane))];
            if let Err(error) = this.db.atomically(|conn| transfer::save(conn, &saved)).and_then(|_| this.replica.borrow_mut().write(&this.db, &id, &birth, &fields)) {
                log!("kanna-daemon: couldn't save that {id} started: {error}");
            }
            log!("kanna-daemon: {id} runs {} in {pane}", string(&record.get("spec").clone(), "label").unwrap_or("its stage"));
            this.sync_at.set(Instant::now());
        });
    }

    pub fn worker_done(self: &Rc<Self>, args: &Value) -> Option<Result<Map, KmuxError>> {
        let by = string(args, "by")?;
        let mut saved = transfer::under_way(&self.db, IN).into_iter().find(|s| s.state == "started" && s.text("pane") == Some(by) && s.text("agent").is_some())?;
        let birth = saved.text("birth").unwrap_or_default().to_string();
        let spec = self.replica.borrow().record(&saved.id, &birth).map(|r| r.get("spec").clone()).unwrap_or(Value::Null);
        let stage = string(&spec, "label").unwrap_or("this stage").to_string();
        if string(args, "action") != Some("done") {
            return Some(Err(KmuxError::new("unsupported", format!("{stage} runs on another machine than its run, where a stage can't ask for you or for a command yet: finish it, or report what's missing in your answer and run `\"$KANNA_CLI\" run done`"))));
        }
        if saved.doc.get("reported").is_some_and(Value::is_object) {
            return Some(Err(KmuxError::new("ended", format!("{stage} has reported already"))));
        }
        let verdict = match (string(args, "verdict"), spec["verdicts"] == true) {
            (Some(verdict @ ("pass" | "fail")), true) => verdict,
            (None, false) => "done",
            (_, true) => return Some(Err(bad(format!("{stage} gives a verdict: kanna run done pass, or kanna run done fail")))),
            (Some(_), false) => return Some(Err(bad(format!("{stage} gives no verdict: kanna run done")))),
        };
        saved.doc["reported"] = json!({ "verdict": verdict, "answer": string(args, "answer").unwrap_or(""), "at": wall_seconds() });
        if let Err(error) = self.db.atomically(|conn| transfer::save(conn, &saved)) {
            return Some(Err(KmuxError::new("failed", format!("couldn't save the report: {error}"))));
        }
        log!("kanna-daemon: {}'s {stage} reports {verdict}", saved.id);
        Some(Ok(Map::from_iter([("run".to_string(), spec["run"].clone()), ("stage".to_string(), spec["stage"].clone()), ("verdict".to_string(), json!(verdict))])))
    }

    pub fn worker_status(&self, pane: &str, ended: bool) {
        if ended && self.hosted.live.get() > 0 {
            self.hosted.rested.borrow_mut().insert(pane.to_string());
        }
    }

    pub(super) fn tick_workers(self: &Rc<Self>) -> Option<Instant> {
        if self.hosted.live.get() == 0 {
            return None;
        }
        let Some(core) = self.core.upgrade() else { return None };
        let hosted = transfer::under_way(&self.db, IN);
        self.hosted.live.set(hosted.len());
        for saved in hosted {
            let birth = saved.text("birth").unwrap_or_default().to_string();
            let record = self.replica.borrow().record(&saved.id, &birth).cloned();
            let flag = |key: &str| record.as_ref().is_some_and(|r| r.get(key) == &Value::Bool(true));
            match saved.state.as_str() {
                "installed" => self.start_worker(&saved.id),
                "started" if flag("cancelled") => self.clear_worker(&saved),
                "started" => self.watch_worker(&core, saved),
                "finished" if flag("closed") || flag("cancelled") => self.clear_worker(&saved),
                _ => {}
            }
        }
        Some(Instant::now() + POLL)
    }

    fn watch_worker(self: &Rc<Self>, core: &CoreHandle, saved: Saved) {
        let Some(pane) = saved.text("pane").map(String::from) else { return };
        if self.hosted.ending.borrow().contains(&saved.id) {
            return;
        }
        let running = alive(core, &pane);
        let lost = core.pane(&pane).is_none_or(|p| p.lost());
        let Some(agent) = saved.text("agent").map(String::from) else {
            let (log, exit) = (PathBuf::from(saved.text("log").unwrap_or_default()), PathBuf::from(saved.text("exit").unwrap_or_default()));
            if !exit.is_file() {
                if running {
                    return;
                }
                if lost {
                    return self.end_worker(&saved, json!({ "interrupted": true }));
                }
                let _ = fs::write(&exit, format!("{}\n", core.pane(&pane).and_then(|p| p.exit_code).unwrap_or(1)));
            }
            let code: i32 = fs::read_to_string(&exit).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(1);
            let output = fs::read_to_string(&log).unwrap_or_default();
            let timed_out = task_scripts::timed_out(code);
            let feedback = crate::runs::command_feedback(&output);
            let result = json!({ "verdict": if code == 0 { "pass" } else { "fail" }, "code": code, "feedback": if timed_out { format!("timed out\n{feedback}") } else { feedback }, "failures": crate::runs::failure_lines(&output), "note": timed_out.then_some("timed out"), "output": tail(&output, OUTPUT) });
            return self.end_worker(&saved, result);
        };
        let Some(reported) = saved.doc.get("reported").filter(|r| r.is_object()).cloned() else {
            if lost {
                return self.end_worker(&saved, json!({ "interrupted": true }));
            }
            if !running {
                return self.end_worker(&saved, json!({ "stopped": "its agent exited without reporting it was done" }));
            }
            return;
        };
        let rested = self.hosted.rested.borrow().contains(&pane);
        if running && !rested && wall_seconds() - reported["at"].as_f64().unwrap_or(0.0) < SETTLE {
            return;
        }
        self.hosted.ending.borrow_mut().insert(saved.id.clone());
        let this = Rc::downgrade(self);
        self.end_agent(&pane, &agent, move |_| {
            let Some(this) = this.upgrade() else { return };
            this.hosted.ending.borrow_mut().remove(&saved.id);
            let answer = saved.text("answer").and_then(|path| fs::read_to_string(path).ok()).filter(|a| !a.trim().is_empty()).unwrap_or_else(|| string(&reported, "answer").unwrap_or("").to_string());
            this.end_worker(&saved, json!({ "verdict": reported["verdict"], "answer": answer }));
        });
    }

    fn end_worker(&self, saved: &Saved, mut result: Value) {
        let id = &saved.id;
        let birth = saved.text("birth").unwrap_or_default().to_string();
        if let Some(pane) = saved.text("pane") {
            self.hosted.rested.borrow_mut().remove(pane);
            if let Some(core) = self.core.upgrade() {
                core.handle(json!({ "cmd": "close", "args": { "pane": pane } }), |_| {});
            }
        }
        let folder = PathBuf::from(saved.text("folder").unwrap_or_default());
        let came = saved.text("pre").unwrap_or_default().to_string();
        let path = transfer::outbox(&self.state).join(format!("{id}-back.bin"));
        let packed = fs::create_dir_all(transfer::outbox(&self.state)).map_err(|e| e.to_string()).and_then(|_| transfer::pack(&folder, id)).and_then(|packed| transfer::bundle(&folder, &packed.branch, std::slice::from_ref(&came), &path).map(|_| packed));
        let mut back = None;
        match packed {
            Ok(packed) => {
                result["end"] = json!(packed.pre);
                result["pre"] = json!(packed.pre);
                result["wip"] = json!(packed.wip);
                back = Some(Saved { id: format!("{id}-back"), task: id.clone(), role: BACK.into(), peer: saved.peer.clone(), state: "sending".into(), doc: json!({ "birth": birth, "branch": packed.branch, "pre": packed.pre, "wip": packed.wip }) });
            }
            Err(why) if result.get("stopped").is_none() => result = json!({ "stopped": format!("its work couldn't be packed on {}: {why}", self.machines().me.name) }),
            Err(_) => {}
        }
        let finished = Saved { state: "finished".into(), ..saved.clone() };
        let fields = [("state", json!("done")), ("result", result.clone())];
        let stored = self.db.atomically(|conn| {
            if let Some(back) = &back {
                transfer::save(conn, back)?;
            }
            transfer::save(conn, &finished)
        });
        if let Err(error) = stored.and_then(|_| self.replica.borrow_mut().write(&self.db, id, &birth, &fields)) {
            log!("kanna-daemon: couldn't save how {id} ended: {error}");
        }
        log!("kanna-daemon: {id} ended {}", string(&result, "verdict").or(string(&result, "stopped")).unwrap_or("interrupted"));
        self.sync_at.set(Instant::now());
        if let Some(back) = back {
            self.send_back(back);
        }
    }

    fn send_back(&self, saved: Saved) {
        let path = transfer::outbox(&self.state).join(format!("{}.bin", saved.id));
        let args = json!({ "worker": saved.task, "back": true, "birth": saved.doc["birth"], "branch": saved.doc["branch"], "pre": saved.doc["pre"], "wip": saved.doc["wip"] });
        self.send_payload(&saved.peer, &saved.id, args, path, Asked::WorkBack(saved.id.clone()));
    }

    pub(super) fn worker_back(&self, id: String, reply: Result<Value, String>) {
        let Some(mut saved) = transfer::saved(&self.db, &id).filter(|s| s.role == BACK && s.state == "sending") else { return };
        match reply {
            Ok(_) => {
                saved.state = "done".into();
                let _ = self.db.atomically(|conn| transfer::save(conn, &saved));
                let _ = fs::remove_file(transfer::outbox(&self.state).join(format!("{id}.bin")));
                self.came_back(&saved.peer);
            }
            Err(why) if why.ends_with("[unreachable]") => self.went_away(saved.peer.clone()),
            Err(why) => {
                log!("kanna-daemon: {} didn't take {}'s commits back: {why}", self.machines().name_of(&saved.peer), saved.task);
                saved.state = "failed".into();
                saved.doc["why"] = json!(why);
                let _ = self.db.atomically(|conn| transfer::save(conn, &saved));
            }
        }
    }

    fn clear_worker(&self, saved: &Saved) {
        if let Some(pane) = saved.text("pane") {
            if let Some(core) = self.core.upgrade() {
                core.handle(json!({ "cmd": "close", "args": { "pane": pane } }), |_| {});
            }
        }
        let done = Saved { state: "done".into(), ..saved.clone() };
        let back = transfer::saved(&self.db, &format!("{}-back", saved.id)).filter(|b| b.state == "sending").map(|b| Saved { state: "done".into(), ..b });
        let stored = self.db.atomically(|conn| {
            if let Some(back) = &back {
                transfer::save(conn, back)?;
            }
            transfer::save(conn, &done)
        });
        if let Err(error) = stored {
            log!("kanna-daemon: couldn't save that {} is over: {error}", saved.id);
        }
        let _ = fs::remove_file(transfer::outbox(&self.state).join(format!("{}-back.bin", saved.id)));
        log!("kanna-daemon: {} is over here, so its worktree goes", saved.id);
        let (repo, folder, branch) = (PathBuf::from(saved.text("repo").unwrap_or_default()), saved.text("folder").map(PathBuf::from), saved.text("branch").map(String::from));
        std::thread::spawn(move || {
            if let (Some(folder), Some(branch)) = (folder, branch) {
                if let Err(why) = transfer::remove(&repo, &folder, &branch) {
                    log!("kanna-daemon: kept the worktree {} of a worker that's over: {why}", folder.display());
                }
            }
        });
    }

    fn take_result(&self, id: &str, peer: &str, args: &Value, file: &str, responder: Responder) {
        let reply = self.land(id, peer, args, Path::new(file));
        let _ = fs::remove_file(file);
        if let Err(why) = &reply {
            log!("kanna-daemon: {id}'s commits didn't land: {why}");
            if let Some(mut saved) = transfer::saved(&self.db, id).filter(|s| s.role == OUT && s.state == "sent") {
                saved.doc["why"] = json!(format!("its commits couldn't be taken from {}: {why}", self.machines().name_of(peer)));
                let _ = self.db.atomically(|conn| transfer::save(conn, &saved));
            }
        }
        self.worker_changed();
        responder.reply(reply.map(|_| Map::new()).map_err(|why| KmuxError::new("failed", why)));
    }

    fn land(&self, id: &str, peer: &str, args: &Value, file: &Path) -> Result<(), String> {
        let mut saved = transfer::saved(&self.db, id).filter(|s| s.role == OUT && s.peer == peer).ok_or_else(|| format!("{id} isn't this machine's worker on that machine"))?;
        if saved.state == "landed" || saved.state == "done" {
            return Ok(());
        }
        if saved.state != "sent" && saved.state != "sending" {
            return Err(format!("{id} was called off"));
        }
        let sent = saved.packed().ok_or("the worker's commits aren't known")?;
        let back = Saved { doc: args.clone(), ..Default::default() }.packed().ok_or("a result needs its branch and commits")?;
        let folder = PathBuf::from(saved.text("worktree").unwrap_or_default());
        let file = file.to_string_lossy();
        let private = format!("refs/kanna/transfers/{id}-back");
        git(&folder, &["bundle", "verify", "--quiet", file.as_ref()]).map_err(|e| format!("this clone can't take the bundle: {e}"))?;
        git(&folder, &["fetch", "--quiet", "--no-tags", file.as_ref(), &format!("+refs/heads/{}:{private}", back.branch)])?;
        let landed = (|| {
            if git(&folder, &["rev-parse", "--verify", &private])? != back.wip {
                return Err("the bundle's branch isn't at the commit the result names".to_string());
            }
            git(&folder, &["merge-base", "--is-ancestor", &sent.pre, &back.pre]).map_err(|_| format!("its commits don't follow {}, where the visit started", &sent.pre[..sent.pre.len().min(9)]))?;
            if git(&folder, &["rev-parse", "--verify", "HEAD"])? != sent.wip || !git(&folder, &["status", "--porcelain"])?.is_empty() {
                return Err(format!("{} changed while its stage ran on another machine", folder.display()));
            }
            git(&folder, &["reset", "--hard", "--quiet", &back.wip])?;
            git(&folder, &["reset", "--soft", &back.pre]).map(|_| ())
        })();
        let _ = git(&folder, &["update-ref", "-d", &private]);
        landed?;
        saved.state = "landed".into();
        self.db.atomically(|conn| transfer::save(conn, &saved)).map_err(|e| e.to_string())?;
        log!("kanna-daemon: {id}'s commits are back: {} is at {}", sent.branch, &back.pre[..back.pre.len().min(9)]);
        Ok(())
    }

    pub(super) fn hosting(&self) {
        self.hosted.live.set(transfer::under_way(&self.db, IN).len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(name: &str, roles: &[&str]) -> Machine {
        Machine { name: name.into(), roles: roles.iter().map(|r| r.to_string()).collect() }
    }

    #[test]
    fn a_placement_is_here_a_machine_a_role_or_any_and_an_unknown_one_names_what_there_is() {
        let me = machine("mbp", &["mac"]);
        let peers = [machine("mini", &["e2e", "mac"]), machine("vm", &["linux"])];
        assert_eq!(place(None, &me, &peers, true), Placed::Here);
        assert_eq!(place(Some("here"), &me, &peers, false), Placed::Here);
        assert_eq!(place(Some("mbp"), &me, &peers, false), Placed::Here, "this machine by name runs here, slot or not");
        assert_eq!(place(Some("mini"), &me, &peers, true), Placed::There);
        assert_eq!(place(Some("e2e"), &me, &peers, true), Placed::There, "a role only another machine declares");
        assert_eq!(place(Some("linux"), &me, &peers, true), Placed::There);
        assert_eq!(place(Some("mac"), &me, &peers, true), Placed::Here, "a role this machine declares too runs here while a runner is free");
        assert_eq!(place(Some("mac"), &me, &peers, false), Placed::There);
        assert_eq!(place(Some("any"), &me, &peers, true), Placed::Here);
        assert_eq!(place(Some("any"), &me, &peers, false), Placed::There);
        assert_eq!(place(Some("any"), &me, &[], false), Placed::Here, "with no other machine, any is here");
        assert_eq!(place(Some("gpu"), &me, &peers, true), Placed::Nowhere("no such machine: none of your machines is named gpu or declares the role gpu (machines: mbp, mini, vm; roles: e2e, linux, mac)".into()));
        assert!(admits("any", &peers[1]) && admits("vm", &peers[1]) && admits("linux", &peers[1]) && !admits("e2e", &peers[1]));
    }

    #[test]
    fn a_machine_declares_its_roles_in_its_own_settings_beside_the_one_its_system_gives_it() {
        let dir = std::env::temp_dir().join(format!("kanna-roles-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        let system = if cfg!(target_os = "macos") { "mac" } else { "linux" };
        assert_eq!(declared_roles(&file), vec![system.to_string()]);
        fs::write(&file, r#"{"machine": {"roles": ["e2e", "Bad Role", "gpu-1", "e2e"]}}"#).unwrap();
        let mut expected = vec!["e2e".to_string(), "gpu-1".to_string(), system.to_string()];
        expected.sort();
        assert_eq!(declared_roles(&file), expected);
        assert!(valid_worker("work-0000beef") && !valid_worker("task-0000beef") && !valid_worker("work-../../x"));
        let _ = fs::remove_dir_all(&dir);
    }
}
