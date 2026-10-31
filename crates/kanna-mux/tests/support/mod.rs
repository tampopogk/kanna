#![allow(dead_code)]

use kanna_mux::json::{json, Value};
use kanna_mux::{pane_table, responder, Core, LayoutCall, LayoutLink, LayoutOwner, Model, Pane, PaneHost, PaneState, Panes, Responder, WeakCore};
use kanna_mux::LayoutHost;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

#[derive(Default)]
pub struct FakeHost {
    pub core: RefCell<Option<WeakCore>>,
    pub fail: RefCell<HashSet<String>>,
    pub stopped: RefCell<Vec<String>>,
    pub sent: RefCell<Vec<String>>,
    pub updated: RefCell<Vec<String>>,
}

impl PaneHost for FakeHost {
    fn start(&self, pane: &Pane) {
        let failing = self.fail.borrow().contains(pane.command.as_deref().unwrap_or(""));
        let Some(core) = self.core.borrow().as_ref().and_then(WeakCore::upgrade) else { return };
        let (id, reporter) = (pane.id.clone(), core.clone());
        core.later(move || {
            if failing {
                reporter.update(&id, PaneState::Failed, None, Some("command not found".into()));
            } else {
                reporter.update(&id, PaneState::Running, None, None);
            }
        });
    }

    fn stop(&self, pane: &Pane) {
        self.stopped.borrow_mut().push(pane.id.clone());
    }

    fn update(&self, pane: &Pane) {
        self.updated.borrow_mut().push(format!("{} markup={} agent={}", pane.id, pane.markup, pane.agent.as_deref().unwrap_or("-")));
    }

    fn send(&self, pane: &Pane, text: &str, done: Responder) {
        self.sent.borrow_mut().push(format!("{}: {text}", pane.id));
        done.reply(Ok(Default::default()));
    }
}

#[derive(Default)]
pub struct AppSide {
    pub core: RefCell<Option<WeakCore>>,
    pub owner: Rc<RefCell<LayoutOwner>>,
}

impl LayoutLink for AppSide {
    fn commands(&self) -> &'static [&'static str] {
        Model::new().commands()
    }

    fn features(&self) -> &'static [&'static str] {
        Model::new().features()
    }

    fn call(&self, mut request: Value, panes: &Panes) -> LayoutCall {
        request["panes"] = pane_table(panes);
        let wire = serde_json::to_string(&json!({ "call": "serve", "request": request })).unwrap();
        let (answer, reply) = responder();
        let owner = self.owner.clone();
        let core = self.core.borrow().as_ref().and_then(WeakCore::upgrade).expect("the core is gone");
        core.later(move || {
            let mut reply: Value = owner.borrow_mut().handle(&serde_json::from_str(&wire).unwrap());
            let reply: Value = serde_json::from_str(&serde_json::to_string(&reply.take()).unwrap()).unwrap();
            let Value::Object(mut fields) = reply else { unreachable!() };
            if fields.remove("ok") == Some(Value::Bool(true)) {
                answer.reply(Ok(fields));
            } else {
                let error = fields.remove("error").unwrap_or(Value::Null);
                let text = |key: &str| error[key].as_str().unwrap_or_default().to_string();
                answer.reply(Err(kanna_mux::KmuxError::new(text("code"), text("message"))));
            }
        });
        Box::pin(reply)
    }
}

pub fn make_remote_core() -> (Core, Rc<FakeHost>, Rc<AppSide>) {
    let app = Rc::new(AppSide::default());
    let core = Core::with_link(app.clone());
    *app.core.borrow_mut() = Some(core.weak());
    let host = Rc::new(FakeHost::default());
    *host.core.borrow_mut() = Some(core.weak());
    core.set_host(Some(host.clone()));
    (core, host, app)
}

pub fn make_core() -> (Core, Rc<FakeHost>) {
    let core = Core::default();
    let host = Rc::new(FakeHost::default());
    *host.core.borrow_mut() = Some(core.weak());
    core.set_host(Some(host.clone()));
    (core, host)
}

pub fn send(core: &Core, request: Value) -> Value {
    core.handle_blocking(request)
}
