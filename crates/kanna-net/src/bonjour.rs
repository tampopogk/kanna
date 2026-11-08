pub const SERVICE: &str = "_kanna-phone._udp";

#[cfg(target_os = "macos")]
mod system {
    use super::SERVICE;
    use std::ffi::{c_char, c_void, CString};

    #[link(name = "System")]
    extern "C" {
        fn DNSServiceRegister(reference: *mut *mut c_void, flags: u32, interface: u32, name: *const c_char, kind: *const c_char, domain: *const c_char, host: *const c_char, port: u16, txt_len: u16, txt: *const c_void, callback: *const c_void, context: *mut c_void) -> i32;
        fn DNSServiceRefDeallocate(reference: *mut c_void);
    }

    pub struct Published(*mut c_void);

    unsafe impl Send for Published {}

    pub fn publish(name: &str, port: u16) -> Option<Published> {
        let (name, kind) = (CString::new(name).ok()?, CString::new(SERVICE).ok()?);
        let mut reference = std::ptr::null_mut();
        let code = unsafe { DNSServiceRegister(&mut reference, 0, 0, name.as_ptr(), kind.as_ptr(), std::ptr::null(), std::ptr::null(), port.to_be(), 0, std::ptr::null(), std::ptr::null(), std::ptr::null_mut()) };
        (code == 0).then_some(Published(reference))
    }

    impl Drop for Published {
        fn drop(&mut self) {
            unsafe { DNSServiceRefDeallocate(self.0) }
        }
    }
}

#[cfg(target_os = "linux")]
mod system {
    use super::SERVICE;
    use mdns_sd::{ServiceDaemon, ServiceInfo};

    pub struct Published(ServiceDaemon);

    pub fn publish(name: &str, port: u16) -> Option<Published> {
        let daemon = ServiceDaemon::new().ok()?;
        let host = format!("{name}.local.");
        let info = ServiceInfo::new(&format!("{SERVICE}.local."), name, &host, "", port, None::<std::collections::HashMap<String, String>>).ok()?.enable_addr_auto();
        daemon.register(info).ok()?;
        Some(Published(daemon))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod system {
    pub struct Published;

    pub fn publish(_name: &str, _port: u16) -> Option<Published> {
        None
    }
}

pub use system::{publish, Published};
