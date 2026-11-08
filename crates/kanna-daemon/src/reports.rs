use crate::db::Db;
use kanna_mux::json::{json, Value};
use kanna_vt::{Need, ProgramState, ProgramStatus};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Kept {
    pub owner: Option<i32>,
    pub records: Vec<ProgramStatus>,
}

impl Kept {
    pub fn to_doc(&self) -> Value {
        let records: Vec<Value> = self
            .records
            .iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "state": r.state.as_str(),
                    "need": r.need.map(Need::as_str),
                    "progress": r.progress,
                    "app": r.app,
                    "title": r.title,
                    "message": r.message,
                })
            })
            .collect();
        json!({ "owner": self.owner, "records": records })
    }

    pub fn from_doc(doc: &Value) -> Kept {
        let text = |r: &Value, key: &str| r.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let records = doc
            .get("records")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let state = ProgramState::parse(r.get("state")?.as_str()?)?;
                Some(ProgramStatus {
                    id: text(r, "id"),
                    state,
                    need: r.get("need").and_then(Value::as_str).and_then(Need::parse).filter(|_| state == ProgramState::Blocked),
                    progress: r.get("progress").and_then(Value::as_u64).filter(|p| *p <= 100).map(|p| p as u8),
                    app: text(r, "app"),
                    title: text(r, "title"),
                    message: text(r, "message"),
                })
            })
            .take(kanna_vt::MAX_PROGRAM_STATUSES)
            .collect();
        let owner = doc.get("owner").and_then(Value::as_i64).and_then(|g| i32::try_from(g).ok()).filter(|g| *g > 0);
        Kept { owner, records }
    }
}

pub fn load(db: &Db) -> HashMap<String, Kept> {
    match db.reports() {
        Ok(rows) => rows.into_iter().map(|(terminal, doc)| (terminal, Kept::from_doc(&doc))).collect(),
        Err(error) => {
            log!("kanna-daemon: can't read the agents' last reports: {error}");
            HashMap::new()
        }
    }
}

pub fn save(db: &Db, kept: &[(String, Option<Kept>)]) {
    let rows: Vec<(String, Option<Value>)> = kept.iter().map(|(terminal, kept)| (terminal.clone(), kept.as_ref().filter(|k| !k.records.is_empty()).map(Kept::to_doc))).collect();
    if let Err(error) = db.put_reports(&rows) {
        log!("kanna-daemon: can't keep the agents' last reports: {error}");
    }
}

pub fn keep_only(db: &Db, terminals: &[String]) {
    if let Err(error) = db.keep_reports(terminals) {
        log!("kanna-daemon: can't drop the reports of terminals that are gone: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, state: ProgramState) -> ProgramStatus {
        ProgramStatus { id: id.into(), state, need: None, progress: None, app: String::new(), title: String::new(), message: String::new() }
    }

    #[test]
    fn a_kept_report_reads_back_whole() {
        let mut blocked = record("", ProgramState::Blocked);
        (blocked.need, blocked.progress, blocked.app, blocked.title, blocked.message) = (Some(Need::Permission), Some(40), "claude".into(), "Build".into(), "Apply?".into());
        let kept = Kept { owner: Some(4242), records: vec![record("sub", ProgramState::Done), blocked] };
        assert_eq!(Kept::from_doc(&kept.to_doc()), kept);
    }

    #[test]
    fn a_doc_from_elsewhere_keeps_only_what_makes_sense() {
        let doc = json!({ "owner": -3, "records": [
            { "state": "idle", "need": "permission", "progress": 400 },
            { "state": "sleeping" },
            { "id": "x" },
        ] });
        assert_eq!(Kept::from_doc(&doc), Kept { owner: None, records: vec![record("", ProgramState::Idle)] });
        assert_eq!(Kept::from_doc(&Value::Null), Kept::default());
    }

    #[test]
    fn reports_are_kept_per_terminal_replaced_cleared_and_pruned() {
        let dir = std::env::temp_dir().join(format!("kanna-reports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir);
        let idle = Kept { owner: Some(7), records: vec![record("", ProgramState::Idle)] };
        let working = Kept { owner: Some(8), records: vec![record("", ProgramState::Working)] };
        save(&db, &[("term-1".into(), Some(working.clone())), ("term-2".into(), Some(idle.clone())), ("term-3".into(), Some(idle.clone()))]);
        save(&db, &[("term-1".into(), Some(idle.clone()))]);
        save(&db, &[("term-2".into(), None)]);
        save(&db, &[("term-4".into(), Some(Kept { owner: Some(9), records: vec![] }))]);
        drop(db);
        let db = Db::open(&dir);
        assert_eq!(load(&db), HashMap::from([("term-1".to_string(), idle.clone()), ("term-3".to_string(), idle.clone())]), "an empty one is no row");
        keep_only(&db, &["term-3".into()]);
        assert_eq!(load(&db), HashMap::from([("term-3".to_string(), idle)]));
    }
}
