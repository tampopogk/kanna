mod cases;
mod support;

use kanna_mux::Core;
use support::{make_core, make_remote_core};

fn sender(core: Core) -> cases::Send {
    core.use_content_ids();
    Box::new(move |request| core.handle_blocking(request))
}

#[test]
fn shared_protocol_cases() {
    assert_eq!(cases::run_cases("native", &|| sender(make_core().0)), 35);
}

#[test]
fn shared_protocol_cases_with_the_layout_in_the_app() {
    assert_eq!(cases::run_cases("layout in the app", &|| sender(make_remote_core().0)), 35);
}
