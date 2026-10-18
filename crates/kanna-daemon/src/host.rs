use crate::app_link::AppLink;
use crate::terminals::Terminals;
use kanna_mux::{KmuxError, Pane, PaneHost, PaneType, Responder};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

pub struct Host {
    terminals: Rc<Terminals>,
    app: RefCell<Weak<AppLink>>,
}

impl Host {
    pub fn new(terminals: Rc<Terminals>) -> Host {
        Host { terminals, app: RefCell::new(Weak::new()) }
    }

    pub fn connect(&self, app: &Rc<AppLink>) {
        *self.app.borrow_mut() = Rc::downgrade(app);
    }

    fn app(&self) -> Option<Rc<AppLink>> {
        self.app.borrow().upgrade()
    }
}

impl PaneHost for Host {
    fn start(&self, pane: &Pane) {
        match pane.kind {
            PaneType::Term => self.terminals.start(pane),
            _ => match self.app().filter(|app| app.attached()) {
                Some(app) => app.show(pane),
                None => self.terminals.unshown(pane),
            },
        }
    }

    fn stop(&self, pane: &Pane) {
        match pane.kind {
            PaneType::Term => self.terminals.stop(pane),
            _ => {
                if let Some(app) = self.app() {
                    app.hide(&pane.id);
                }
            }
        }
    }

    fn end(&self, pane: &Pane) {
        if pane.kind == PaneType::Term {
            self.terminals.end(pane);
        }
    }

    fn send(&self, pane: &Pane, text: &str, done: Responder) {
        self.terminals.send(pane, text, done);
    }

    fn send_keys(&self, pane: &Pane, keys: Vec<String>, done: Responder) {
        self.terminals.send_keys(pane, keys, done);
    }

    fn read(&self, pane: &Pane, lines: Option<usize>) -> Result<String, KmuxError> {
        self.terminals.read(pane, lines)
    }
}
