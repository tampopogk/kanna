use crate::Event;
use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

pub type Events = tokio::sync::mpsc::UnboundedSender<Event>;
pub type Inbox = tokio::sync::mpsc::UnboundedReceiver<Event>;

pub const SLOW: Duration = Duration::from_millis(20);

pub fn channel() -> (Events, Inbox) {
    tokio::sync::mpsc::unbounded_channel()
}

static HELD: AtomicU64 = AtomicU64::new(0);
static OVER: AtomicU64 = AtomicU64::new(0);

pub fn holds() -> (u64, u64) {
    (HELD.load(Ordering::Relaxed), OVER.load(Ordering::Relaxed))
}

pub fn reset_holds() {
    HELD.store(0, Ordering::Relaxed);
    OVER.store(0, Ordering::Relaxed);
}

pub fn check(what: impl Display, took: Duration) {
    HELD.fetch_add(1, Ordering::Relaxed);
    if took > SLOW {
        OVER.fetch_add(1, Ordering::Relaxed);
        log!("kanna-daemon: slow: {what} held the loop {}ms", took.as_millis());
    }
}

#[cfg(test)]
pub fn build() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

pub fn timed(name: &str, took: Duration) {
    crate::perf::record("loop.async", took);
    check(name, took);
}

#[cfg(not(test))]
pub fn spawn(name: &'static str, future: impl Future<Output = ()> + 'static) {
    tokio::task::spawn_local(Timed { name, future: Box::pin(future) });
}

#[cfg(test)]
pub fn spawn(name: &'static str, future: impl Future<Output = ()> + 'static) {
    let timed = Timed { name, future: Box::pin(future) };
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_local(timed);
        return;
    }
    let runtime = build();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(timed);
    runtime.block_on(local);
}

pub async fn off_loop<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok()
}

struct Timed<F> {
    name: &'static str,
    future: Pin<Box<F>>,
}

impl<F: Future<Output = ()>> Future for Timed<F> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let started = Instant::now();
        let polled = self.future.as_mut().poll(cx);
        let took = started.elapsed();
        timed(self.name, took);
        polled
    }
}

#[cfg(test)]
pub fn next(inbox: &mut Inbox, within: Duration) -> Result<Event, ()> {
    let by = Instant::now() + within;
    loop {
        match inbox.try_recv() {
            Ok(event) => return Ok(event),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return Err(()),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) if Instant::now() >= by => return Err(()),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}
