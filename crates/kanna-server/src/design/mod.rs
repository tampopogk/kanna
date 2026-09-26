//! App Design: design-first workflows (docs/specs/app-design.md).
//!
//! A design task keeps one live session and workspace from its first design
//! position to the hand-off (§3). This module owns what that session works
//! on beside its terminal:
//!
//! - [`document`]: the live document's schema-exact Yrs adapter.
//! - [`live`]: the documents in memory, durable before acknowledged.
//! - [`service`]: sessions, positions, threads and agent edits.
//! - [`delivery`]: feedback queued to the live session as it becomes free.
//! - [`approval`]: Approve for build and the hand-off to the factory.

pub(crate) mod approval;
pub(crate) mod delivery;
pub(crate) mod document;
pub(crate) mod export;
pub(crate) mod live;
pub(crate) mod service;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{watch, Notify};

/// Process-wide design state: live documents, the feed's change signals and
/// the delivery worker's wake-up.
#[derive(Clone, Default)]
pub(crate) struct DesignRuntime {
    pub(crate) documents: live::LiveDocuments,
    feeds: Arc<Mutex<HashMap<String, watch::Sender<u64>>>>,
    delivery_wake: Arc<Notify>,
    pub(crate) delivery: Arc<Mutex<delivery::DeliveryMemory>>,
}

impl DesignRuntime {
    /// A counter that moves whenever a task's threads, deliveries, position
    /// or approval change. Not durable: a client treats any change (including
    /// a restart's reset) as "fetch again".
    pub(crate) fn feed_revision(&self, task_id: &str) -> u64 {
        self.feeds
            .lock()
            .unwrap()
            .get(task_id)
            .map(|sender| *sender.borrow())
            .unwrap_or(0)
    }

    pub(crate) fn feed_changed(&self, task_id: &str) {
        let mut feeds = self.feeds.lock().unwrap();
        let sender = feeds
            .entry(task_id.to_string())
            .or_insert_with(|| watch::channel(0).0);
        sender.send_modify(|revision| *revision += 1);
    }

    pub(crate) fn subscribe_feed(&self, task_id: &str) -> watch::Receiver<u64> {
        self.feeds
            .lock()
            .unwrap()
            .entry(task_id.to_string())
            .or_insert_with(|| watch::channel(0).0)
            .subscribe()
    }

    pub(crate) fn wake_delivery(&self) {
        self.delivery_wake.notify_one();
    }

    pub(crate) async fn delivery_woken(&self) {
        self.delivery_wake.notified().await
    }
}
