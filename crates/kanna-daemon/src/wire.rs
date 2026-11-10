use crate::app_link::{Outbox, QUEUE_BEHIND};
use crate::perf;
use crate::reader::{self, Feed, Resumer, Typing};
use crate::Event;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use kanna_mux::json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

pub const CHUNK: usize = 64 << 10;

struct Link {
    out: Outbox,
}

struct Line {
    serial: u64,
    resumer: Resumer,
    input: Sender<Vec<u8>>,
    typing: Typing,
    feed: Arc<Mutex<Feed>>,
    user: u64,
}

#[derive(Default)]
struct State {
    links: HashMap<u64, Link>,
    lines: HashMap<String, Line>,
    heeded: HashSet<String>,
    last_user: u64,
}

pub struct Wire {
    state: Mutex<State>,
    events: Mutex<crate::runtime::Events>,
}

impl Wire {
    pub fn new(events: crate::runtime::Events) -> Wire {
        Wire { state: Mutex::default(), events: Mutex::new(events) }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn connect(&self, conn: u64, out: Outbox) {
        self.state().links.insert(conn, Link { out });
    }

    pub fn disconnect(&self, conn: u64) {
        self.state().links.remove(&conn);
    }

    pub fn disconnect_all(&self) {
        self.state().links.clear();
    }

    pub fn add(&self, id: &str, serial: u64, resumer: Resumer, input: Sender<Vec<u8>>, typing: Typing, feed: Arc<Mutex<Feed>>) {
        self.state().lines.insert(id.to_string(), Line { serial, resumer, input, typing, feed, user: 0 });
    }

    pub fn remove(&self, id: &str) {
        let mut state = self.state();
        state.heeded.remove(id);
        state.lines.remove(id);
    }

    pub fn input(&self, conn: u64, id: &str, bytes: Vec<u8>, at: Instant) {
        let mut state = self.state();
        if !state.links.contains_key(&conn) {
            return;
        }
        let Some(line) = state.lines.get(id) else { return };
        perf::since("input.handled", at);
        reader::typed(&line.typing, conn, at);
        let report = is_focus_report(&bytes);
        let _ = line.input.send(bytes);
        if !report {
            if state.heeded.remove(id) {
                let _ = self.events.lock().unwrap_or_else(PoisonError::into_inner).send(Event::Typed { terminal: id.to_string() });
            }
            self.note_user(&mut state, conn, id);
        }
    }

    pub fn heed(&self, id: &str) {
        self.state().heeded.insert(id.to_string());
    }

    pub fn used(&self, conn: u64, id: &str) {
        let mut state = self.state();
        if state.links.contains_key(&conn) {
            self.note_user(&mut state, conn, id);
        }
    }

    pub fn left(&self, conn: u64, id: &str) {
        if let Some(line) = self.state().lines.get_mut(id).filter(|line| line.user == conn) {
            line.user = 0;
        }
    }

    fn note_user(&self, state: &mut State, conn: u64, id: &str) {
        let Some(line) = state.lines.get_mut(id) else { return };
        let same = line.user == conn && state.last_user == conn;
        line.user = conn;
        state.last_user = conn;
        if !same {
            let _ = self.events.lock().unwrap_or_else(PoisonError::into_inner).send(Event::AppUsed { conn, terminal: id.to_string() });
        }
    }

    pub fn taken(&self, conn: u64, id: &str) {
        let state = self.state();
        if !state.links.contains_key(&conn) {
            return;
        }
        let Some(line) = state.lines.get(id) else { return };
        let (feed, serial) = (line.feed.clone(), line.serial);
        drop(state);
        let mut feed = Feed::lock(&feed);
        let Some(viewer) = feed.viewers.get_mut(&conn) else { return };
        if !std::mem::replace(&mut viewer.inflight, false) {
            return;
        }
        if viewer.overflowed {
            let _ = self.events.lock().unwrap_or_else(PoisonError::into_inner).send(Event::Output { terminal: id.to_string(), serial });
        } else {
            self.deliver(id, &mut feed);
        }
        drop(feed);
        if let Some(line) = self.state().lines.get(id) {
            line.resumer.resume();
        }
    }

    pub fn deliver(&self, id: &str, feed: &mut Feed) {
        let state = self.state();
        let written = feed.written();
        for (conn, viewer) in feed.viewers.iter_mut() {
            if viewer.overflowed {
                continue;
            }
            let Some(link) = state.links.get(conn).filter(|link| !link.out.behind()) else {
                viewer.discard();
                continue;
            };
            if viewer.inflight {
                continue;
            }
            let Some(chunk) = viewer.chunk(CHUNK, written) else { continue };
            let queued = link.out.push_for(Some(id), &output(id, &chunk.bytes, chunk.offset), chunk.typed);
            viewer.inflight = true;
            if queued > QUEUE_BEHIND && link.out.fell_behind() {
                let _ = self.events.lock().unwrap_or_else(PoisonError::into_inner).send(Event::AppBehind { conn: *conn, queued });
            }
        }
    }

    pub fn shown(&self, conn: u64, id: &str, feed: &mut Feed, messages: &[Value]) {
        let state = self.state();
        let Some(link) = state.links.get(&conn) else { return };
        for message in messages {
            link.out.push_for(Some(id), message, None);
        }
        feed.show(conn);
        drop(state);
        self.deliver(id, feed);
    }

    pub fn after_output(&self, conn: u64, id: &str, feed: &mut Feed, message: &Value) {
        let state = self.state();
        let Some(link) = state.links.get(&conn) else { return };
        let written = feed.written();
        if let Some(viewer) = feed.viewers.get_mut(&conn).filter(|viewer| !viewer.overflowed && !link.out.behind()) {
            while let Some(chunk) = viewer.chunk(usize::MAX, written) {
                link.out.push_for(Some(id), &output(id, &chunk.bytes, chunk.offset), chunk.typed);
            }
        }
        link.out.push_for(Some(id), message, None);
    }
}

fn is_focus_report(bytes: &[u8]) -> bool {
    bytes == b"\x1b[I" || bytes == b"\x1b[O"
}

pub fn output(id: &str, bytes: &[u8], offset: u64) -> Value {
    json!({ "cmd": "output", "args": { "terminal": id, "data": BASE64.encode(bytes), "offset": offset } })
}
