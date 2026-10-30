use crate::db::Db;
use kanna_mux::json::{json, Map, Value};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_OPS: usize = 2000;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hlc {
    pub ms: i64,
    pub n: i64,
    pub device: String,
}

impl Hlc {
    pub fn text(&self) -> String {
        format!("{}.{}.{}", self.ms, self.n, self.device)
    }

    pub fn parse(text: &str) -> Option<Hlc> {
        let mut parts = text.splitn(3, '.');
        let ms = parts.next()?.parse().ok()?;
        let n = parts.next()?.parse().ok()?;
        Some(Hlc { ms, n, device: parts.next()?.to_string() })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    pub at: Hlc,
    pub task: String,
    pub birth: String,
    pub field: String,
    pub value: Value,
}

impl Op {
    pub fn to_value(&self) -> Value {
        json!({ "device": self.at.device, "ms": self.at.ms, "n": self.at.n, "task": self.task, "birth": self.birth, "field": self.field, "value": self.value })
    }

    pub fn from_value(value: &Value) -> Option<Op> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
        let at = Hlc { ms: value.get("ms")?.as_i64()?, n: value.get("n")?.as_i64()?, device: text("device")? };
        Some(Op { at, task: text("task")?, birth: text("birth")?, field: text("field")?, value: value.get("value").cloned().unwrap_or(Value::Null) })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    pub task: String,
    pub birth: String,
    fields: BTreeMap<String, (Hlc, Value)>,
}

impl Record {
    pub fn get(&self, field: &str) -> &Value {
        self.fields.get(field).map_or(&Value::Null, |(_, value)| value)
    }

    pub fn text(&self, field: &str) -> Option<&str> {
        self.get(field).as_str().filter(|s| !s.is_empty())
    }

    pub fn list(&self, field: &str) -> Vec<String> {
        self.get(field).as_array().map_or(vec![], |items| items.iter().filter_map(Value::as_str).map(String::from).collect())
    }

    pub fn born(&self) -> Hlc {
        Hlc::parse(&self.birth).unwrap_or(Hlc { ms: 0, n: 0, device: self.birth.clone() })
    }

    pub fn to_value(&self) -> Value {
        let mut value = json!({ "task": self.task, "birth": self.birth });
        for (field, (_, v)) in &self.fields {
            value[field.as_str()] = v.clone();
        }
        value
    }

