use crate::app_link::{Outbox, QUEUE_BEHIND};
use crate::perf;
use crate::reader::{self, Feed, Resumer, Typing};
use crate::Event;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use kanna_mux::json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

pub const CHUNK: usize = 64 << 10;

pub const FLOW_PROTOCOL: u64 = 3;

struct Link {
    conn: u64,
    out: Outbox,
    flow: bool,
}

struct Line {
    serial: u64,
    resumer: Resumer,
    input: Sender<Vec<u8>>,
    typing: Typing,
    feed: Arc<Mutex<Feed>>,
}

#[derive(Default)]
struct State {
    link: Option<Link>,
    lines: HashMap<String, Line>,
}

pub struct Wire {
    state: Mutex<State>,
    events: Mutex<Sender<Event>>,
}

impl Wire {
    pub fn new(events: Sender<Event>) -> Wire {
        Wire { state: Mutex::default(), events: Mutex::new(events) }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn connect(&self, conn: u64, out: Outbox, flow: bool) {
        self.state().link = Some(Link { conn, out, flow });
    }

    pub fn disconnect(&self) {
        self.state().link = None;
    }

    pub fn add(&self, id: &str, serial: u64, resumer: Resumer, input: Sender<Vec<u8>>, typing: Typing, feed: Arc<Mutex<Feed>>) {
        self.state().lines.insert(id.to_string(), Line { serial, resumer, input, typing, feed });
    }

    pub fn remove(&self, id: &str) {
        self.state().lines.remove(id);
    }

    pub fn input(&self, conn: u64, id: &str, bytes: Vec<u8>, at: Instant) {
        let state = self.state();
        if state.link.as_ref().map(|link| link.conn) != Some(conn) {
            return;
        }
        let Some(line) = state.lines.get(id) else { return };
        perf::since("input.handled", at);
        reader::typed(&line.typing, at);
        let _ = line.input.send(bytes);
    }

    pub fn taken(&self, conn: u64, id: &str) {
        let state = self.state();
        if state.link.as_ref().map(|link| link.conn) != Some(conn) {
            return;
        }
        let Some(line) = state.lines.get(id) else { return };
        let (feed, serial) = (line.feed.clone(), line.serial);
        drop(state);
        let mut feed = Feed::lock(&feed);
        if !std::mem::replace(&mut feed.inflight, false) {
            return;
        }
        if feed.overflowed() {
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
        if !feed.shown {
            feed.discard();
            return;
        }
        if feed.overflowed() {
            return;
        }
        let state = self.state();
        let Some(link) = state.link.as_ref() else { return };
        if link.out.behind() {
            feed.discard();
            return;
        }
        if link.flow && feed.inflight {
            return;
        }
        let Some((bytes, typed)) = feed.chunk(if link.flow { CHUNK } else { usize::MAX }) else { return };
        let queued = link.out.push_typed(&output(id, &bytes), typed);
        feed.inflight = link.flow;
        if queued > QUEUE_BEHIND && link.out.fell_behind() {
            let _ = self.events.lock().unwrap_or_else(PoisonError::into_inner).send(Event::AppBehind { conn: link.conn, queued });
        }
    }

    pub fn shown(&self, id: &str, feed: &mut Feed, messages: &[Value]) {
        let state = self.state();
        let Some(link) = state.link.as_ref() else { return };
        for message in messages {
            link.out.push(message);
        }
        feed.shown = true;
        feed.inflight = false;
        drop(state);
        self.deliver(id, feed);
    }

    pub fn after_output(&self, id: &str, feed: &mut Feed, message: &Value) {
        let state = self.state();
        let Some(link) = state.link.as_ref() else { return };
        if feed.shown && !feed.overflowed() && !link.out.behind() {
            while let Some((bytes, typed)) = feed.chunk(usize::MAX) {
                link.out.push_typed(&output(id, &bytes), typed);
            }
        }
        link.out.push(message);
    }
}

fn output(id: &str, bytes: &[u8]) -> Value {
    json!({ "cmd": "output", "args": { "terminal": id, "data": BASE64.encode(bytes) } })
}
