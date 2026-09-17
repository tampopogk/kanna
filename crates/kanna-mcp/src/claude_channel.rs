//! Host side of the Claude native-channel wake transport.
//!
//! This process is already the task's MCP server, so it already owns a stdio
//! pipe into the running CLI. A supervisory wake therefore never has to become
//! terminal input: `kanna-server` hands this host a frame, and this host turns
//! it into a `notifications/claude/channel` on that pipe. Nothing is typed, so
//! an unsent human draft and its cursor are untouched — which is the whole
//! point, and what the 2026-09-14 live experiment measured.
//!
//! What this host may claim is exactly one thing: that a notification was
//! written to the MCP transport. It cannot see whether the model read it — the
//! experiment recorded a notice absorbed into a running turn with no mailbox
//! read — so the receipt says `written`, never `read` and never `acknowledged`.
//! A failed write says `uncertain` and nothing else happens: there is no
//! composer fallback here, by design.
use serde_json::{json, Value};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Opt-in, per the experiment's scoped decision. Absent or unset means this
/// host never attaches and the transport does not exist for that session.
const ENABLE_ENV: &str = "KANNA_CLAUDE_CHANNELS";

pub(crate) struct ChannelHost {
    base_url: String,
    task_id: String,
    run_id: String,
}

/// The capability advertised at `initialize`, which is what lets the CLI route
/// `notifications/claude/channel` at all.
pub(crate) fn experimental_capabilities() -> Option<Value> {
    enabled().map(|_| json!({"claude/channel": {}}))
}

fn enabled() -> Option<(String, String)> {
    if std::env::var(ENABLE_ENV).ok().as_deref() != Some("1") {
        return None;
    }
    let task_id = std::env::var("KANNA_TASK_ID")
        .ok()
        .filter(|v| !v.is_empty())?;
    let run_id = std::env::var(kanna_tool_catalog::KANNA_STAGE_RUN_ID_ENV)
        .ok()
        .filter(|v| !v.is_empty())?;
    Some((task_id, run_id))
}

/// Attach this session's channel host, if it is enabled for this run.
///
/// The stream is held for the life of the process and re-established when it
/// drops. A reconnect is not a retry of anything: the server mints a fresh,
/// unconfirmed channel and re-probes, and every pending batch is still in the
/// durable mailbox where it always was.
pub(crate) fn spawn<W>(base_url: &str, stdout: Arc<Mutex<W>>)
where
    W: Write + Send + 'static,
{
    let Some((task_id, run_id)) = enabled() else {
        return;
    };
    let host = ChannelHost {
        base_url: base_url.to_string(),
        task_id,
        run_id,
    };
    tokio::spawn(async move {
        let mut backoff = Duration::from_millis(250);
        loop {
            match host.run(&stdout).await {
                Ok(()) => backoff = Duration::from_millis(250),
                Err(error) => {
                    eprintln!("Warning: Kanna channel host disconnected: {error}");
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(15));
        }
    });
}

impl ChannelHost {
    async fn run<W: Write>(&self, stdout: &Arc<Mutex<W>>) -> Result<(), String> {
        let url = format!(
            "{}/v1/tasks/{}/claude-channel?runId={}",
            self.base_url.trim_end_matches('/'),
            urlencode(&self.task_id),
            urlencode(&self.run_id),
        );
        let response = super::http_client()
            .get(&url)
            .header("accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| format!("channel stream request failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("channel stream refused: {}", response.status()));
        }
        let mut response = response;
        let mut buffer = String::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("channel stream read failed: {e}"))?
        {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            // SSE frames are separated by a blank line; this server sends one
            // `data:` line per frame.
            while let Some(split) = buffer.find("\n\n") {
                let frame = buffer[..split].to_string();
                buffer.drain(..split + 2);
                for line in frame.lines() {
                    let Some(payload) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let Ok(value) = serde_json::from_str::<Value>(payload.trim()) else {
                        continue;
                    };
                    self.handle(&value, stdout).await;
                }
            }
        }
        Ok(())
    }

    async fn handle<W: Write>(&self, frame: &Value, stdout: &Arc<Mutex<W>>) {
        match frame["type"].as_str() {
            Some("probe") => {
                let Some(channel_id) = frame["channelId"].as_str() else {
                    return;
                };
                // Deliberately best-effort and unrecorded: a probe announces a
                // transport, not an event. If it is lost the server re-probes
                // from the next admitted wake, which is the whole fix for the
                // startup race the experiment found.
                let _ = self.notify(stdout, &probe_notice(&self.task_id, channel_id));
            }
            Some("wake") => {
                let attempt = &frame["attempt"];
                let (Some(attempt_id), Some(channel_id), Some(message)) = (
                    attempt["id"].as_str(),
                    attempt["binding"]["channelId"].as_str(),
                    attempt["message"].as_str(),
                ) else {
                    return;
                };
                let (kind, error) = match self.notify(stdout, message) {
                    Ok(()) => ("written", None),
                    Err(error) => ("uncertain", Some(error)),
                };
                self.receipt(attempt_id, channel_id, kind, error).await;
            }
            _ => {}
        }
    }

    /// Write one channel notification onto the MCP stdio this process owns.
    ///
    /// `params.prompt` is the shape the live experiment recorded arriving in
    /// the transcript as a `channel`-origin attachment; it is the only part of
    /// this module taken from observation rather than from Kanna's own
    /// contract, and it is the one thing a CLI upgrade could move.
    fn notify<W: Write>(&self, stdout: &Arc<Mutex<W>>, prompt: &str) -> Result<(), String> {
        let mut line = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/claude/channel",
            "params": {"prompt": prompt},
        }))
        .map_err(|e| format!("failed to render channel notification: {e}"))?;
        line.push('\n');
        super::write_line(stdout, &line)
    }

    async fn receipt(&self, attempt_id: &str, channel_id: &str, kind: &str, error: Option<String>) {
        let url = format!(
            "{}/v1/tasks/{}/claude-channel/receipt",
            self.base_url.trim_end_matches('/'),
            urlencode(&self.task_id),
        );
        let body = json!({
            "runId": self.run_id,
            "channelId": channel_id,
            "attemptId": attempt_id,
            "kind": kind,
            "error": error,
        });
        if let Err(error) = super::http_client().post(&url).json(&body).send().await {
            // The batch stays pending either way; a lost receipt round trip is
            // never resolved by sending the notice again from here.
            eprintln!("Warning: Kanna channel receipt failed: {error}");
        }
    }
}