    fn see(&mut self, op: &Op) -> bool {
        match self.fields.get(&op.field) {
            Some((at, _)) if *at >= op.at => false,
            _ => {
                self.fields.insert(op.field.clone(), (op.at.clone(), op.value.clone()));
                true
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permit {
    Ready,
    Running,
    Sent,
    Acked,
}

impl Permit {
    fn as_str(self) -> &'static str {
        match self {
            Permit::Ready => "ready",
            Permit::Running => "running",
            Permit::Sent => "sent",
            Permit::Acked => "acked",
        }
    }

    fn parse(text: &str) -> Permit {
        match text {
            "ready" => Permit::Ready,
            "running" => Permit::Running,
            "sent" => Permit::Sent,
            _ => Permit::Acked,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Authority {
    pub epoch: i64,
    pub permit: Permit,
    pub to: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grant {
    pub task: String,
    pub birth: String,
    pub epoch: i64,
    pub to: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Peer {
    pub key: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Roster {
    pub me: Peer,
    pub peers: Vec<Peer>,
}

impl Roster {
    pub fn name_of(&self, key: &str) -> String {
        std::iter::once(&self.me).chain(&self.peers).find(|p| p.key == key).map_or_else(|| key.chars().take(10).collect(), |p| p.name.clone())
    }
}

pub fn wall() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

pub struct Replica {
    me: String,
    clock: (i64, i64),
    seen: HashMap<String, (i64, i64)>,
    records: BTreeMap<(String, String), Record>,
}

impl Replica {
    pub fn empty() -> Replica {
        Replica { me: String::new(), clock: (0, 0), seen: HashMap::new(), records: BTreeMap::new() }
    }

    pub fn open(db: &Db) -> rusqlite::Result<Replica> {
        db.atomically(|conn| {
            let clock: Option<String> = conn.query_row("select value from meta where key = 'hlc'", [], |r| r.get(0)).optional()?;
            let clock = clock.and_then(|c| serde_json::from_str::<(i64, i64)>(&c).ok()).unwrap_or((0, 0));
            let mut replica = Replica { clock, ..Replica::empty() };
            let mut statement = conn.prepare("select device, ms, n, task, birth, field, value from record_ops")?;
            let ops = statement.query_map([], op_row)?.collect::<rusqlite::Result<Vec<Op>>>()?;
            for op in &ops {
                replica.project(op);
            }
            Ok(replica)
        })
    }

    pub fn me(&self) -> &str {
        &self.me
    }

    pub fn set_device(&mut self, device: &str) {
        self.me = device.to_string();
    }

    pub fn tick(&mut self) -> Hlc {
        let ms = wall().max(self.clock.0);
        let n = if ms == self.clock.0 { self.clock.1 + 1 } else { 0 };
        self.clock = (ms, n);
        Hlc { ms, n, device: self.me.clone() }
    }

    fn project(&mut self, op: &Op) -> bool {
        let seen = self.seen.entry(op.at.device.clone()).or_insert((op.at.ms, op.at.n));
        *seen = (*seen).max((op.at.ms, op.at.n));
        self.clock = self.clock.max((op.at.ms, op.at.n));
        let record = self.records.entry((op.task.clone(), op.birth.clone())).or_insert_with(|| Record { task: op.task.clone(), birth: op.birth.clone(), ..Default::default() });
        record.see(op)
    }

    pub fn record(&self, task: &str, birth: &str) -> Option<&Record> {
        self.records.get(&(task.to_string(), birth.to_string()))
    }

    pub fn births(&self, task: &str) -> Vec<&Record> {
        self.records.range((task.to_string(), String::new())..).take_while(|((t, _), _)| t == task).map(|(_, r)| r).collect()
    }

    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.records.values()
    }

    pub fn open_task(&self, task: &str) -> bool {
        self.births(task).iter().any(|r| r.text("state").is_some_and(|s| s != "done"))
    }

    pub fn stage(&mut self, task: &str, birth: &str, fields: &[(&str, Value)]) -> Vec<Op> {
        let current = self.record(task, birth).cloned().unwrap_or_default();
        let changed: Vec<(&str, Value)> = fields.iter().filter(|(field, value)| current.fields.get(*field).is_none_or(|(_, v)| v != value)).cloned().collect();
        changed.into_iter().map(|(field, value)| Op { at: self.tick(), task: task.to_string(), birth: birth.to_string(), field: field.to_string(), value }).collect()
    }

    pub fn commit(&mut self, conn: &Connection, ops: &[Op]) -> rusqlite::Result<usize> {
        let mut added = 0;
        for op in ops {
            added += conn.execute(
                "insert or ignore into record_ops (device, ms, n, task, birth, field, value) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![op.at.device, op.at.ms, op.at.n, op.task, op.birth, op.field, op.value.to_string()],
            )?;
        }
        conn.execute("insert into meta (key, value) values ('hlc', ?1) on conflict (key) do update set value = excluded.value", [json!([self.clock.0, self.clock.1]).to_string()])?;
        Ok(added)
    }

    pub fn applied(&mut self, ops: &[Op]) -> usize {
        ops.iter().filter(|op| self.project(op)).count()
    }

    pub fn write(&mut self, db: &Db, task: &str, birth: &str, fields: &[(&str, Value)]) -> rusqlite::Result<usize> {
        let ops = self.stage(task, birth, fields);
        if ops.is_empty() {
            return Ok(0);
        }
        db.atomically(|conn| self.commit(conn, &ops))?;
        Ok(self.applied(&ops))
    }

    pub fn apply(&mut self, db: &Db, ops: &[Op]) -> rusqlite::Result<usize> {
        let mut sorted = ops.to_vec();
        sorted.sort_by(|a, b| a.at.cmp(&b.at));
        let added = db.atomically(|conn| {
            for op in &sorted {
                self.clock = self.clock.max((op.at.ms, op.at.n));
            }
            self.commit(conn, &sorted)
        })?;
        self.applied(&sorted);
        Ok(added)
    }

    pub fn vector(&self) -> Value {
        Value::Object(self.seen.iter().map(|(device, (ms, n))| (device.clone(), json!([ms, n]))).collect())
    }

    pub fn ops_after(&self, db: &Db, vector: &Value) -> rusqlite::Result<Vec<Op>> {
        db.atomically(|conn| {
            let mut out = vec![];
            for device in self.seen.keys() {
                let (ms, n) = vector.get(device).and_then(|v| serde_json::from_value::<(i64, i64)>(v.clone()).ok()).unwrap_or((i64::MIN, i64::MIN));
                let mut statement = conn.prepare("select device, ms, n, task, birth, field, value from record_ops where device = ?1 and (ms > ?2 or (ms = ?2 and n > ?3)) order by ms, n limit ?4")?;
                let ops = statement.query_map(params![device, ms, n, (MAX_OPS - out.len()) as i64], op_row)?.collect::<rusqlite::Result<Vec<Op>>>()?;
                out.extend(ops);
                if out.len() >= MAX_OPS {
                    break;
                }
            }
            Ok(out)
        })
    }

    pub fn authority(db: &Db, task: &str, birth: &str) -> rusqlite::Result<Option<Authority>> {
        db.atomically(|conn| authority(conn, task, birth))
    }

    pub fn hold(db: &Db, task: &str, birth: &str, permit: Permit) -> rusqlite::Result<()> {
        db.atomically(|conn| {
            let epoch = authority(conn, task, birth)?.map_or(0, |a| a.epoch);
            set_authority(conn, task, birth, &Authority { epoch, permit, to: None })
        })
    }

    pub fn start(db: &Db, task: &str, birth: &str) -> Result<(), String> {
        let held = Replica::authority(db, task, birth).map_err(|e| e.to_string())?;
        match held {
            Some(Authority { permit: Permit::Ready, .. }) => Replica::hold(db, task, birth, Permit::Running).map_err(|e| e.to_string()),
            Some(Authority { permit: Permit::Running, .. }) => Ok(()),
            Some(Authority { to, .. }) => Err(format!("it was given to {}", to.unwrap_or_else(|| "another machine".into()))),
            None => Err("this machine was never given it".into()),
        }
    }

    pub fn grant(&mut self, db: &Db, task: &str, birth: &str, to: &str, also: impl FnOnce(&Connection) -> rusqlite::Result<()>) -> Result<Grant, String> {
        let held = Replica::authority(db, task, birth).map_err(|e| e.to_string())?;
        let epoch = match held {
            Some(Authority { permit: Permit::Ready, epoch, .. }) => epoch + 1,
            Some(Authority { permit: Permit::Running, .. }) => return Err("started".into()),
            Some(Authority { to, .. }) => return Err(format!("given to {}", to.unwrap_or_default())),
            None => return Err("not_owner".into()),
        };
        let ops = self.stage(task, birth, &[("owner", json!(to))]);
        db.atomically(|conn| {
            set_authority(conn, task, birth, &Authority { epoch, permit: Permit::Sent, to: Some(to.to_string()) })?;
            self.commit(conn, &ops)?;
            also(conn)
        })
        .map_err(|e| e.to_string())?;
        self.applied(&ops);
        Ok(Grant { task: task.to_string(), birth: birth.to_string(), epoch, to: to.to_string() })
    }

    pub fn receive(&mut self, db: &Db, task: &str, birth: &str, epoch: i64) -> rusqlite::Result<bool> {
        if Replica::authority(db, task, birth)?.is_some_and(|a| a.epoch >= epoch) {
            return Ok(false);
        }
        let me = self.me.clone();
        let ops = self.stage(task, birth, &[("owner", json!(me))]);
        db.atomically(|conn| {
            set_authority(conn, task, birth, &Authority { epoch, permit: Permit::Ready, to: None })?;
            self.commit(conn, &ops)
        })?;
        self.applied(&ops);
        Ok(true)
    }

    pub fn acked(db: &Db, task: &str, birth: &str, epoch: i64) -> rusqlite::Result<()> {
        db.atomically(|conn| {
            match authority(conn, task, birth)? {
                Some(held) if held.permit == Permit::Sent && held.epoch == epoch => set_authority(conn, task, birth, &Authority { permit: Permit::Acked, ..held }),
                _ => Ok(()),
            }
        })
    }

    pub fn outbox(db: &Db) -> rusqlite::Result<Vec<Grant>> {
        db.atomically(|conn| {
            let mut statement = conn.prepare("select task, birth, epoch, recipient from authority where permit = 'sent'")?;
            let grants = statement.query_map([], |r| Ok(Grant { task: r.get(0)?, birth: r.get(1)?, epoch: r.get(2)?, to: r.get::<_, Option<String>>(3)?.unwrap_or_default() }))?;
            grants.collect()
        })
    }

    pub fn held(db: &Db) -> rusqlite::Result<Vec<(String, String, Authority)>> {
        db.atomically(|conn| {
            let mut statement = conn.prepare("select task, birth, epoch, permit, recipient from authority")?;
            let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, Authority { epoch: r.get(2)?, permit: Permit::parse(&r.get::<_, String>(3)?), to: r.get(4)? })))?;
            rows.collect()
        })
    }
}

pub fn hold_unless_held(conn: &Connection, task: &str, birth: &str, permit: Permit) -> rusqlite::Result<()> {
    if authority(conn, task, birth)?.is_none() {
        set_authority(conn, task, birth, &Authority { epoch: 0, permit, to: None })?;
    }
    Ok(())
}

pub fn ops_of(value: &Value) -> Vec<Op> {
    value.as_array().map_or(vec![], |ops| ops.iter().filter_map(Op::from_value).collect())
}

pub fn ops_value(ops: &[Op]) -> Value {
    Value::Array(ops.iter().map(Op::to_value).collect())
}

pub fn authority_value(authority: &Authority) -> Map {
    Map::from_iter([("epoch".to_string(), json!(authority.epoch)), ("permit".to_string(), json!(authority.permit.as_str())), ("to".to_string(), json!(authority.to))])
}

fn op_row(r: &rusqlite::Row) -> rusqlite::Result<Op> {
    let value: String = r.get(6)?;
    Ok(Op { at: Hlc { device: r.get(0)?, ms: r.get(1)?, n: r.get(2)? }, task: r.get(3)?, birth: r.get(4)?, field: r.get(5)?, value: serde_json::from_str(&value).unwrap_or(Value::Null) })
}

fn authority(conn: &Connection, task: &str, birth: &str) -> rusqlite::Result<Option<Authority>> {
    conn.query_row("select epoch, permit, recipient from authority where task = ?1 and birth = ?2", params![task, birth], |r| Ok(Authority { epoch: r.get(0)?, permit: Permit::parse(&r.get::<_, String>(1)?), to: r.get(2)? })).optional()
}

fn set_authority(conn: &Connection, task: &str, birth: &str, held: &Authority) -> rusqlite::Result<()> {
    conn.execute(
        "insert into authority (task, birth, epoch, permit, recipient) values (?1, ?2, ?3, ?4, ?5)
         on conflict (task, birth) do update set epoch = excluded.epoch, permit = excluded.permit, recipient = excluded.recipient",
        params![task, birth, held.epoch, held.permit.as_str(), held.to],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Machine {
        dir: PathBuf,
        db: Db,
        replica: Replica,
    }

    impl Machine {
        fn new(name: &str) -> Machine {
            static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!("kanna-replica-{name}-{}-{}", std::process::id(), COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let db = Db::open(&dir);
            let mut replica = Replica::open(&db).unwrap();
            replica.set_device(name);
            Machine { dir, db, replica }
        }

        fn restart(mut self) -> Machine {
            let me = self.replica.me().to_string();
            self.db = Db::open(&self.dir);
            self.replica = Replica::open(&self.db).unwrap();
            self.replica.set_device(&me);
            self
        }

        fn make(&mut self, task: &str) -> String {
            let birth = self.replica.tick().text();
            let me = self.replica.me().to_string();
            self.replica.write(&self.db, task, &birth, &[("name", json!(task)), ("state", json!("waiting")), ("owner", json!(me))]).unwrap();
            Replica::hold(&self.db, task, &birth, Permit::Ready).unwrap();
            birth
        }

        fn pull(&mut self, from: &Machine) {
            let ops = from.replica.ops_after(&from.db, &self.replica.vector()).unwrap();
            self.replica.apply(&self.db, &ops).unwrap();
        }

        fn ask(&mut self, owner: &mut Machine, task: &str, birth: &str) -> Result<Grant, String> {
            let me = self.replica.me().to_string();
            owner.replica.grant(&owner.db, task, birth, &me, |_| Ok(()))
        }
    }

    impl Drop for Machine {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn sync(a: &mut Machine, b: &mut Machine) {
        a.pull(b);
        b.pull(a);
    }

    #[test]
    fn two_replicas_converge_by_field_and_a_later_write_wins_whoever_made_it() {
        let (mut a, mut b) = (Machine::new("a"), Machine::new("b"));
        let birth = a.make("task-00000001");
        sync(&mut a, &mut b);
        assert_eq!(b.replica.record("task-00000001", &birth).unwrap().text("name"), Some("task-00000001"));
        a.replica.write(&a.db, "task-00000001", &birth, &[("name", json!("from a"))]).unwrap();
        b.replica.write(&b.db, "task-00000001", &birth, &[("after", json!(["task-00000002"]))]).unwrap();
        b.replica.write(&b.db, "task-00000001", &birth, &[("name", json!("from b"))]).unwrap();
        sync(&mut a, &mut b);
        for m in [&a, &b] {
            let record = m.replica.record("task-00000001", &birth).unwrap();
            assert_eq!(record.text("name"), Some("from b"), "the later write wins on {}", m.replica.me());
            assert_eq!(record.list("after"), vec!["task-00000002"], "a different field's write survives on {}", m.replica.me());
        }
        assert_eq!(a.replica.ops_after(&a.db, &b.replica.vector()).unwrap(), vec![], "nothing is left to send");
    }

    #[test]
    fn a_relayed_replica_passes_on_another_machines_ops_and_the_clock_survives_a_restart() {
        let (mut a, mut b, mut c) = (Machine::new("a"), Machine::new("b"), Machine::new("c"));
        let birth = a.make("task-00000001");
        b.pull(&a);
        c.pull(&b);
        assert!(c.replica.record("task-00000001", &birth).is_some(), "c learns a's task through b");
        let before = a.replica.tick();
        let mut a = a.restart();
        assert!(a.replica.tick() > before, "the clock never goes back across a restart");
        a.replica.write(&a.db, "task-00000001", &birth, &[("name", json!("renamed"))]).unwrap();
        c.pull(&a);
        assert_eq!(c.replica.record("task-00000001", &birth).unwrap().text("name"), Some("renamed"));
    }

    #[test]
    fn a_task_is_granted_to_one_of_two_runners_asking_at_once() {
        let (mut owner, mut r1, mut r2) = (Machine::new("owner"), Machine::new("r1"), Machine::new("r2"));
        let birth = owner.make("task-00000001");
        r1.pull(&owner);
        r2.pull(&owner);
        let first = r1.ask(&mut owner, "task-00000001", &birth).unwrap();
        assert_eq!(r2.ask(&mut owner, "task-00000001", &birth), Err("given to r1".into()));
        assert!(r1.replica.receive(&r1.db, &first.task, &first.birth, first.epoch).unwrap());
        assert_eq!(Replica::start(&r1.db, "task-00000001", &birth), Ok(()));
        assert!(Replica::start(&owner.db, "task-00000001", &birth).is_err(), "the owner can't start what it gave away");
        assert!(Replica::start(&r2.db, "task-00000001", &birth).is_err());
        r2.pull(&owner);
        assert_eq!(r2.replica.record("task-00000001", &birth).unwrap().text("owner"), Some("r1"));
    }

    #[test]
    fn a_started_task_is_not_granted_and_a_grant_lost_on_the_way_is_sent_again_and_taken_once() {
        let (mut owner, mut runner) = (Machine::new("owner"), Machine::new("runner"));
        let started = owner.make("task-00000001");
        Replica::start(&owner.db, "task-00000001", &started).unwrap();
        assert_eq!(runner.ask(&mut owner, "task-00000001", &started), Err("started".into()));

        let birth = owner.make("task-00000002");
        runner.ask(&mut owner, "task-00000002", &birth).unwrap();
        let owner = owner.restart();
        let outbox = Replica::outbox(&owner.db).unwrap();
        assert_eq!(outbox.len(), 1, "the committed grant survives the owner's restart");
        let grant = &outbox[0];
        assert!(runner.replica.receive(&runner.db, &grant.task, &grant.birth, grant.epoch).unwrap());
        let mut runner = runner.restart();
        assert!(!runner.replica.receive(&runner.db, &grant.task, &grant.birth, grant.epoch).unwrap(), "a grant delivered twice is taken once");
        Replica::acked(&owner.db, &grant.task, &grant.birth, grant.epoch).unwrap();
        assert_eq!(Replica::outbox(&owner.db).unwrap(), vec![]);
        assert_eq!(Replica::start(&runner.db, "task-00000002", &birth), Ok(()));
    }

    #[test]
    fn an_old_grant_replayed_after_the_task_came_back_gives_nothing() {
        let (mut a, mut b) = (Machine::new("a"), Machine::new("b"));
        let birth = a.make("task-00000001");
        let first = b.ask(&mut a, "task-00000001", &birth).unwrap();
        b.replica.receive(&b.db, &first.task, &first.birth, first.epoch).unwrap();
        let back = a.ask(&mut b, "task-00000001", &birth).unwrap();
        assert!(a.replica.receive(&a.db, &back.task, &back.birth, back.epoch).unwrap());
        assert!(!b.replica.receive(&b.db, &first.task, &first.birth, first.epoch).unwrap(), "b's old grant doesn't make it the owner again");
        assert!(Replica::start(&b.db, "task-00000001", &birth).is_err());
        assert_eq!(Replica::start(&a.db, "task-00000001", &birth), Ok(()));
    }

    #[test]
    fn an_offline_owners_task_waits_and_nobody_else_can_take_it() {
        let (mut owner, mut runner) = (Machine::new("owner"), Machine::new("runner"));
        let birth = owner.make("task-00000001");
        runner.pull(&owner);
        assert!(runner.replica.record("task-00000001", &birth).is_some());
        assert!(Replica::start(&runner.db, "task-00000001", &birth).is_err(), "knowing the record isn't permission to start it");
        let me = runner.replica.me().to_string();
        assert!(runner.replica.grant(&runner.db, "task-00000001", &birth, &me, |_| Ok(())).is_err(), "nor to grant it");
    }

    #[test]
    fn one_id_made_on_two_machines_at_once_stays_two_tasks_and_both_see_it() {
        let (mut a, mut b) = (Machine::new("a"), Machine::new("b"));
        let one = a.make("task-0000beef");
        let other = b.make("task-0000beef");
        sync(&mut a, &mut b);
        for m in [&a, &b] {
            let births: Vec<&str> = m.replica.births("task-0000beef").iter().map(|r| r.birth.as_str()).collect();
            assert_eq!(births.len(), 2, "{} keeps both births apart", m.replica.me());
            assert!(births.contains(&one.as_str()) && births.contains(&other.as_str()));
        }
    }
}
