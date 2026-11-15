use crate::messages::{self, Job};
use crate::reader::Feed;
use crate::reports::Kept;
use crate::Event;
use kanna_detect::program::{self, Program, Version};
use kanna_detect::{agents, Status};
use kanna_vt::{ProgramState, ProgramStatus};
use std::collections::HashMap;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(100);
const IDLE_TICK: Duration = Duration::from_secs(1);
const LOOK_EVERY: Duration = Duration::from_secs(1);

struct Watch {
    serial: u64,
    feed: Arc<Mutex<Feed>>,
    master: OwnedFd,
    program: Option<Program>,
    version: Option<String>,
    version_known: bool,
    looked: Option<Instant>,
    status: Option<Status>,
    group: Option<i32>,
    reported: Option<ProgramState>,
    turns: u64,
    kept: u64,
    parked: Option<Result<Job, String>>,
    moved: Option<String>,
    config: Option<std::path::PathBuf>,
}

struct Shared {
    watches: Mutex<HashMap<String, Watch>>,
    events: crate::runtime::Events,
    stop: AtomicBool,
    wake: (Mutex<bool>, Condvar),
}

pub struct Detector {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Debug)]
pub struct Explained {
    pub agent: Option<String>,
    pub executable: Option<String>,
    pub version: Option<String>,
    pub reported: Option<ProgramStatus>,
    pub records: Vec<ProgramStatus>,
    pub parked: Option<Result<Job, String>>,
}

