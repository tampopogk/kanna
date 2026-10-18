use kanna_mux::json::{Map, Value};
use kanna_mux::{pane_table, read_pane_table, KmuxError, LayoutCall, LayoutHost, LayoutLink, LayoutOwner, Model, PaneState, PaneType, Panes, StateFile, WeakCore};
use crate::db::Db;
use kanna_mux::Contents;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::ready;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

const SAVE_AFTER: Duration = Duration::from_millis(500);

pub struct DaemonLayout {
    owner: RefCell<LayoutOwner>,
    db: Option<Rc<Db>>,
    core: RefCell<Option<WeakCore>>,
    panes_changed: Cell<bool>,
    touched: Cell<bool>,
    save_at: Cell<Option<Instant>>,
    shown: RefCell<Option<Value>>,
    pub on_view: RefCell<Option<Box<dyn Fn(&Value)>>>,
}

impl DaemonLayout {
    pub fn open(db: Rc<Db>, state_dir: &Path) -> Rc<DaemonLayout> {
        let flag = |name: &str| std::env::var(name).is_ok_and(|v| v == "1");
        let db = (!flag("KANNA_NO_STATE")).then_some(db);
        let saved = match &db {
            Some(_) if flag("KANNA_FRESH") => None,
            Some(db) => saved(db, state_dir),
            None => None,
        };
        let owner = saved.and_then(|state| {
            LayoutOwner::restoring(state).map_err(|error| eprintln!("kanna-daemon: not restoring the saved layout: {}", error.message)).ok()
        });
        let layout = Rc::new(DaemonLayout {
            owner: RefCell::new(owner.unwrap_or_default()),
            db: db.clone(),
            core: RefCell::new(None),
            panes_changed: Cell::new(false),
            touched: Cell::new(false),
            save_at: Cell::new(None),
            shown: RefCell::new(None),
            on_view: RefCell::new(None),
        });
        if let Some(db) = db {
            let weak = Rc::downgrade(&layout);
            db.saves_layout_with_done(move || weak.upgrade().and_then(|layout| layout.owner.try_borrow_mut().ok().and_then(|mut owner| owner.changed_state())));
        }
        layout
    }

    pub fn connect(&self, core: WeakCore) {
        *self.core.borrow_mut() = Some(core);
    }

    pub fn content_numbers(&self) -> HashMap<String, usize> {
        self.owner.borrow().content_numbers()
    }

    pub fn saved_panes(&self) -> Vec<kanna_mux::Pane> {
        let records = self.owner.borrow().hello();
        let mut panes = read_pane_table(&records).unwrap_or_else(|error| {
            eprintln!("kanna-daemon: the saved layout's panes can't be read ({}); leaving them out", error.message);
            vec![]
        });
        for pane in panes.iter_mut().filter(|p| p.kind != PaneType::Term && p.state == PaneState::Starting) {
            pane.state = PaneState::Running;
        }
        panes
    }

    pub fn restore(&self, panes: &Value) {
        match self.owner.borrow_mut().attach(panes) {
            Ok(reply) => {
                if let Some(problem) = reply.get("problem").and_then(Value::as_str) {
                    eprintln!("kanna-daemon: {problem}");
                }
            }
            Err(error) => eprintln!("kanna-daemon: the saved layout couldn't be restored: {}", error.message),
        }
        self.touched.set(true);
    }

    pub fn panes_changed(&self) {
        self.panes_changed.set(true);
    }

    pub fn view(&self) -> Value {
        Value::Object(self.owner.borrow().view())
    }

    pub fn flush(&self) {
        if self.panes_changed.take() {
            let table = self.core.borrow().as_ref().and_then(WeakCore::upgrade).map(|core| core.with_panes(pane_table));
            if let Some(table) = table {
                if let Err(error) = self.owner.borrow_mut().sync(&table) {
                    eprintln!("kanna-daemon: the layout couldn't take the pane table: {}", error.message);
                }
                self.touched.set(true);
            }
        }
        if self.touched.take() {
            let view = self.view();
            if self.shown.borrow().as_ref() != Some(&view) {
                if let Some(on_view) = self.on_view.borrow().as_ref() {
                    on_view(&view);
                }
                *self.shown.borrow_mut() = Some(view);
            }
            if self.save_at.get().is_none() {
                self.save_at.set(Some(Instant::now() + SAVE_AFTER));
            }
        }
        if self.save_at.get().is_some_and(|at| at <= Instant::now()) {
            self.save_now();
        }
    }

    pub fn save_now(&self) {
        self.save_at.set(None);
        let Some(db) = &self.db else { return };
        let Some(state) = self.owner.borrow_mut().changed_state() else { return };
        if let Err(error) = db.put_layout(&state) {
            eprintln!("kanna-daemon: couldn't save the layout: {error}");
            self.owner.borrow_mut().forget_saved();
        }
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.save_at.get()
    }
}

impl LayoutLink for DaemonLayout {
    fn commands(&self) -> &'static [&'static str] {
        Model::new().commands()
    }

    fn features(&self) -> &'static [&'static str] {
        Model::new().features()
    }

    fn call(&self, mut request: Value, panes: &Panes) -> LayoutCall {
        request["panes"] = pane_table(panes);
        let result: Result<Map, KmuxError> = self.owner.borrow_mut().serve(&request);
        self.touched.set(true);
        Box::pin(ready(result))
    }
}

pub fn saved(db: &Db, state_dir: &Path) -> Option<Value> {
    match db.layout() {
        Ok(Some(state)) => return Some(state),
        Ok(None) => {}
        Err(error) => {
            eprintln!("kanna-daemon: can't read the saved layout: {error}");
            return None;
        }
    }
    let path = state_dir.join("layout.json");
    let state = match StateFile::new(&path.to_string_lossy()).load() {
        Contents::State(state) => state,
        Contents::None => return None,
        Contents::Refused(why) => {
            eprintln!("kanna-daemon: not importing {}: {why}", path.display());
            return None;
        }
    };
    if let Err(error) = db.put_layout(&state) {
        eprintln!("kanna-daemon: can't import {} into kanna.db: {error}", path.display());
        return Some(state);
    }
    let to = state_dir.join("layout.json.imported");
    match std::fs::rename(&path, &to) {
        Ok(()) => eprintln!("kanna-daemon: imported the layout from {} into kanna.db; the file is now {}", path.display(), to.display()),
        Err(error) => eprintln!("kanna-daemon: imported the layout from {} into kanna.db, but can't rename it ({error})", path.display()),
    }
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kanna_mux::json::json;

    #[test]
    fn a_layout_json_is_imported_into_kanna_db_once() {
        let dir = std::env::temp_dir().join(format!("kanna-layout-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let state = json!({ "kmux": "state", "version": 3, "key": null, "windows": [], "workspaces": [], "panes": [], "counters": {} });
        StateFile::new(&dir.join("layout.json").to_string_lossy()).write(&state).unwrap();
        let db = Db::open(&dir);
        assert_eq!(saved(&db, &dir), Some(state.clone()));
        assert!(!dir.join("layout.json").exists() && dir.join("layout.json.imported").exists());
        assert_eq!(db.layout().unwrap(), Some(state.clone()));
        assert_eq!(saved(&db, &dir), Some(state));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
