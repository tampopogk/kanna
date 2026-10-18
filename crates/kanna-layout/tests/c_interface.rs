use kanna_layout::{kanna_layout_fraction, kanna_layout_free_string};
use std::ffi::CStr;

#[test]
fn fractions_are_shown_as_kmux_shows_them() {
    for (value, text) in [(0.25, "1/4"), (2.0 / 3.0, "2/3"), (0.5, "1/2")] {
        let reply = kanna_layout_fraction(value);
        assert_eq!(unsafe { CStr::from_ptr(reply) }.to_str().unwrap(), text);
        kanna_layout_free_string(reply);
    }
}
