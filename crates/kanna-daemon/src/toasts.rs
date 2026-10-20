use crate::backlog::Backlog;
use kanna_mux::json::{json, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, Responder, WeakCore};
use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

pub const HELD_FOR: Duration = Duration::from_secs(60);

pub const HELD_AT_MOST: usize = 8;

pub const TITLE_MAX: usize = 120;

pub const DETAIL_MAX: usize = 400;

#[derive(Default)]
pub struct Toast {
    pub title: String,
    pub detail: Option<String>,
    pub pane: Option<String>,
    pub task: Option<String>,
    pub key: Option<String>,
}

pub struct Toasts {
    core: WeakCore,
    backlog: RefCell<Weak<Backlog>>,
    next: Cell<u64>,
    keys: RefCell<HashSet<String>>,
    held: RefCell<VecDeque<(Instant, Value)>>,
    pub on_toast: RefCell<Option<Box<dyn Fn(&Value) -> bool>>>,
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn clean(text: &str, max: usize) -> String {
    let line: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", line[..at].trim_end()),
        None => line,
    }
}

impl Toasts {
    pub fn new(core: WeakCore) -> Toasts {
        Toasts { core, backlog: RefCell::default(), next: Cell::new(0), keys: RefCell::default(), held: RefCell::default(), on_toast: RefCell::default() }
    }

    pub fn connect_backlog(&self, backlog: &Rc<Backlog>) {
        *self.backlog.borrow_mut() = Rc::downgrade(backlog);
    }

    pub fn raise(&self, toast: Toast) -> Result<Option<(String, bool)>, KmuxError> {
        let title = clean(&toast.title, TITLE_MAX);
        if title.is_empty() {
            return Err(bad("a toast needs a title"));
        }
        let (pane, task, about) = self.about(toast.pane.as_deref(), toast.task.as_deref())?;
        if let Some(key) = &toast.key {
            if !self.keys.borrow_mut().insert(key.clone()) {
                return Ok(None);
            }
        }
        let id = format!("toast-{}", self.next.get() + 1);
        self.next.set(self.next.get() + 1);
        let detail = toast.detail.as_deref().map(|d| clean(d, DETAIL_MAX)).filter(|d| !d.is_empty());
        let record = json!({ "id": id, "title": title, "detail": detail, "pane": pane, "task": task, "about": about });
        let shown = self.on_toast.borrow().as_ref().is_some_and(|show| show(&record));
        if !shown {
            let mut held = self.held.borrow_mut();
            held.push_back((Instant::now(), record));
            while held.len() > HELD_AT_MOST {
                held.pop_front();
            }
        }
        Ok(Some((id, shown)))
    }

    pub fn attached(&self) {
        let now = Instant::now();
        let held: Vec<Value> = self.held.borrow_mut().drain(..).filter(|(at, _)| now.duration_since(*at) < HELD_FOR).map(|(_, record)| record).collect();
        let Some(show) = self.on_toast.borrow_mut().take() else { return };
        for record in held {
            show(&record);
        }
        *self.on_toast.borrow_mut() = Some(show);
    }