fn urlencode(value: &str) -> String {
    kanna_tool_catalog::encode_path_segment(value)
}

/// What a probe asks of the model. It names the tool that answers it and says
/// plainly that it is transport setup, so it cannot read as work.
pub(crate) fn probe_notice(task_id: &str, channel_id: &str) -> String {
    format!(
        "<channel source=\"kanna-mcp\" kind=\"probe\" channel_id=\"{channel_id}\">\nKanna is \
         checking that this session can receive supervisory event notices. Call \
         kanna_confirm_event_channel with task_id \"{task_id}\" and channel_id \
         \"{channel_id}\" now. This is transport setup, not a task, not owner speech, and \
         not anything to acknowledge.\n</channel>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(raw: &str) -> Vec<Value> {
        raw.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect()
    }

    /// The host may only ever claim a transport write. A receipt that could say
    /// "read" or "acknowledged" would let an unread notice retire a batch.
    #[test]
    fn a_probe_notice_names_its_tool_and_disclaims_being_work() {
        let notice = probe_notice("child-c", "channel-7");
        assert!(notice.contains("kanna_confirm_event_channel"));
        assert!(notice.contains("channel-7"));
        assert!(notice.contains("not owner speech"));
        assert!(notice.contains("not anything to acknowledge"));
    }

    #[tokio::test]
    async fn a_wake_frame_is_written_as_one_channel_notification_carrying_the_server_text() {
        let stdout = Arc::new(Mutex::new(Vec::<u8>::new()));
        let host = ChannelHost {
            base_url: "http://127.0.0.1:1".into(),
            task_id: "child-c".into(),
            run_id: "manager-run".into(),
        };
        host.notify(&stdout, "<channel>batch 2</channel>").unwrap();
        let written = String::from_utf8(stdout.lock().unwrap().clone()).unwrap();
        let frames = frames(&written);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["method"], "notifications/claude/channel");
        assert_eq!(frames[0]["params"]["prompt"], "<channel>batch 2</channel>");
        // A notification carries no id: it is not a request and takes no reply.
        assert!(frames[0].get("id").is_none());
    }

    /// Frame dispatch: a probe asks for confirmation, a wake carries the
    /// server's own text through unchanged, and an unknown frame writes
    /// nothing at all rather than guessing what it meant.
    #[tokio::test]
    async fn frames_dispatch_to_one_notification_each_and_unknown_frames_write_nothing() {
        let stdout = Arc::new(Mutex::new(Vec::<u8>::new()));
        let host = ChannelHost {
            // Unroutable on purpose: the receipt round trip must not be able to
            // decide whether the notification was written.
            base_url: "http://127.0.0.1:1".into(),
            task_id: "child-c".into(),
            run_id: "manager-run".into(),
        };
        host.handle(&json!({"type": "probe", "channelId": "channel-9"}), &stdout)
            .await;
        host.handle(&json!({"type": "something-new"}), &stdout)
            .await;
        host.handle(
            &json!({"type": "wake", "attempt": {
                "id": "watch-1-2",
                "message": "<channel>server text</channel>",
                "binding": {"channelId": "channel-9"},
            }}),
            &stdout,
        )
        .await;
        let written = String::from_utf8(stdout.lock().unwrap().clone()).unwrap();
        let frames = frames(&written);
        assert_eq!(
            frames.len(),
            2,
            "an unknown frame writes nothing: {written}"
        );
        assert!(frames[0]["params"]["prompt"]
            .as_str()
            .unwrap()
            .contains("kanna_confirm_event_channel"));
        assert_eq!(
            frames[1]["params"]["prompt"], "<channel>server text</channel>",
            "the notice is the server's text, never rewritten here"
        );
    }

    /// Without the opt-in the capability is absent, so the CLI has no channel
    /// to route and this host never attaches.
    #[test]
    fn the_capability_is_absent_unless_the_run_opted_in() {
        // The process-wide env of a test binary is shared, so this asserts the
        // decision function rather than mutating it: with the variable unset
        // (the default in tests) there is no capability and no binding.
        if std::env::var(ENABLE_ENV).is_err() {
            assert!(experimental_capabilities().is_none());
            assert!(enabled().is_none());
        }
    }
}
