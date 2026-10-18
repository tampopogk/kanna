use std::ffi::{c_char, CString};
use std::ptr::null_mut;

fn give(text: String) -> *mut c_char {
    CString::new(text.replace('\0', "")).map_or(null_mut(), CString::into_raw)
}

#[no_mangle]
pub extern "C" fn kanna_layout_fraction(value: f64) -> *mut c_char {
    give(kanna_mux::fraction::format(value))
}

#[no_mangle]
pub extern "C" fn kanna_layout_free_string(string: *mut c_char) {
    if !string.is_null() {
        drop(unsafe { CString::from_raw(string) });
    }
}
