use kanna_mux::json::json;
use kanna_mux::Core;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn a_request_never_overtakes_what_is_queued() {
    let core = Core::default();
    let log = Rc::new(RefCell::new(vec![]));
    let queued = log.clone();
    core.later(move || queued.borrow_mut().push("queued"));
    let asked = log.clone();
    core.handle(json!({ "cmd": "list" }), move |_| asked.borrow_mut().push("request"));
    assert!(log.borrow().is_empty(), "nothing runs until the runtime does");
    core.turn();
    assert_eq!(*log.borrow(), vec!["queued", "request"]);
}

#[test]
fn a_timed_wait_runs_on_core_s_runtime_outside_any_loop() {
    let core = Core::default();
    assert_eq!(core.handle_blocking(json!({ "cmd": "list" }))["ok"], true);
    core.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(1)).await });
    assert_eq!(core.handle_blocking(json!({ "cmd": "list" }))["ok"], true);
}
