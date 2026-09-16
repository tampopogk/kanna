//! macOS discovery goes through libSystem's DNS-SD broker. Raw multicast
//! sockets can fail on every physical interface while still discovering self
//! on loopback. The system responder owns publication and address changes.
use super::{endpoint_rank, MdnsState};
use crate::discovery::{decode_txt_record, encode_txt_record, TxtPeerRecord, SERVICE_TYPE};
use crate::protocol::{PeerRegistryEntry, CURRENT_PROTOCOL_VERSION};
use crate::runtime::RuntimeError;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::ptr;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

type Ref = *mut c_void;
const ADD: u32 = 2;
const SHARE_CONNECTION: u32 = 0x4000;

type RegisterReply =
    unsafe extern "C" fn(Ref, u32, i32, *const c_char, *const c_char, *const c_char, *mut c_void);
type BrowseReply = unsafe extern "C" fn(
    Ref,
    u32,
    u32,
    i32,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut c_void,
);
type ResolveReply = unsafe extern "C" fn(
    Ref,
    u32,
    u32,
    i32,
    *const c_char,
    *const c_char,
    u16,
    u16,
    *const u8,
    *mut c_void,
);
type AddressReply = unsafe extern "C" fn(
    Ref,
    u32,
    u32,
    i32,
    *const c_char,
    *const libc::sockaddr,
    u32,
    *mut c_void,
);

