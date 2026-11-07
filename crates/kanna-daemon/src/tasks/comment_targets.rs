use super::*;

pub(super) const REPAIRED: &str = "comment_targets_repaired";

fn doc_file(row: &Value) -> Option<PathBuf> {
    let path = string(row, "path").or_else(|| string(row, "url")?.strip_prefix("file://"))?;
    Some(PathBuf::from(path.split(['#', '?']).next()?))
}

impl Tasks {
    pub fn repair_comment_targets(self: &Rc<Self>) {
        if !matches!(self.db.meta(REPAIRED), Ok(None)) {
            return;
        }
        let Some(core) = self.core.upgrade() else { return };
        let this = Rc::downgrade(self);
        core.handle(json!({ "cmd": "list" }), move |reply| {
            if let Some(error) = failed(&reply) {
                return log!("kanna-daemon: couldn't list the panes to repair comment targets: {}", error.message);
            }
            if let Some(this) = this.upgrade() {
                this.repoint_comment_targets(reply.get("panes").and_then(Value::as_array).map_or(&[], Vec::as_slice));
            }
        });
    }

    fn repoint_comment_targets(&self, rows: &[Value]) {
        let Some(core) = self.core.upgrade() else { return };
        let dead = |agent: &str| !agent.contains('@') && core.pane(agent).is_none();
        let current = |task: &str| self.pane_of(task).filter(|pane| core.pane(pane).is_some());
        let mut repaired = 0;
        for row in rows.iter().filter(|row| matches!(string(row, "type"), Some("md" | "web"))) {
            let Some((id, _)) = string(row, "id").zip(string(row, "agent").filter(|agent| dead(agent))) else { continue };
            let in_workspace = string(row, "workspace").and_then(|workspace| self.task_in(workspace)).map(|(task, _)| task);
            let task = in_workspace.or_else(|| self.task_in_folder(doc_file(row)?.parent()?).map(|entry| entry.id));
            if let Some(pane) = task.and_then(|task| current(&task)) {
                core.set_agent(id, &pane);
                repaired += 1;
            }
        }
        for entry in self.open_tasks() {
            let (Some(pane), Some(folder)) = (current(&entry.id), entry.folder.as_deref()) else { continue };
            let stores = fs::read_dir(Path::new(folder).join(".kanna/docs")).into_iter().flatten().flatten();
            for store in stores.map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|e| e == "json")) {
                let Some(doc) = kanna_threads::read_no_follow(&store).and_then(|raw| serde_json::from_slice::<Value>(&raw).ok()) else { continue };
                let Some(file) = string(&doc, "file").map(PathBuf::from).filter(|file| kanna_threads::store_path(file) == store) else { continue };
                if !string(&doc, "agent").is_some_and(&dead) {
                    continue;
                }
                let set = |doc: &mut Value| {
                    doc["agent"] = json!(pane);
                    Ok(())
                };
                match kanna_threads::update(&file, set, |why| why) {
                    Ok(()) => repaired += 1,
                    Err(why) => log!("kanna-daemon: couldn't re-point {}'s comments: {why}", file.display()),
                }
            }
        }
        if repaired > 0 {
            log!("kanna-daemon: re-pointed {repaired} docs at their tasks' panes");
        }
        if let Err(error) = self.db.put_meta(REPAIRED, "1") {
            log!("kanna-daemon: couldn't note that comment targets are repaired: {error}");
        }
    }
}
