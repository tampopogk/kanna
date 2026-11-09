use super::*;

pub struct Candidate {
    pub entry: Entry,
    pub pane: Option<String>,
    pub on_desk: bool,
}

impl Tasks {
    pub fn merge_candidate(&self, reference: &str) -> Result<Candidate, KmuxError> {
        let id = self.resolve(reference)?.ok_or_else(|| not_found(reference))?;
        if let Some(entry) = self.entries.borrow().iter().find(|e| e.id == id).cloned() {
            let pane = entry.held.clone();
            return Ok(Candidate { entry, pane, on_desk: false });
        }
        let running = self.running.borrow().iter().find(|r| r.entry.id == id).map(|r| (r.entry.clone(), r.pane.clone(), r.place));
        let (entry, pane, place) = running.ok_or_else(|| KmuxError::new("not_found", format!("{} isn't on this machine's Desk or Shelf", self.short(&id))))?;
        Ok(Candidate { entry, pane: Some(pane), on_desk: place == Place::Desk })
    }

    pub fn queue_info(&self, entry: &Entry) -> Value {
        let Some(id) = entry.queued.as_deref() else { return Value::Null };
        let live = self.db.queued(true).unwrap_or_default();
        let Some(q) = live.iter().find(|q| q.id == id) else { return json!({ "id": id, "state": "queued" }) };
        let ahead = live.iter().filter(|o| o.repo == q.repo && o.queued_at < q.queued_at).count();
        let after = live.iter().filter(|o| o.repo == q.repo && o.queued_at < q.queued_at).last().map(|o| o.task.clone());
        json!({
            "id": q.id,
            "position": ahead + 1,
            "state": q.state,
            "stage": q.doc.get("stage").cloned().unwrap_or(Value::Null),
            "run": opt(q.run.as_deref()),
            "head": q.head,
            "merge": q.doc.get("merge").cloned().unwrap_or(Value::Null),
            "main": q.doc.get("main").cloned().unwrap_or(Value::Null),
            "since": num(q.doc.get("since").and_then(Value::as_f64).unwrap_or(q.queued_at)),
            "queuedAt": num(q.queued_at),
            "why": q.doc.get("why").cloned().unwrap_or(Value::Null),
            "after": opt(after.as_deref()),
            "afterName": opt(after.and_then(|a| self.known(&a)).and_then(|e| e.name).as_deref()),
            "by": q.doc.get("by").cloned().unwrap_or(Value::Null),
        })
    }

    pub fn queue_moved(&self) {
        self.changed();
    }

    pub fn short_of(&self, id: &str) -> String {
        self.short(id)
    }

    pub fn label_for(&self, id: &str) -> String {
        self.label(id)
    }

    pub fn enqueue(self: &Rc<Self>, candidate: Candidate, queue: &str, by: Option<String>, settled: impl FnOnce() + 'static) {
        let id = candidate.entry.id.clone();
        let queued = queue.to_string();
        let change = Change::new("queued").by(by.as_deref()).with("queue", json!(queue));
        let entry = self.alter(&id, change, |e| {
            e.queued = Some(queued.clone());
            e.merge_failed = None;
        });
        let Some(entry) = entry else { return settled() };
        self.tell_planner(&entry, What::Queued, by.as_deref());
        self.changed();
        let Some(pane) = candidate.pane.filter(|pane| self.core.upgrade().is_some_and(|core| core.pane(pane).is_some())) else { return settled() };
        if candidate.on_desk {
            let this = Rc::downgrade(self);
            return self.hold(pane.clone(), entry, Place::Desk, by.clone(), false, move |_, held| {
                if let Err(error) = held {
                    log!("kanna-daemon: {id} went into the merge queue but stays on the Desk: {}", error.message);
                }
                match this.upgrade() {
                    Some(this) => this.quit_for_queue(&pane, by, settled),
                    None => settled(),
                }
            });
        }
        self.quit_for_queue(&pane, by, settled);
    }

    fn quit_for_queue(self: &Rc<Self>, pane: &str, by: Option<String>, settled: impl FnOnce() + 'static) {
        let Some(core) = self.core.upgrade() else { return settled() };
        let close_tabs = {
            let weak = core.weak();
            let pane = pane.to_string();
            move || {
                let Some(core) = weak.upgrade() else { return settled() };
                let closing = pane.clone();
                core.handle(json!({ "cmd": "close", "args": { "task": pane, "tabs": true } }), move |reply| {
                    if let Some(error) = failed(&reply) {
                        log!("kanna-daemon: couldn't close {closing}'s tabs for the merge queue: {}", error.message);
                    }
                    settled();
                });
            }
        };
        if !alive(&core, pane) {
            return close_tabs();
        }
        let agent = self.pane_entry(pane).map_or_else(|| core.pane(pane).and_then(|p| p.command).unwrap_or_default(), |e| e.agent);
        let (this, weak, ending) = (Rc::downgrade(self), core.weak(), pane.to_string());
        let then = move |session: Option<String>| {
            let (Some(tasks), Some(core)) = (this.upgrade(), weak.upgrade()) else { return };
            tasks.keep_session(&ending, session);
            tasks.note(&ending, Change::new("stopped").by(Some("queue")));
            core.hang_up(&ending, move |_| close_tabs());
        };
        match by.as_deref() == Some(pane) {
            true => self.end_agent_after_turn(pane, &agent, then),
            false => self.end_agent(pane, &agent, then),
        }
    }

    pub fn landed(self: &Rc<Self>, id: &str) {
        log!("kanna-daemon: {id} landed from the merge queue, so it's Done");
        self.done(&json!({ "task": id, "by": "queue" }), self.unanswered());
    }

    pub fn merge_failed(self: &Rc<Self>, id: &str, queue: &str, failure: Value, text: &str) {
        let change = Change::new("merge failed").by(Some("queue")).with("queue", json!(queue)).with("why", failure.get("why").cloned().unwrap_or(Value::Null));
        let entry = self.alter(id, change, |e| {
            e.queued = None;
            e.merge_failed = Some(failure.clone());
        });
        let Some(entry) = entry else { return };
        if let Err(error) = self.post(id, Some(&entry.repo), queue, text) {
            log!("kanna-daemon: couldn't tell {id} its merge failed: {}", error.message);
        }
        self.tell_planner(&entry, What::MergeFailed, Some("queue"));
        self.changed();
        self.refill();
    }

    pub fn unqueue(&self, id: &str, by: Option<&str>) {
        if self.alter(id, Change::new("unqueued").by(by), |e| e.queued = None).is_some() {
            self.changed();
        }
    }
}