// DNS-SD is provided by macOS libSystem, with no build-machine dependency.
unsafe extern "C" {
    fn DNSServiceCreateConnection(reference: *mut Ref) -> i32;
    fn DNSServiceRegister(
        reference: *mut Ref,
        flags: u32,
        interface: u32,
        name: *const c_char,
        regtype: *const c_char,
        domain: *const c_char,
        host: *const c_char,
        port: u16,
        txt_len: u16,
        txt: *const c_void,
        callback: RegisterReply,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceBrowse(
        reference: *mut Ref,
        flags: u32,
        interface: u32,
        regtype: *const c_char,
        domain: *const c_char,
        callback: BrowseReply,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceResolve(
        reference: *mut Ref,
        flags: u32,
        interface: u32,
        name: *const c_char,
        regtype: *const c_char,
        domain: *const c_char,
        callback: ResolveReply,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceGetAddrInfo(
        reference: *mut Ref,
        flags: u32,
        interface: u32,
        protocols: u32,
        host: *const c_char,
        callback: AddressReply,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceRefSockFD(reference: Ref) -> i32;
    fn DNSServiceProcessResult(reference: Ref) -> i32;
    fn DNSServiceRefDeallocate(reference: Ref);
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Key {
    name: String,
    regtype: String,
    domain: String,
    interface: u32,
}
impl Key {
    fn observation(&self) -> String {
        format!(
            "{}.{}{}@{}",
            self.name, self.regtype, self.domain, self.interface
        )
    }
}
enum Event {
    Browse {
        key: Key,
        added: bool,
    },
    Resolved {
        key: Key,
        generation: u64,
        record: Result<(TxtPeerRecord, String, u16), String>,
    },
    Address {
        key: Key,
        generation: u64,
        address: Result<SocketAddr, String>,
        added: bool,
    },
    Failed(String),
}
struct Context {
    sender: mpsc::Sender<Event>,
    key: Option<Key>,
    generation: u64,
    port: u16,
}

// Every callback runs synchronously inside DNSServiceProcessResult on the
// owning thread. NativeRef deallocates the operation before freeing context.
unsafe fn context<'a>(raw: *mut c_void) -> &'a Context {
    unsafe { &*raw.cast::<Context>() }
}
unsafe fn string(raw: *const c_char) -> String {
    unsafe { CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned()
}
fn error(code: i32) -> String {
    format!("macOS transfer DNS-SD error {code}")
}
unsafe extern "C" fn registered(
    _: Ref,
    _: u32,
    code: i32,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
    raw: *mut c_void,
) {
    if code != 0 {
        let _ = unsafe { context(raw) }
            .sender
            .send(Event::Failed(error(code)));
    }
}
unsafe extern "C" fn browsed(
    _: Ref,
    flags: u32,
    interface: u32,
    code: i32,
    name: *const c_char,
    regtype: *const c_char,
    domain: *const c_char,
    raw: *mut c_void,
) {
    let ctx = unsafe { context(raw) };
    let event = if code != 0 {
        Event::Failed(error(code))
    } else {
        Event::Browse {
            key: Key {
                name: unsafe { string(name) },
                regtype: unsafe { string(regtype) },
                domain: unsafe { string(domain) },
                interface,
            },
            added: flags & ADD != 0,
        }
    };
    let _ = ctx.sender.send(event);
}
unsafe extern "C" fn resolved(
    _: Ref,
    _: u32,
    _: u32,
    code: i32,
    _: *const c_char,
    host: *const c_char,
    port: u16,
    length: u16,
    txt: *const u8,
    raw: *mut c_void,
) {
    let ctx = unsafe { context(raw) };
    let record = if code != 0 {
        Err(error(code))
    } else if txt.is_null() || host.is_null() {
        Err("transfer DNS-SD resolution missing TXT or host".into())
    } else {
        parse_txt(unsafe { std::slice::from_raw_parts(txt, length as usize) })
            .and_then(|txt| decode_txt_record(&txt).map_err(|e| e.to_string()))
            .map(|record| (record, unsafe { string(host) }, u16::from_be(port)))
    };
    let _ = ctx.sender.send(Event::Resolved {
        key: ctx.key.clone().unwrap(),
        generation: ctx.generation,
        record,
    });
}
unsafe extern "C" fn addressed(
    _: Ref,
    flags: u32,
    interface: u32,
    code: i32,
    _: *const c_char,
    addr: *const libc::sockaddr,
    _: u32,
    raw: *mut c_void,
) {
    let ctx = unsafe { context(raw) };
    let address = if code != 0 {
        Err(error(code))
    } else {
        unsafe { socket_address(addr, ctx.port, interface) }
            .ok_or_else(|| "transfer DNS-SD returned an invalid address".to_owned())
    };
    let _ = ctx.sender.send(Event::Address {
        key: ctx.key.clone().unwrap(),
        generation: ctx.generation,
        address,
        added: flags & ADD != 0,
    });
}
unsafe fn socket_address(
    addr: *const libc::sockaddr,
    port: u16,
    interface: u32,
) -> Option<SocketAddr> {
    if addr.is_null() {
        return None;
    }
    match unsafe { (*addr).sa_family as i32 } {
        libc::AF_INET => {
            let addr = unsafe { &*addr.cast::<libc::sockaddr_in>() };
            Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes())),
                port,
            ))
        }
        libc::AF_INET6 => {
            let addr = unsafe { &*addr.cast::<libc::sockaddr_in6>() };
            let ip = Ipv6Addr::from(addr.sin6_addr.s6_addr);
            let scope = if ip.is_unicast_link_local() {
                interface
            } else {
                0
            };
            Some(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, scope)))
        }
        _ => None,
    }
}
fn parse_txt(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let mut txt = BTreeMap::new();
    let mut remaining = bytes;
    while let Some((&length, tail)) = remaining.split_first() {
        let entry = tail
            .get(..length as usize)
            .ok_or("truncated transfer TXT record")?;
        let entry = std::str::from_utf8(entry).map_err(|e| e.to_string())?;
        if let Some((key, value)) = entry.split_once('=') {
            txt.insert(key.to_owned(), value.to_owned());
        }
        remaining = &tail[length as usize..];
    }
    Ok(txt)
}
struct NativeRef {
    raw: Ref,
    _context: Box<Context>,
}
impl Drop for NativeRef {
    fn drop(&mut self) {
        unsafe { DNSServiceRefDeallocate(self.raw) };
    }
}
fn operation(
    shared: Ref,
    ctx: Context,
    start: impl FnOnce(*mut Ref, *mut c_void) -> i32,
) -> Result<NativeRef, String> {
    let mut ctx = Box::new(ctx);
    let mut raw = shared;
    let code = start(&mut raw, (&mut *ctx as *mut Context).cast());
    if code != 0 {
        return Err(error(code));
    }
    Ok(NativeRef { raw, _context: ctx })
}
struct Connection(Ref);
impl Drop for Connection {
    fn drop(&mut self) {
        unsafe { DNSServiceRefDeallocate(self.0) };
    }
}
struct Observation {
    generation: u64,
    address_generation: u64,
    _resolve: NativeRef,
    address: Option<NativeRef>,
    record: Option<TxtPeerRecord>,
    addresses: BTreeSet<SocketAddr>,
}

