use kanna_mux::json::{Map, Value};
use kanna_mux::{pane_table, read_pane_table, KmuxError, LayoutCall, LayoutHost, LayoutLink, LayoutOwner, Model, PaneState, PaneType, Panes, StateFile, WeakCore};
use std::cell::{Cell, RefCell};
use std::future::ready;
use std::path::Path;
use std::time::{Duration, Instant};

const SAVE_AFTER: Duration = Duration::from_millis(500);

pub struct DaemonLayout {
    owner: RefCell<LayoutOwner>,
    core: RefCell<Option<WeakCore>>,
    panes_changed: Cell<bool>,
    touched: Cell<bool>,
    save_at: Cell<Option<Instant>>,
    shown: RefCell<Option<Value>>,
    pub on_view: RefCell<Option<Box<dyn Fn(&Value)>>>,
}

impl DaemonLayout {
    pub fn open(state_dir: &Path) -> DaemonLayout {
        let flag = |name: &str| std::env::var(name).is_ok_and(|v| v == "1");
        let file = (!flag("KANNA_NO_STATE")).then(|| StateFile::new(&state_dir.join("layout.json").to_string_lossy()));
        let (owner, problem) = LayoutOwner::open(file, flag("KANNA_FRESH"));
        if let Some(problem) = problem {
            eprintln!("kanna-daemon: {problem}");
        }
        DaemonLayout {
            owner: RefCell::new(owner),
            core: RefCell::new(None),
            panes_changed: Cell::new(false),
            touched: Cell::new(false),
            save_at: Cell::new(None),
            shown: RefCell::new(None),
            on_view: RefCell::new(None),
        }
    }

    pub fn connect(&self, core: WeakCore) {
        *self.core.borrow_mut() = Some(core);
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

    pub fn call(&self, request: &Value) -> Value {
        let reply = self.owner.borrow_mut().handle(request);
        self.touched.set(true);
        reply
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
        if let Err(error) = self.owner.borrow_mut().save() {
            eprintln!("kanna-daemon: couldn't save the layout: {}", error.message);
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