pub fn from_record(state: ProgramState) -> Status {
    match state {
        ProgramState::Working => Status::Working,
        ProgramState::Blocked => Status::NeedsYou,
        ProgramState::Idle | ProgramState::Done | ProgramState::Error => Status::Idle,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Followed {
    pub status: Option<Status>,
    pub changed: bool,
    pub ended: bool,
    pub moved: Option<String>,
}

pub fn follow(before: Option<Status>, moved: Option<&str>, parked: &Result<Job, String>) -> Followed {
    let status = match parked {
        Ok(job) => job.agent_status(),
        Err(_) => Some(Status::Idle),
    };
    let ended = before == Some(Status::Working) && status.is_some_and(|s| s != Status::Working);
    let moved = parked.as_ref().ok().and_then(|job| job.session.clone()).filter(|session| moved != Some(session.as_str()));
    Followed { status, changed: before != status || ended, ended, moved }
}

pub fn turn_ended(before: Option<ProgramState>, now: ProgramState) -> bool {
    before == Some(ProgramState::Working) && matches!(now, ProgramState::Idle | ProgramState::Done | ProgramState::Blocked)
}

impl Detector {
    pub fn start(events: crate::runtime::Events) -> Detector {
        let shared = Arc::new(Shared { watches: Mutex::default(), events, stop: AtomicBool::new(false), wake: (Mutex::new(false), Condvar::new()) });
        let running = shared.clone();
        let thread = std::thread::Builder::new().name("kanna.status".into()).spawn(move || run(&running)).expect("start agent status");
        Detector { shared, thread: Some(thread) }
    }

    pub fn watch(&self, pane: &str, serial: u64, feed: Arc<Mutex<Feed>>, master: OwnedFd) {
        let (turns, kept) = {
            let feed = lock(&feed);
            (feed.turns(), feed.statuses_serial())
        };
        let watch = Watch { serial, feed, master, program: None, version: None, version_known: false, looked: None, status: None, group: None, reported: None, turns, kept, parked: None, moved: None, config: None };
        lock(&self.shared.watches).insert(pane.into(), watch);
        self.poke();
    }

    pub fn forget(&self, pane: &str, serial: u64) {
        let mut watches = lock(&self.shared.watches);
        if watches.get(pane).is_some_and(|w| w.serial == serial) {
            watches.remove(pane);
            let _ = self.shared.events.send(Event::Unreported { terminal: pane.into() });
        }
    }

    pub fn kept(&self) -> Vec<(String, Option<Kept>)> {
        let watches = lock(&self.shared.watches);
        watches.iter().map(|(pane, watch)| (pane.clone(), Some(kept(&lock(&watch.feed))))).collect()
    }

    pub fn program(&self, pane: &str) -> Option<Program> {
        lock(&self.shared.watches).get(pane)?.program.clone()
    }

    pub fn line_mode(&self, pane: &str) -> bool {
        let watches = lock(&self.shared.watches);
        let Some(watch) = watches.get(pane) else { return false };
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        unsafe { libc::tcgetattr(watch.master.as_raw_fd(), &mut termios) == 0 && termios.c_lflag & libc::ICANON != 0 }
    }

    pub fn argv(&self, pane: &str) -> Vec<Vec<String>> {
        let group = lock(&self.shared.watches).get(pane).and_then(|watch| program::foreground_group(watch.master.as_raw_fd()));
        group.map(program::group_argv).unwrap_or_default()
    }

    pub fn explain(&self, pane: &str) -> Option<Explained> {
        let watches = lock(&self.shared.watches);
        let watch = watches.get(pane)?;
        let feed = lock(&watch.feed);
        let records = feed.program_statuses();
        let reported = watch.program.as_ref().and(feed.reported(program::foreground_group(watch.master.as_raw_fd())));
        drop(feed);
        Some(Explained {
            agent: watch.program.as_ref().map(|p| p.agent.clone()),
            executable: watch.program.as_ref().and_then(|p| p.executable.as_ref()).map(|p| p.display().to_string()),
            version: watch.version.clone(),
            reported,
            records,
            parked: watch.parked.clone(),
        })
    }

    fn poke(&self) {
        let (flag, condvar) = &self.shared.wake;
        *lock(flag) = true;
        condvar.notify_all();
    }
}

impl Drop for Detector {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.poke();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn kept(feed: &Feed) -> Kept {
    let (owner, records) = feed.kept_statuses();
    Kept { owner, records }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn run(shared: &Shared) {
    while !shared.stop.load(Ordering::SeqCst) {
        let agents = tick(shared, Instant::now());
        let (flag, condvar) = &shared.wake;
        let guard = lock(flag);
        let (mut poked, _) = condvar.wait_timeout_while(guard, if agents { TICK } else { IDLE_TICK }, |p| !*p).unwrap_or_else(|e| e.into_inner());
        *poked = false;
    }
}

fn tick(shared: &Shared, now: Instant) -> bool {
    let mut watches = lock(&shared.watches);
    let mut any = false;
    for (pane, watch) in watches.iter_mut() {
        let serial = watch.serial;
        let send = |status: Option<Status>, ended: bool| {
            let _ = shared.events.send(Event::Status { terminal: pane.clone(), serial, status: status.map(Status::as_str), ended });
        };
        let group = program::foreground_group(watch.master.as_raw_fd());
        if group != watch.group {
            if watch.group.is_some() {
                lock(&watch.feed).foreground_changed(group);
            }
            watch.group = group;
        }
        {
            let feed = lock(&watch.feed);
            let now = feed.statuses_serial();
            if now != watch.kept {
                watch.kept = now;
                let _ = shared.events.send(Event::Reported { terminal: pane.clone(), serial, kept: kept(&feed) });
            }
        }
        if watch.looked.is_none_or(|t| now.duration_since(t) >= LOOK_EVERY) {
            watch.looked = Some(now);
            let found = group.and_then(program::identify);
            let changed = found.as_ref().map(|p| (&p.agent, &p.executable)) != watch.program.as_ref().map(|p| (&p.agent, &p.executable));
            if changed {
                watch.program = found;
                watch.version = None;
                watch.version_known = false;
                watch.reported = None;
                watch.parked = None;
                watch.moved = None;
                watch.config = watch.program.as_ref().filter(|p| p.agent == "claude").map(|p| messages::claude_config(p.pid));
                if watch.status.take().is_some() {
                    send(None, false);
                }
            }
            if let Some((program, config)) = watch.program.as_ref().zip(watch.config.as_ref()) {
                let parked = messages::parked_job(config, program.pid);
                if parked.is_none() && watch.parked.is_some() {
                    watch.reported = None;
                }
                watch.parked = parked;
            }
        } else if let Some(Ok(job)) = &watch.parked {
            watch.parked = Some(job.reread().ok_or_else(|| job.id.clone()));
        }
        let Some(program) = watch.program.clone() else { continue };
        any = true;
        if !watch.version_known {
            if let Version::Known(version) = program::version(&program) {
                watch.version_known = true;
                if let Some(why) = version.as_deref().and_then(|v| agents::too_old(&program.agent, v)) {
                    let _ = shared.events.send(Event::TooOld { terminal: pane.clone(), serial, why });
                }
                watch.version = version;
            }
        }
        if let Some(parked) = &watch.parked {
            let followed = follow(watch.status, watch.moved.as_deref(), parked);
            if let Some(session) = followed.moved {
                watch.moved = Some(session.clone());
                let _ = shared.events.send(Event::Moved { terminal: pane.clone(), serial, session });
            }
            if followed.changed {
                watch.status = followed.status;
                send(followed.status, followed.ended);
            }
            watch.turns = lock(&watch.feed).turns();
            continue;
        }
        let feed = lock(&watch.feed);
        let record = feed.reported(group);
        let turns = feed.turns();
        drop(feed);
        let Some(record) = record else {
            watch.reported = None;
            if watch.status.take().is_some() {
                send(None, false);
            }
            continue;
        };
        let missed = turns > watch.turns && record.state != ProgramState::Working;
        watch.turns = turns;
        let ended = turn_ended(watch.reported.replace(record.state), record.state) || missed;
        let status = from_record(record.state);
        if watch.status.replace(status) != Some(status) || ended {
            send(Some(status), ended);
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_maps_to_a_status() {
        assert_eq!(from_record(ProgramState::Working), Status::Working);
        assert_eq!(from_record(ProgramState::Blocked), Status::NeedsYou);
        for state in [ProgramState::Idle, ProgramState::Done, ProgramState::Error] {
            assert_eq!(from_record(state), Status::Idle);
        }
    }

    #[test]
    fn a_parked_claude_takes_its_status_and_session_from_the_job_running_its_conversation() {
        let job = |status: &str| Ok(Job { id: "c71180f2".into(), pid: 7, entry: "/c/sessions/7.json".into(), session: Some("c71180f2-ffd5".into()), status: Some(status.into()), waiting_for: None, protocol: Some(1), socket: None });
        let busy = follow(None, None, &job("busy"));
        assert_eq!(busy, Followed { status: Some(Status::Working), changed: true, ended: false, moved: Some("c71180f2-ffd5".into()) }, "the job's session replaces the old one once");
        assert_eq!(follow(Some(Status::Working), Some("c71180f2-ffd5"), &job("busy")), Followed { status: Some(Status::Working), changed: false, ended: false, moved: None });
        assert_eq!(follow(Some(Status::Working), Some("c71180f2-ffd5"), &job("idle")), Followed { status: Some(Status::Idle), changed: true, ended: true, moved: None }, "busy to idle ends the turn");
        assert_eq!(follow(Some(Status::Working), Some("c71180f2-ffd5"), &job("waiting")), Followed { status: Some(Status::NeedsYou), changed: true, ended: true, moved: None }, "waiting is needs-you, and ends the turn");
        assert_eq!(follow(Some(Status::Idle), Some("c71180f2-ffd5"), &job("idle")).changed, false);
        assert_eq!(follow(Some(Status::Idle), Some("c71180f2-ffd5"), &job("other")).status, None, "a status Kanna doesn't know is none");
        assert_eq!(follow(Some(Status::Working), Some("c71180f2-ffd5"), &Err("c71180f2".into())), Followed { status: Some(Status::Idle), changed: true, ended: true, moved: None }, "with its job gone, the parked claude sits in its agent view");
    }

    #[test]
    fn working_to_idle_done_or_blocked_ends_the_turn() {
        use ProgramState::*;
        assert!(turn_ended(Some(Working), Idle));
        assert!(turn_ended(Some(Working), Done));
        assert!(turn_ended(Some(Working), Blocked));
        assert!(!turn_ended(Some(Working), Error));
        assert!(!turn_ended(Some(Idle), Idle));
        assert!(!turn_ended(None, Done));
    }
}
