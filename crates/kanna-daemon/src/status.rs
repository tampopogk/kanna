use crate::reader::Feed;
use crate::Event;
use kanna_detect::program::{self, Program, Version};
use kanna_detect::{Screen, Status, Store, Tracker, Verdict};
use kanna_vt::{ProgramState, ProgramStatus};
use std::collections::HashMap;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
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
    generation: u64,
    looked: Option<Instant>,
    tracker: Tracker,
    fresh: bool,
    group: Option<i32>,
    reported: Option<ProgramState>,
}

struct Shared {
    watches: Mutex<HashMap<String, Watch>>,
    store: Store,
    events: Sender<Event>,
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
    pub verdict: Option<Verdict>,
    pub screen: Screen,
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
    before == Some(ProgramState::Working) && matches!(now, ProgramState::Idle | ProgramState::Done)
}

impl Detector {
    pub fn start(store: Store, events: Sender<Event>) -> Detector {
        let shared = Arc::new(Shared { watches: Mutex::default(), store, events, stop: AtomicBool::new(false), wake: (Mutex::new(false), Condvar::new()) });
        let running = shared.clone();
        let thread = std::thread::Builder::new().name("kanna.status".into()).spawn(move || run(&running)).expect("start agent status");
        Detector { shared, thread: Some(thread) }
    }

    pub fn store(&self) -> &Store {
        &self.shared.store
    }

    pub fn watch(&self, pane: &str, serial: u64, feed: Arc<Mutex<Feed>>, master: OwnedFd) {
        let watch = Watch { serial, feed, master, program: None, version: None, version_known: false, generation: 0, looked: None, tracker: Tracker::default(), fresh: true, group: None, reported: None };
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

    pub fn argv(&self, pane: &str) -> Vec<Vec<String>> {
        let group = lock(&self.shared.watches).get(pane).and_then(|watch| program::foreground_group(watch.master.as_raw_fd()));
        group.map(program::group_argv).unwrap_or_default()
    }

    pub fn explain(&self, pane: &str) -> Option<Explained> {
        let watches = lock(&self.shared.watches);
        let watch = watches.get(pane)?;
        let feed = lock(&watch.feed);
        let screen = feed.status_screen_peek();
        let records = feed.program_statuses();
        let reported = watch.program.as_ref().and(feed.reported(program::foreground_group(watch.master.as_raw_fd())));
        drop(feed);
        let rules = self.shared.store.rules();
        let agent = watch.program.as_ref().map(|p| p.agent.clone());
        let verdict = agent.as_deref().and_then(|a| rules.evaluate(a, &screen, watch.version.as_deref()));
        Some(Explained {
            agent,
            executable: watch.program.as_ref().and_then(|p| p.executable.as_ref()).map(|p| p.display().to_string()),
            verdict,
            screen,
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
        let now = Instant::now();
        let (agents, due) = tick(shared, now);
        let pause = due.map_or(TICK, |t| t.saturating_duration_since(now).min(TICK));
        let (flag, condvar) = &shared.wake;
        let guard = lock(flag);
        let (mut poked, _) = condvar.wait_timeout_while(guard, if agents { pause } else { IDLE_TICK }, |p| !*p).unwrap_or_else(|e| e.into_inner());
        *poked = false;
    }
}

fn tick(shared: &Shared, now: Instant) -> (bool, Option<Instant>) {
    let rules = shared.store.rules();
    let generation = shared.store.generation();
    let mut watches = lock(&shared.watches);
    let mut any = false;
    let mut due: Option<Instant> = None;
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
            let found = program::foreground_group(watch.master.as_raw_fd()).and_then(|group| program::identify(group, &rules));
            let changed = found.as_ref().map(|p| (&p.agent, &p.executable)) != watch.program.as_ref().map(|p| (&p.agent, &p.executable));
            if changed {
                let had = watch.tracker.status().is_some();
                watch.program = found;
                watch.version = None;
                watch.version_known = false;
                watch.tracker = Tracker::default();
                watch.fresh = true;
                watch.reported = None;
                if had && watch.program.is_none() {
                    send(None, false);
                }
            }
        }
        let Some(program) = watch.program.clone() else { continue };
        let Some(agent) = rules.agent(&program.agent) else { continue };
        any = true;
        if !watch.version_known {
            if let Version::Known(version) = program::version(&program, agent) {
                watch.version = version;
                watch.version_known = true;
                watch.fresh = true;
            }
        }
        if watch.generation != generation {
            watch.generation = generation;
            watch.fresh = true;
        }
        let mut feed = lock(&watch.feed);
        if let Some(record) = feed.reported(group) {
            feed.settle();
            drop(feed);
            watch.fresh = false;
            let ended = turn_ended(watch.reported.replace(record.state), record.state);
            let changed = watch.tracker.set(from_record(record.state));
            if changed.is_some() || ended {
                send(watch.tracker.status(), ended);
            }
            continue;
        }
        if watch.reported.take().is_some() {
            watch.fresh = true;
        }
        let activity = feed.activity();
        let recheck = watch.tracker.recheck();
        let pending = activity.dirty_since.is_some() || watch.fresh || recheck.is_some_and(|t| now >= t);
        if !pending || !activity.settled(now) {
            let next = if pending { activity.wake(now) } else { recheck };
            due = due.into_iter().chain(next).min();
            continue;
        }
        let screen = feed.status_screen();
        drop(feed);
        watch.fresh = false;
        let verdict = agent.evaluate(&screen, watch.version.as_deref());
        if let Some(status) = watch.tracker.judge(verdict.status, now) {
            send(Some(status), false);
        }
        due = due.into_iter().chain(watch.tracker.recheck()).min();
    }
    (any, due)
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
    fn working_to_idle_or_done_ends_the_turn() {
        use ProgramState::*;
        assert!(turn_ended(Some(Working), Idle));
        assert!(turn_ended(Some(Working), Done));
        assert!(!turn_ended(Some(Working), Error));
        assert!(!turn_ended(Some(Working), Blocked));
        assert!(!turn_ended(Some(Idle), Idle));
        assert!(!turn_ended(None, Done));
    }
}