    fn about(&self, pane: Option<&str>, task: Option<&str>) -> Result<(Option<String>, Option<String>, Option<String>), KmuxError> {
        let backlog = self.backlog.borrow().upgrade();
        let (mut pane_id, mut label) = (None, None);
        if let Some(reference) = pane {
            let core = self.core.upgrade().ok_or_else(|| KmuxError::new("app_gone", "the daemon is stopping"))?;
            let found = core.pane(reference).ok_or_else(|| KmuxError::new("not_found", format!("no pane \"{reference}\"")))?;
            label = Some(found.name.clone().unwrap_or_else(|| found.id.clone()));
            pane_id = Some(found.id);
        }
        let task = match (task, &pane_id, &backlog) {
            (Some(reference), _, Some(backlog)) => Some(backlog.about(reference)?),
            (Some(reference), _, None) => return Err(KmuxError::new("not_found", format!("no task \"{reference}\""))),
            (None, Some(pane), Some(backlog)) => backlog.task_of(pane).and_then(|id| backlog.about(&id).ok()),
            (None, _, _) => None,
        };
        let Some((id, name, agent)) = task else { return Ok((pane_id, None, label)) };
        Ok((pane_id.or(agent), Some(id.clone()), name.or(label).or(Some(id))))
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let this = Rc::downgrade(self);
        core.add_command(
            "toast",
            Rc::new(move |args, responder: Responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                responder.reply(this.command(&args))
            }),
        );
    }

    fn command(&self, args: &Value) -> Result<Map, KmuxError> {
        let text = |key: &str| string(args, key).map(String::from);
        let toast = Toast { title: text("title").unwrap_or_default(), detail: text("detail"), pane: text("pane"), task: text("task"), key: text("key").filter(|k| !k.is_empty()) };
        let key = toast.key.clone();
        let mut out = Map::new();
        match self.raise(toast)? {
            Some((id, shown)) => {
                out.insert("toast".into(), json!(id));
                out.insert("shown".into(), json!(shown));
            }
            None => {
                out.insert("toast".into(), Value::Null);
                out.insert("already".into(), json!(key));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toasts() -> (Toasts, Rc<RefCell<Vec<Value>>>) {
        let toasts = Toasts::new(kanna_mux::Core::default().weak());
        (toasts, Rc::default())
    }

    fn show(toasts: &Toasts, seen: &Rc<RefCell<Vec<Value>>>) {
        let seen = seen.clone();
        *toasts.on_toast.borrow_mut() = Some(Box::new(move |record| {
            seen.borrow_mut().push(record.clone());
            true
        }));
    }

    fn titled(title: &str, key: Option<&str>) -> Toast {
        Toast { title: title.into(), detail: Some("a line\nof detail".into()), key: key.map(String::from), ..Toast::default() }
    }

    #[test]
    fn a_keyed_toast_is_raised_once() {
        let (toasts, seen) = toasts();
        show(&toasts, &seen);
        assert_eq!(toasts.raise(titled("Messages wait", Some("m:term-1:why"))).unwrap(), Some(("toast-1".into(), true)));
        assert_eq!(toasts.raise(titled("Messages wait", Some("m:term-1:why"))).unwrap(), None);
        assert_eq!(toasts.raise(titled("Messages wait", Some("m:term-1:other"))).unwrap(), Some(("toast-2".into(), true)));
        assert_eq!(toasts.raise(titled("Plain", None)).unwrap(), Some(("toast-3".into(), true)));
        assert_eq!(toasts.raise(titled("Plain", None)).unwrap(), Some(("toast-4".into(), true)));
        assert_eq!(seen.borrow().len(), 4);
        assert_eq!(seen.borrow()[0]["detail"], json!("a line of detail"));
    }

    #[test]
    fn toasts_raised_with_no_app_wait_for_one() {
        let (toasts, seen) = toasts();
        for n in 0..HELD_AT_MOST + 2 {
            assert_eq!(toasts.raise(titled(&format!("t{n}"), None)).unwrap().map(|(_, shown)| shown), Some(false));
        }
        show(&toasts, &seen);
        toasts.attached();
        let titles: Vec<String> = seen.borrow().iter().map(|r| r["title"].as_str().unwrap().to_string()).collect();
        assert_eq!(titles.len(), HELD_AT_MOST);
        assert_eq!(titles[0], "t2");
        toasts.attached();
        assert_eq!(seen.borrow().len(), HELD_AT_MOST);
    }

    #[test]
    fn a_toast_needs_a_title_and_long_text_is_cut() {
        let (toasts, _) = toasts();
        assert!(toasts.raise(titled("  \n ", None)).is_err());
        assert_eq!(clean(&"x".repeat(TITLE_MAX + 5), TITLE_MAX).chars().count(), TITLE_MAX + 1);
        assert_eq!(clean("tab\there", 40), "tab here");
    }
}
