//! Durable hosted input attempts. Pending/uncertain inputs are not delivered
//! conversation. Confirmation and its ledger outbox entry commit together.
use super::Db;
use crate::mutation_provenance::ChannelIdentity;
use kanna_agent_protocol::hosted_frontend::{Binding, Delivery, DeliveryState, Snapshot};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) fn create_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_input_delivery (
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        id TEXT NOT NULL,
        attempt_json TEXT NOT NULL,
        PRIMARY KEY(task_id, id)
    );",
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Attempt {
    pub id: String,
    pub binding: Binding,
    pub run_id: String,
    pub stage: Option<String>,
    pub sequence: Option<u64>,
    pub payload_hash: String,
    #[serde(default)]
    pub request_fingerprint: Option<String>,
    pub message: String,
    pub source: String,
    pub channel_identity: ChannelIdentity,
    pub state: DeliveryState,
    pub error: Option<String>,
    pub input_id: Option<i64>,
    pub initial_prompt: bool,
}

fn decode(text: String) -> rusqlite::Result<Attempt> {
    serde_json::from_str(&text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

impl Db {
    pub(crate) fn task_input_delivery(
        &self,
        task: &str,
        id: &str,
    ) -> rusqlite::Result<Option<Attempt>> {
        self.conn
            .query_row(
                "SELECT attempt_json FROM task_input_delivery WHERE task_id = ? AND id = ?",
                params![task, id],
                |row| row.get(0),
            )
            .optional()?
            .map(decode)
            .transpose()
    }

    pub(crate) fn task_input_deliveries(&self, task: &str) -> rusqlite::Result<Vec<Attempt>> {
        let mut stmt = self.conn.prepare(
            "SELECT attempt_json FROM task_input_delivery WHERE task_id = ? ORDER BY rowid",
        )?;
        let result = stmt
            .query_map([task], |row| row.get::<_, String>(0))?
            .map(|row| decode(row?))
            .collect();
        result
    }

    fn save_input_delivery(&self, attempt: &Attempt) -> rusqlite::Result<()> {
        let json = serde_json::to_string(attempt)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        self.conn.execute("INSERT INTO task_input_delivery(task_id,id,attempt_json) VALUES(?,?,?) ON CONFLICT(task_id,id) DO UPDATE SET attempt_json=excluded.attempt_json", params![attempt.binding.task_id, attempt.id, json])?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_task_input_delivery(
        &self,
        binding: &Binding,
        active_run_id: &str,
        id: &str,
        message: &str,
        source: &str,
        channel: &ChannelIdentity,
        initial_prompt: bool,
        request_fingerprint: Option<String>,
    ) -> rusqlite::Result<Attempt> {
        self.with_immediate_transaction(|_| {
            let hash = format!("{:x}", Sha256::digest(message.as_bytes()));
            if let Some(existing) = self.task_input_delivery(&binding.task_id, id)? {
                if existing.payload_hash != hash || existing.binding != *binding {
                    return Err(rusqlite::Error::InvalidParameterName("delivery id belongs to another payload or session incarnation".into()));
                }
                return Ok(existing);
            }
            let (run_id, stage): (String, Option<String>) = if initial_prompt {
                self.conn.query_row("SELECT id, stage FROM stage_run WHERE id = ? AND task_id = ?", params![binding.run_id, binding.task_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            } else {
                let running = self.conn.query_row("SELECT id, stage FROM stage_run WHERE task_id = ? AND status = 'running' AND kind IN ('main','post') ORDER BY rowid DESC LIMIT 1", [&binding.task_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
                match running {
                    Some(run) => run,
                    // A live frontend still accepts operator input at a manual
                    // gate. Its active run can differ from its original binding.
                    None => self.conn.query_row("SELECT id, stage FROM stage_run WHERE id = ? AND task_id = ?", params![active_run_id, binding.task_id], |row| Ok((row.get(0)?, row.get(1)?)))?,
                }
            };
            let attempt = Attempt { id: id.into(), binding: binding.clone(), run_id, stage, sequence: None, payload_hash: hash,
                request_fingerprint, message: message.into(), source: source.into(), channel_identity: channel.clone(), state: DeliveryState::Queued,
                error: None, input_id: None, initial_prompt };
            self.save_input_delivery(&attempt)?;
            Ok(attempt)
        })
    }

    pub(crate) fn fail_task_input_delivery(
        &self,
        task: &str,
        id: &str,
        uncertain: bool,
        reason: &str,
    ) -> rusqlite::Result<()> {
        self.with_immediate_transaction(|_| {
            if let Some(mut attempt) = self.task_input_delivery(task, id)? {
                if attempt.state != DeliveryState::Submitted {
                    attempt.state = if uncertain {
                        DeliveryState::Uncertain
                    } else {
                        DeliveryState::Failed
                    };
                    attempt.error = Some(reason.into());
                    self.save_input_delivery(&attempt)?;
                }
            }
            Ok(())
        })
    }

    pub(crate) fn reconcile_task_input_delivery(
        &self,
        binding: &Binding,
        delivery: &Delivery,
    ) -> rusqlite::Result<()> {
        self.with_immediate_transaction(|_| {
            let Some(mut attempt) = self.task_input_delivery(&binding.task_id, &delivery.delivery_id)? else { return Ok(()) };
            if attempt.binding != *binding || attempt.payload_hash != delivery.payload_hash { return Ok(()) }
            if attempt.state == DeliveryState::Submitted { return Ok(()) }
            if matches!(attempt.state, DeliveryState::Failed | DeliveryState::Uncertain) && delivery.state.pending() { return Ok(()) }
            if attempt.run_id != delivery.run_id { return Ok(()) }
            attempt.state = delivery.state;
            attempt.sequence = Some(delivery.sequence);
            attempt.error = delivery.error.clone();
            if delivery.state == DeliveryState::Submitted && !attempt.initial_prompt && attempt.input_id.is_none() {
                self.conn.execute("INSERT INTO task_input(task_id,run_id,stage,source,channel_identity,message) VALUES(?,?,?,?,?,?)",
                    params![binding.task_id, attempt.run_id, attempt.stage, attempt.source, attempt.channel_identity.to_column(), attempt.message])?;
                let input_id = self.conn.last_insert_rowid();
                self.enqueue_task_input_entry(&binding.task_id, input_id, Some(&attempt.run_id), attempt.stage.as_deref(), &attempt.source, &attempt.channel_identity, &attempt.message, None, None, None)?;
                self.append_task_event(&binding.task_id, super::TaskEventKind::InputDelivered, serde_json::json!({
                    "inputId":input_id,"deliveryId":attempt.id,"source":attempt.source,"channelIdentity":attempt.channel_identity.to_json(),"runId":attempt.run_id,"stage":attempt.stage
                }))?;
                attempt.input_id = Some(input_id);
            }
            self.save_input_delivery(&attempt)
        })
    }

    pub(crate) fn reconcile_missing_hosted_sessions(
        &self,
        live: &std::collections::HashMap<String, String>,
    ) -> rusqlite::Result<()> {
        let mut stmt = self.conn.prepare("SELECT attempt_json FROM task_input_delivery WHERE json_extract(attempt_json, '$.state') IN ('queued','submitting')")?;
        let attempts = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for encoded in attempts {
            let attempt = decode(encoded)?;
            if live.get(&attempt.binding.session_id) != Some(&attempt.binding.incarnation) {
                if let Ok(frontend) =
                    kanna_daemon::hosted_frontend::Frontend::for_binding(&attempt.binding)
                {
                    self.reconcile_hosted_frontend(&frontend.final_snapshot())?;
                    if self
                        .task_input_delivery(&attempt.binding.task_id, &attempt.id)?
                        .is_some_and(|saved| !saved.state.pending())
                    {
                        continue;
                    }
                }
                self.fail_task_input_delivery(&attempt.binding.task_id, &attempt.id, true, "original frontend incarnation is no longer live; inspect its native transcript before resending")?;
            }
        }
        Ok(())
    }

    pub(crate) fn reconcile_hosted_frontend(&self, snapshot: &Snapshot) -> rusqlite::Result<()> {
        // Identity is persisted on the spawned run, never whichever run happens
        // to be latest when this asynchronous event reaches the server.
        if let Some(id) = &snapshot.provider_session_id {
            self.record_stage_run_provider_session_id(
                &snapshot.binding.run_id,
                &snapshot.binding.session_id,
                id,
            )?;
            let context: Option<(String, String)> = self.conn.query_row(
                "SELECT agent_provider, COALESCE(cwd, '') FROM stage_run WHERE id = ? AND session_id = ? AND provider_session_id = ? AND (transcript_ref IS NULL OR json_extract(transcript_ref, '$.path') IS NULL)",
                params![snapshot.binding.run_id, snapshot.binding.session_id, id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
            if let Some((provider, cwd)) = context {
                if let Some(reference) =
                    crate::task_creator::transcript_ref(&provider, Some(id), &cwd)
                {
                    self.conn.execute(
                        "UPDATE stage_run SET transcript_ref = ? WHERE id = ?",
                        params![
                            serde_json::to_string(&reference).unwrap(),
                            snapshot.binding.run_id
                        ],
                    )?;
                }
            }
        }
        for delivery in &snapshot.deliveries {
            if self
                .task_input_delivery(&snapshot.binding.task_id, &delivery.delivery_id)?
                .is_none()
            {
                if let Some(text) = &delivery.text {
                    // The frontend's initial prompt can precede watcher attach.
                    // Its text is already durable on the run and is not an
                    // operator input or a second delivered ledger entry.
                    if delivery.initial_prompt {
                        let mut binding = snapshot.binding.clone();
                        binding.run_id = delivery.run_id.clone();
                        let mut attempt = self.prepare_task_input_delivery(
                            &binding,
                            &binding.run_id,
                            &delivery.delivery_id,
                            text,
                            "engine",
                            &ChannelIdentity::Server,
                            true,
                            None,
                        )?;
                        attempt.binding = snapshot.binding.clone();
                        self.save_input_delivery(&attempt)?;
                    }
                }
            }
            self.reconcile_task_input_delivery(&snapshot.binding, delivery)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Db, Binding) {
        let db = Db::open_for_tests(&Db::test_db_path("hosted-input")).unwrap();
        db.insert_test_repo("repo-host", "Host").unwrap();
        db.insert_test_pipeline_item(
            "task-host",
            "repo-host",
            "prompt",
            None,
            "in progress",
            "2026-09-28 00:00:00",
        )
        .unwrap();
        db.insert_stage_run(super::super::NewStageRun {
            id: "run-host",
            task_id: "task-host",
            stage: "in progress",
            kind: "main",
            agent: None,
            agent_provider: Some("codex"),
            model: None,
            effort: None,
            status: "running",
            result: None,
            feedback: None,
            session_id: Some("task-host"),
            provider_session_id: None,
            cwd: None,
            resumed_from_run_id: None,
        })
        .unwrap();
        (
            db,
            Binding {
                task_id: "task-host".into(),
                run_id: "run-host".into(),
                session_id: "task-host".into(),
                incarnation: "incarnation-1".into(),
            },
        )
    }

    #[test]
    fn provider_receipt_records_one_input_with_original_provenance() {
        let (db, binding) = fixture();
        let attempt = db
            .prepare_task_input_delivery(
                &binding,
                &binding.run_id,
                "delivery-1",
                "first\nsecond",
                "operator",
                &ChannelIdentity::Server,
                false,
                Some("request-hash".into()),
            )
            .unwrap();
        assert!(db.list_task_inputs("task-host", 100).unwrap().is_empty());
        let mut delivery = Delivery {
            initial_prompt: false,
            run_id: binding.run_id.clone(),
            delivery_id: attempt.id.clone(),
            sequence: 1,
            payload_hash: attempt.payload_hash,
            text: None,
            state: DeliveryState::Submitting,
            error: None,
        };
        db.reconcile_task_input_delivery(&binding, &delivery)
            .unwrap();
        assert!(db.list_task_inputs("task-host", 100).unwrap().is_empty());
        delivery.state = DeliveryState::Submitted;
        db.reconcile_task_input_delivery(&binding, &delivery)
            .unwrap();
        db.reconcile_task_input_delivery(&binding, &delivery)
            .unwrap();
        let inputs = db.list_task_inputs("task-host", 100).unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].message, "first\nsecond");
        assert_eq!(inputs[0].source, "operator");
        assert_eq!(inputs[0].run_id.as_deref(), Some("run-host"));
        assert_eq!(
            db.task_input_delivery("task-host", "delivery-1")
                .unwrap()
                .unwrap()
                .input_id,
            Some(inputs[0].id)
        );
        assert!(db
            .prepare_task_input_delivery(
                &binding,
                &binding.run_id,
                "delivery-1",
                "changed",
                "operator",
                &ChannelIdentity::Server,
                false,
                None
            )
            .is_err());
    }

    #[test]
    fn finished_run_uses_frontend_active_run_and_records_one_input() {
        let (db, mut binding) = fixture();
        db.finish_stage_run("run-host", "succeeded", Some("done"), None)
            .unwrap();
        // A retained frontend may have started on an earlier run.
        binding.run_id = "run-original".into();
        let attempt = db
            .prepare_task_input_delivery(
                &binding,
                "run-host",
                "after-finish",
                "One more change",
                "operator",
                &ChannelIdentity::Server,
                false,
                None,
            )
            .unwrap();
        assert_eq!(attempt.run_id, "run-host");
        assert_eq!(attempt.stage.as_deref(), Some("in progress"));
        assert_eq!(attempt.state, DeliveryState::Queued);
        assert!(db.list_task_inputs("task-host", 100).unwrap().is_empty());
        let delivery = Delivery {
            initial_prompt: false,
            run_id: attempt.run_id,
            delivery_id: attempt.id,
            sequence: 1,
            payload_hash: attempt.payload_hash,
            text: None,
            state: DeliveryState::Submitted,
            error: None,
        };
        db.reconcile_task_input_delivery(&binding, &delivery)
            .unwrap();
        db.reconcile_task_input_delivery(&binding, &delivery)
            .unwrap();
        let inputs = db.list_task_inputs("task-host", 100).unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].run_id.as_deref(), Some("run-host"));
        assert_eq!(inputs[0].stage.as_deref(), Some("in progress"));
        assert_eq!(inputs[0].message, "One more change");
    }

    #[test]
    fn stale_incarnation_cannot_confirm_or_relabel_an_attempt() {
        let (db, binding) = fixture();
        let attempt = db
            .prepare_task_input_delivery(
                &binding,
                &binding.run_id,
                "delivery-1",
                "message",
                "manager",
                &ChannelIdentity::Server,
                false,
                None,
            )
            .unwrap();
        let mut stale = binding.clone();
        stale.incarnation = "previous".into();
        let delivery = Delivery {
            initial_prompt: false,
            run_id: binding.run_id.clone(),
            delivery_id: attempt.id,
            sequence: 1,
            payload_hash: attempt.payload_hash,
            text: None,
            state: DeliveryState::Submitted,
            error: None,
        };
        db.reconcile_task_input_delivery(&stale, &delivery).unwrap();
        assert!(db.list_task_inputs("task-host", 100).unwrap().is_empty());
        db.fail_task_input_delivery("task-host", "delivery-1", true, "lost response")
            .unwrap();
        let attempt = db
            .task_input_delivery("task-host", "delivery-1")
            .unwrap()
            .unwrap();
        assert_eq!(attempt.state, DeliveryState::Uncertain);
        assert_eq!(attempt.message, "message");
        let state = db.task_state_record("task-host").unwrap();
        assert!(serde_json::to_string(&state)
            .unwrap()
            .contains("task_input_delivery"));
    }
    #[test]
    fn uncertain_delivery_retains_attachments_until_explicitly_failed() {
        let (db, binding) = fixture();
        let path = db.conn.path().to_string();
        let directory =
            crate::task_input_attachments::task_attachments_dir(&path, &binding.task_id);
        std::fs::create_dir_all(&directory).unwrap();
        let attachment = directory.join("fixture.png");
        std::fs::write(&attachment, b"fixture").unwrap();
        db.prepare_task_input_delivery(
            &binding,
            &binding.run_id,
            "uncertain",
            "see fixture.png",
            "operator",
            &ChannelIdentity::Server,
            false,
            None,
        )
        .unwrap();
        db.fail_task_input_delivery(&binding.task_id, "uncertain", true, "lost acknowledgement")
            .unwrap();
        crate::task_input_attachments::remove_task_attachments(&path, &binding.task_id);
        assert!(attachment.exists());
        db.fail_task_input_delivery(&binding.task_id, "uncertain", false, "explicitly abandoned")
            .unwrap();
        crate::task_input_attachments::remove_task_attachments(&path, &binding.task_id);
        assert!(!attachment.exists());
    }

    #[test]
    fn hosted_pending_attempt_survives_rebuild_and_blocks_transfer() {
        let (db, binding) = fixture();
        db.prepare_task_input_delivery(
            &binding,
            &binding.run_id,
            "pending",
            "retained\nmessage",
            "operator",
            &ChannelIdentity::Server,
            false,
            None,
        )
        .unwrap();
        assert!(db
            .transfer_state_blocker("task-host")
            .unwrap()
            .unwrap()
            .contains("hosted input"));
        let root = crate::test_paths::unique_test_dir("hosted-rebuild")
            .join("repos/repo-host/tasks/task-host");
        std::fs::create_dir_all(root.join("ledger")).unwrap();
        std::fs::write(
            root.join("task.json"),
            serde_json::to_vec(&db.task_snapshot_facts("task-host").unwrap().unwrap()).unwrap(),
        )
        .unwrap();
        let directory = crate::task_store::rebuild::read_task_directory(&root).unwrap();
        let projection = crate::task_store::rebuild::project(&[directory]);
        let rebuilt = Db::open_migrated(&Db::test_db_path("hosted-rebuilt")).unwrap();
        rebuilt.apply_disk_projection(&projection).unwrap();
        rebuilt.apply_disk_projection(&projection).unwrap();
        let attempt = rebuilt
            .task_input_delivery("task-host", "pending")
            .unwrap()
            .unwrap();
        assert_eq!(attempt.state, DeliveryState::Queued);
        assert_eq!(attempt.message, "retained\nmessage");
        assert_eq!(attempt.binding, binding);
        assert!(rebuilt
            .list_task_inputs("task-host", 100)
            .unwrap()
            .is_empty());
        rebuilt
            .reconcile_missing_hosted_sessions(&Default::default())
            .unwrap();
        assert_eq!(
            rebuilt
                .task_input_delivery("task-host", "pending")
                .unwrap()
                .unwrap()
                .state,
            DeliveryState::Uncertain
        );
        assert!(rebuilt
            .transfer_state_blocker("task-host")
            .unwrap()
            .is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
}
