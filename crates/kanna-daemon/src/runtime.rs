use crate::Event;
use std::fmt::Display;
use std::time::Duration;

pub type Events = tokio::sync::mpsc::UnboundedSender<Event>;
pub type Inbox = tokio::sync::mpsc::UnboundedReceiver<Event>;

pub const SLOW: Duration = Duration::from_millis(20);

pub fn channel() -> (Events, Inbox) {
    tokio::sync::mpsc::unbounded_channel()
}

pub fn build() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread().enable_all().thread_name("kanna.loop").build().map_err(|e| format!("can't start the daemon's runtime: {e}"))
}

pub fn check(what: impl Display, took: Duration) {
    if took > SLOW {
        log!("kanna-daemon: slow: {what} held the loop {}ms", took.as_millis());
    }
}

#[cfg(test)]
pub fn next(inbox: &mut Inbox, within: Duration) -> Result<Event, ()> {
    let by = std::time::Instant::now() + within;
    loop {
        match inbox.try_recv() {
            Ok(event) => return Ok(event),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return Err(()),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) if std::time::Instant::now() >= by => return Err(()),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}
