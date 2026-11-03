use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub path: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub serial: Option<String>,
    pub product: Option<String>,
    pub vendor: Option<String>,
    pub location: Option<u32>,
}

impl Found {
    pub fn usb(&self) -> Option<String> {
        Some(format!("{:04x}:{:04x}", self.vid?, self.pid?))
    }

    pub fn identity(&self) -> Option<String> {
        let usb = self.usb()?;
        Some(match &self.serial {
            Some(serial) => format!("{usb}:{serial}"),
            None => match self.location {
                Some(location) => format!("{usb}@{location:x}"),
                None => usb,
            },
        })
    }

    pub fn name(&self) -> String {
        name_of(&self.path)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "name": self.name(),
            "usb": self.usb(),
            "serial": self.serial,
            "product": self.product,
            "vendor": self.vendor,
            "device": self.identity(),
        })
    }

    pub fn matches(&self, wanted: &str) -> bool {
        let wanted = wanted.trim();
        if wanted == self.path || wanted.trim_start_matches("/dev/") == self.path.trim_start_matches("/dev/") || wanted == self.name() {
            return true;
        }
        let Some(usb) = self.usb() else { return false };
        let wanted = wanted.to_ascii_lowercase();
        wanted == usb || self.identity().is_some_and(|id| id.to_ascii_lowercase() == wanted)
    }

    pub fn uart_bridge(&self) -> bool {
        matches!(self.vid, Some(0x303a | 0x10c4 | 0x1a86 | 0x0403 | 0x067b))
    }
}

pub fn name_of(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_prefix("cu.").or_else(|| file.strip_prefix("tty.")).unwrap_or(file).to_string()
}