pub(in crate::runtime) struct MdnsDiscovery {
    state: Arc<tokio::sync::Mutex<MdnsState>>,
    stop: Mutex<UnixStream>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl MdnsDiscovery {
    pub(in crate::runtime) async fn spawn(
        peer_id: &str,
        display_name: &str,
        public_key: &str,
        port: u16,
    ) -> Result<Self, RuntimeError> {
        let txt = encode_txt_record(
            peer_id,
            display_name,
            public_key,
            CURRENT_PROTOCOL_VERSION,
            true,
        )
        .map_err(|e| RuntimeError::InvalidConfig(e.to_string()))?;
        let mut wire = Vec::new();
        for (key, value) in txt {
            let entry = format!("{key}={value}");
            wire.push(entry.len() as u8);
            wire.extend_from_slice(entry.as_bytes());
        }
        let name = CString::new(peer_id).map_err(|e| RuntimeError::InvalidConfig(e.to_string()))?;
        let (stop, cancelled) = UnixStream::pair()?;
        let state = Arc::new(tokio::sync::Mutex::new(MdnsState::default()));
        let worker_state = Arc::clone(&state);
        let (ready, startup) = tokio::sync::oneshot::channel();
        let worker = std::thread::Builder::new()
            .name("kanna-transfer-dns-sd".into())
            .spawn(move || {
                let result = run(name, wire, port, cancelled, &worker_state, ready);
                if let Err(error) = result {
                    eprintln!("[transfer-discovery] {error}");
                }
                worker_state.blocking_lock().observations.clear();
            })?;
        let discovery = Self {
            state,
            stop: Mutex::new(stop),
            worker: Mutex::new(Some(worker)),
        };
        startup
            .await
            .map_err(|_| {
                RuntimeError::Discovery("macOS transfer DNS-SD stopped during startup".into())
            })?
            .map_err(RuntimeError::Discovery)?;
        Ok(discovery)
    }
    pub(in crate::runtime) async fn list_peers(
        &self,
        self_peer_id: &str,
    ) -> Result<Vec<PeerRegistryEntry>, RuntimeError> {
        Ok(self.state.lock().await.list_peers(self_peer_id))
    }
    pub(in crate::runtime) fn shutdown(&self) {
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = self
                .stop
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .write_all(&[1]);
            let _ = worker.join();
        }
    }
}
impl Drop for MdnsDiscovery {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(
    name: CString,
    txt: Vec<u8>,
    port: u16,
    stop: UnixStream,
    state: &Arc<tokio::sync::Mutex<MdnsState>>,
    ready: tokio::sync::oneshot::Sender<Result<(), String>>,
) -> Result<(), String> {
    let mut raw = ptr::null_mut();
    let code = unsafe { DNSServiceCreateConnection(&mut raw) };
    if code != 0 {
        let message = error(code);
        let _ = ready.send(Err(message.clone()));
        return Err(message);
    }
    // Children are declared after the connection and drop before it.
    let connection = Connection(raw);
    let (sender, events) = mpsc::channel();
    let ctx = || Context {
        sender: sender.clone(),
        key: None,
        generation: 0,
        port: 0,
    };
    let regtype = CString::new(SERVICE_TYPE.trim_end_matches("local.")).unwrap();
    let _registration = operation(raw, ctx(), |r, c| unsafe {
        DNSServiceRegister(
            r,
            SHARE_CONNECTION,
            0,
            name.as_ptr(),
            regtype.as_ptr(),
            ptr::null(),
            ptr::null(),
            port.to_be(),
            txt.len() as u16,
            txt.as_ptr().cast(),
            registered,
            c,
        )
    })?;
    let _browse = operation(raw, ctx(), |r, c| unsafe {
        DNSServiceBrowse(
            r,
            SHARE_CONNECTION,
            0,
            regtype.as_ptr(),
            ptr::null(),
            browsed,
            c,
        )
    })?;
    let fd = unsafe { DNSServiceRefSockFD(connection.0) };
    if fd < 0 {
        return Err("macOS transfer DNS-SD returned no socket".into());
    }
    let mut observations = HashMap::<Key, Observation>::new();
    let mut generation = 0;
    let _ = ready.send(Ok(()));
    loop {
        // Wait on native events or explicit shutdown; no discovery polling or retries.
        let mut fds = [
            libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: stop.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let polled = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if polled < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.to_string());
        }
        if fds[1].revents != 0 {
            return Ok(());
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err("macOS transfer DNS-SD connection closed".into());
        }
        if fds[0].revents & libc::POLLIN == 0 {
            continue;
        }
        let code = unsafe { DNSServiceProcessResult(connection.0) };
        if code != 0 {
            return Err(error(code));
        }
        while let Ok(event) = events.try_recv() {
            match event {
                Event::Failed(error) => return Err(error),
                Event::Browse { key, added } => {
                    if !added {
                        observations.remove(&key);
                        state
                            .blocking_lock()
                            .observations
                            .remove(&key.observation());
                        continue;
                    }
                    if observations.contains_key(&key) {
                        continue;
                    }
                    generation += 1;
                    let ctx = Context {
                        sender: sender.clone(),
                        key: Some(key.clone()),
                        generation,
                        port: 0,
                    };
                    let name = CString::new(key.name.as_str()).map_err(|e| e.to_string())?;
                    let regtype = CString::new(key.regtype.as_str()).map_err(|e| e.to_string())?;
                    let domain = CString::new(key.domain.as_str()).map_err(|e| e.to_string())?;
                    let resolve = operation(raw, ctx, |r, c| unsafe {
                        DNSServiceResolve(
                            r,
                            SHARE_CONNECTION,
                            key.interface,
                            name.as_ptr(),
                            regtype.as_ptr(),
                            domain.as_ptr(),
                            resolved,
                            c,
                        )
                    });
                    match resolve {
                        Ok(resolve) => {
                            observations.insert(
                                key,
                                Observation {
                                    generation,
                                    address_generation: 0,
                                    _resolve: resolve,
                                    address: None,
                                    record: None,
                                    addresses: BTreeSet::new(),
                                },
                            );
                        }
                        Err(error) => eprintln!(
                            "[transfer-discovery] resolve {}: {error}",
                            key.observation()
                        ),
                    }
                }
                Event::Resolved {
                    key,
                    generation: resolved_generation,
                    record,
                } => {
                    let Some(observation) = observations
                        .get_mut(&key)
                        .filter(|o| o.generation == resolved_generation)
                    else {
                        continue;
                    };
                    generation += 1;
                    observation.address_generation = generation;
                    observation.address = None;
                    observation.addresses.clear();
                    observation.record = None;
                    state
                        .blocking_lock()
                        .observations
                        .remove(&key.observation());
                    match record {
                        Ok((record, host, port)) => {
                            let host = CString::new(host).map_err(|e| e.to_string())?;
                            let ctx = Context {
                                sender: sender.clone(),
                                key: Some(key.clone()),
                                generation: observation.address_generation,
                                port,
                            };
                            match operation(raw, ctx, |r, c| unsafe {
                                DNSServiceGetAddrInfo(
                                    r,
                                    SHARE_CONNECTION,
                                    key.interface,
                                    0,
                                    host.as_ptr(),
                                    addressed,
                                    c,
                                )
                            }) {
                                Ok(address) => {
                                    observation.address = Some(address);
                                    observation.record = Some(record);
                                }
                                Err(error) => eprintln!(
                                    "[transfer-discovery] address {}: {error}",
                                    key.observation()
                                ),
                            }
                        }
                        Err(error) => eprintln!(
                            "[transfer-discovery] rejected {}: {error}",
                            key.observation()
                        ),
                    }
                }
                Event::Address {
                    key,
                    generation,
                    address,
                    added,
                } => {
                    let Some(observation) = observations
                        .get_mut(&key)
                        .filter(|o| o.address_generation == generation)
                    else {
                        continue;
                    };
                    match address {
                        Ok(address) if !address.ip().is_unspecified() => {
                            if added {
                                observation.addresses.insert(address);
                            } else {
                                observation.addresses.remove(&address);
                            }
                        }
                        Ok(_) => continue,
                        Err(error) => {
                            observation.addresses.clear();
                            eprintln!(
                                "[transfer-discovery] address {}: {error}",
                                key.observation()
                            );
                        }
                    }
                    let endpoint = observation
                        .addresses
                        .iter()
                        .min_by_key(|a| (endpoint_rank(&a.to_string()), **a));
                    let mut state = state.blocking_lock();
                    if let (Some(endpoint), Some(record)) = (endpoint, &observation.record) {
                        state.observations.insert(
                            key.observation(),
                            PeerRegistryEntry {
                                peer_id: record.peer_id.clone(),
                                display_name: record.display_name.clone(),
                                endpoint: endpoint.to_string(),
                                pid: 0,
                                public_key: record.public_key.clone(),
                                protocol_version: record.protocol_version,
                                accepting_transfers: record.accepting_transfers,
                            },
                        );
                    } else {
                        state.observations.remove(&key.observation());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_txt_callback_consumes_the_advertisers_record_and_rejects_truncation() {
        let txt = encode_txt_record("peer-native", "Native Mac", "key", 5, true).unwrap();
        let mut wire = Vec::new();
        for (key, value) in &txt {
            let entry = format!("{key}={value}");
            wire.push(entry.len() as u8);
            wire.extend_from_slice(entry.as_bytes());
        }
        let (sender, events) = mpsc::channel();
        let mut ctx = Context {
            sender,
            key: Some(Key {
                name: "peer-native".into(),
                regtype: "_kanna-xfer._tcp.".into(),
                domain: "local.".into(),
                interface: 7,
            }),
            generation: 9,
            port: 0,
        };
        let host = CString::new("native.local.").unwrap();
        unsafe {
            resolved(
                ptr::null_mut(),
                0,
                7,
                0,
                ptr::null(),
                host.as_ptr(),
                4455u16.to_be(),
                wire.len() as u16,
                wire.as_ptr(),
                (&mut ctx as *mut Context).cast(),
            );
        }
        let Event::Resolved {
            record: Ok((record, host, port)),
            generation,
            ..
        } = events.recv().unwrap()
        else {
            panic!("native resolve did not decode producer TXT");
        };
        assert_eq!(record, decode_txt_record(&txt).unwrap());
        assert_eq!(
            (host.as_str(), port, generation),
            ("native.local.", 4455, 9)
        );
        wire.pop();
        assert!(parse_txt(&wire).is_err());
        // Error callbacks have undefined payload pointers and interface ids.
        unsafe {
            resolved(
                ptr::null_mut(),
                0,
                0,
                -65537,
                ptr::null(),
                ptr::null(),
                0,
                0,
                ptr::null(),
                (&mut ctx as *mut Context).cast(),
            );
        }
        assert!(matches!(
            events.recv().unwrap(),
            Event::Resolved {
                generation: 9,
                record: Err(_),
                ..
            }
        ));
    }
}
