use crate::reader::Feed;
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
}

pub fn from_record(state: ProgramState) -> Status {
    match state {
        ProgramState::Working => Status::Working,
        ProgramState::Blocked => Status::NeedsYou,
        ProgramState::Idle | ProgramState::Done | ProgramState::Error => Status::Idle,
    }
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
        let turns = lock(&feed).turns();
        let watch = Watch { serial, feed, master, program: None, version: None, version_known: false, looked: None, status: None, group: None, reported: None, turns };
        lock(&self.shared.watches).insert(pane.into(), watch);
        self.poke();
    }

    pub fn forget(&self, pane: &str, serial: u64) {
        let mut watches = lock(&self.shared.watches);
        if watches.get(pane).is_some_and(|w| w.serial == serial) {
            watches.remove(pane);
        }
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
        if watch.looked.is_none_or(|t| now.duration_since(t) >= LOOK_EVERY) {
            watch.looked = Some(now);
            let found = group.and_then(program::identify);
            let changed = found.as_ref().map(|p| (&p.agent, &p.executable)) != watch.program.as_ref().map(|p| (&p.agent, &p.executable));
            if changed {
                watch.program = found;
                watch.version = None;
                watch.version_known = false;
                watch.reported = None;
                if watch.status.take().is_some() {
                    send(None, false);
                }
            }
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