pub fn fake_file() -> Option<PathBuf> {
    std::env::var_os("KANNA_SERIAL_PORTS").filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn list() -> Vec<Found> {
    match fake_file() {
        Some(file) => read_fake(&file),
        None => system(),
    }
}

fn read_fake(file: &std::path::Path) -> Vec<Found> {
    let Ok(text) = std::fs::read_to_string(file) else { return vec![] };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else { return vec![] };
    let hex = |item: &Value, key: &str| item[key].as_str().and_then(|v| u16::from_str_radix(v, 16).ok());
    let text = |item: &Value, key: &str| item[key].as_str().map(String::from);
    items
        .iter()
        .filter_map(|item| {
            Some(Found {
                path: item["path"].as_str()?.to_string(),
                vid: hex(item, "vid"),
                pid: hex(item, "pid"),
                serial: text(item, "serial"),
                product: text(item, "product"),
                vendor: text(item, "vendor"),
                location: item["location"].as_u64().map(|l| l as u32),
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn system() -> Vec<Found> {
    iokit::list()
}

#[cfg(not(target_os = "macos"))]
fn system() -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir("/dev") else { return vec![] };
    let mut found: Vec<Found> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("ttyUSB") || name.starts_with("ttyACM"))
        .map(|name| linux_details(&name))
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

#[cfg(not(target_os = "macos"))]
fn linux_details(name: &str) -> Found {
    let mut found = Found { path: format!("/dev/{name}"), ..Found::default() };
    let Ok(mut dir) = std::fs::canonicalize(format!("/sys/class/tty/{name}/device")) else { return found };
    let read = |dir: &std::path::Path, key: &str| std::fs::read_to_string(dir.join(key)).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    for _ in 0..4 {
        if let Some(vid) = read(&dir, "idVendor") {
            found.vid = u16::from_str_radix(&vid, 16).ok();
            found.pid = read(&dir, "idProduct").and_then(|p| u16::from_str_radix(&p, 16).ok());
            found.serial = read(&dir, "serial");
            found.product = read(&dir, "product");
            found.vendor = read(&dir, "manufacturer");
            break;
        }
        if !dir.pop() {
            break;
        }
    }
    found
}

pub fn watch(on_change: impl Fn() + Send + 'static) {
    match fake_file() {
        Some(file) => watch_file(file, on_change),
        None => watch_system(on_change),
    }
}

fn watch_file(file: PathBuf, on_change: impl Fn() + Send + 'static) {
    use notify::{RecursiveMode, Watcher};
    let folder = file.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let _ = std::thread::Builder::new().name("kanna.serial.fake".into()).spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(tx) else { return };
        if watcher.watch(&folder, RecursiveMode::NonRecursive).is_err() {
            return;
        }
        for event in rx {
            if event.is_ok_and(|e| e.paths.iter().any(|p| p.file_name() == file.file_name())) {
                on_change();
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn watch_system(on_change: impl Fn() + Send + 'static) {
    iokit::watch(Box::new(on_change));
}

#[cfg(not(target_os = "macos"))]
fn watch_system(on_change: impl Fn() + Send + 'static) {
    let _ = std::thread::Builder::new().name("kanna.serial.scan".into()).spawn(move || {
        let mut last = system();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let now = system();
            if now != last {
                last = now;
                on_change();
            }
        }
    });
}

#[cfg(target_os = "macos")]
mod iokit {
    use super::Found;
    use core_foundation_sys::base::{kCFAllocatorDefault, CFGetTypeID, CFRelease, CFTypeRef};
    use core_foundation_sys::number::{kCFNumberSInt64Type, CFNumberGetTypeID, CFNumberGetValue, CFNumberRef};
    use core_foundation_sys::runloop::{kCFRunLoopDefaultMode, CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRun, CFRunLoopSourceRef};
    use core_foundation_sys::string::{kCFStringEncodingUTF8, CFStringCreateWithCString, CFStringGetCString, CFStringGetTypeID, CFStringRef};
    use std::ffi::{c_char, c_void, CString};

    type IoObject = u32;
    type MutableDictionary = *mut c_void;
    type NotificationPort = *mut c_void;
    type Callback = extern "C" fn(*mut c_void, IoObject);

    const ITERATE: u32 = 1 | 2;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> MutableDictionary;
        fn IOServiceGetMatchingServices(main: u32, matching: MutableDictionary, iterator: *mut IoObject) -> i32;
        fn IOIteratorNext(iterator: IoObject) -> IoObject;
        fn IOObjectRelease(object: IoObject) -> i32;
        fn IORegistryEntryCreateCFProperty(entry: IoObject, key: CFStringRef, allocator: *const c_void, options: u32) -> CFTypeRef;
        fn IORegistryEntrySearchCFProperty(entry: IoObject, plane: *const c_char, key: CFStringRef, allocator: *const c_void, options: u32) -> CFTypeRef;
        fn IONotificationPortCreate(main: u32) -> NotificationPort;
        fn IONotificationPortGetRunLoopSource(port: NotificationPort) -> CFRunLoopSourceRef;
        fn IOServiceAddMatchingNotification(port: NotificationPort, kind: *const c_char, matching: MutableDictionary, callback: Callback, refcon: *mut c_void, iterator: *mut IoObject) -> i32;
    }

    fn key(name: &str) -> CFStringRef {
        let c = CString::new(name).unwrap();
        unsafe { CFStringCreateWithCString(kCFAllocatorDefault, c.as_ptr(), kCFStringEncodingUTF8) }
    }

    fn owned_string(value: CFTypeRef) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let out = if unsafe { CFGetTypeID(value) } == unsafe { CFStringGetTypeID() } {
            let mut buffer = [0 as c_char; 512];
            (unsafe { CFStringGetCString(value as CFStringRef, buffer.as_mut_ptr(), buffer.len() as _, kCFStringEncodingUTF8) } != 0)
                .then(|| unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().trim().to_string())
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        unsafe { CFRelease(value) };
        out
    }

    fn owned_number(value: CFTypeRef) -> Option<i64> {
        if value.is_null() {
            return None;
        }
        let mut n: i64 = 0;
        let ok = unsafe { CFGetTypeID(value) } == unsafe { CFNumberGetTypeID() } && unsafe { CFNumberGetValue(value as CFNumberRef, kCFNumberSInt64Type, (&mut n as *mut i64).cast()) };
        unsafe { CFRelease(value) };
        ok.then_some(n)
    }

    fn property(entry: IoObject, name: &str) -> CFTypeRef {
        let k = key(name);
        let value = unsafe { IORegistryEntryCreateCFProperty(entry, k, kCFAllocatorDefault as _, 0) };
        unsafe { CFRelease(k as CFTypeRef) };
        value
    }

    fn search(entry: IoObject, name: &str) -> CFTypeRef {
        let k = key(name);
        let plane = CString::new("IOService").unwrap();
        let value = unsafe { IORegistryEntrySearchCFProperty(entry, plane.as_ptr(), k, kCFAllocatorDefault as _, ITERATE) };
        unsafe { CFRelease(k as CFTypeRef) };
        value
    }

    fn describe(entry: IoObject) -> Option<Found> {
        let path = owned_string(property(entry, "IOCalloutDevice"))?;
        let vid = owned_number(search(entry, "idVendor")).map(|v| v as u16);
        let usb = vid.is_some();
        Some(Found {
            path,
            vid,
            pid: owned_number(search(entry, "idProduct")).map(|v| v as u16),
            serial: if usb { owned_string(search(entry, "USB Serial Number")).or_else(|| owned_string(search(entry, "kUSBSerialNumberString"))) } else { None },
            product: if usb { owned_string(search(entry, "USB Product Name")).or_else(|| owned_string(search(entry, "kUSBProductString"))) } else { None },
            vendor: if usb { owned_string(search(entry, "USB Vendor Name")).or_else(|| owned_string(search(entry, "kUSBVendorString"))) } else { None },
            location: if usb { owned_number(search(entry, "locationID")).map(|v| v as u32) } else { None },
        })
    }

    fn matching() -> MutableDictionary {
        let name = CString::new("IOSerialBSDClient").unwrap();
        unsafe { IOServiceMatching(name.as_ptr()) }
    }

    pub fn list() -> Vec<Found> {
        let mut iterator: IoObject = 0;
        if unsafe { IOServiceGetMatchingServices(0, matching(), &mut iterator) } != 0 {
            return vec![];
        }
        let mut out = Vec::new();
        loop {
            let entry = unsafe { IOIteratorNext(iterator) };
            if entry == 0 {
                break;
            }
            out.extend(describe(entry));
            unsafe { IOObjectRelease(entry) };
        }
        unsafe { IOObjectRelease(iterator) };
        out.retain(|f| f.path.starts_with("/dev/cu.") && f.name() != "Bluetooth-Incoming-Port" && f.name() != "debug-console");
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    fn drain(iterator: IoObject) {
        loop {
            let entry = unsafe { IOIteratorNext(iterator) };
            if entry == 0 {
                return;
            }
            unsafe { IOObjectRelease(entry) };
        }
    }

    extern "C" fn changed(refcon: *mut c_void, iterator: IoObject) {
        drain(iterator);
        let on_change = unsafe { &*(refcon as *const Box<dyn Fn() + Send>) };
        on_change();
    }

    pub fn watch(on_change: Box<dyn Fn() + Send>) {
        let _ = std::thread::Builder::new().name("kanna.serial.usb".into()).spawn(move || {
            let refcon = Box::into_raw(Box::new(on_change)) as *mut c_void;
            let port = unsafe { IONotificationPortCreate(0) };
            if port.is_null() {
                return;
            }
            unsafe { CFRunLoopAddSource(CFRunLoopGetCurrent(), IONotificationPortGetRunLoopSource(port), kCFRunLoopDefaultMode) };
            for kind in ["IOServiceFirstMatch", "IOServiceTerminate"] {
                let kind = CString::new(kind).unwrap();
                let mut iterator: IoObject = 0;
                if unsafe { IOServiceAddMatchingNotification(port, kind.as_ptr(), matching(), changed, refcon, &mut iterator) } == 0 {
                    drain(iterator);
                }
            }
            unsafe { CFRunLoopRun() };
        });
    }
}
