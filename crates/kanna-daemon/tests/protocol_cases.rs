mod common;
#[path = "../../kanna-mux/tests/cases/mod.rs"]
mod cases;

use common::Instance;

#[test]
fn kmux_cases_pass_through_the_daemon_and_the_app_link() {
    let passed = cases::run_cases("daemon and app link", &|| {
        let instance = Instance::start();
        instance.app().answering();
        Box::new(move |request| common::request(&instance.socket(), request))
    });
    assert_eq!(passed, 35);
}
