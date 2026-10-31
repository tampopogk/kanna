use crate::error::KmuxError;
use crate::json::{encode, json, Value};
use std::io::{BufRead, BufReader};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub type Handler = Arc<dyn Fn(Value, Box<dyn FnOnce(Value) + Send>) + Send + Sync>;

pub type Takeover = Arc<dyn Fn(Value, BufReader<UnixStream>, UnixStream) + Send + Sync>;

pub struct SocketServer {
    pub path: String,
    handler: Handler,
    takeover: Option<(String, Takeover)>,
    limits: Limits,
    running: Mutex<Option<Running>>,
}

#[derive(Clone, Copy)]
struct Limits {
    connections: usize,
    line: usize,
    idle: Duration,
}

const LIMITS: Limits = Limits { connections: 64, line: 16 << 20, idle: Duration::from_secs(600) };

struct Running {
    identity: Option<(u64, u64)>,
    listener: UnixListener,
    wake: UnixStream,
    thread: JoinHandle<()>,
}

impl SocketServer {
    pub fn new(path: &str, handler: Handler) -> SocketServer {
        SocketServer { path: path.into(), handler, takeover: None, limits: LIMITS, running: Mutex::new(None) }
    }

    pub fn take_over(&mut self, cmd: &str, takeover: Takeover) {
        self.takeover = Some((cmd.into(), takeover));
    }

    pub fn start(&self) -> Result<(), KmuxError> {
        let mut running = self.running.lock().unwrap();
        if running.is_some() {
            return Err(KmuxError::new("bad_request", format!("already listening on {}", self.path)));
        }
        let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if self.path.len() >= address.sun_path.len() {
            return Err(KmuxError::new("bad_request", format!("socket path is too long: {}", self.path)));
        }
        if let Some(folder) = Path::new(&self.path).parent() {
            std::fs::create_dir_all(folder).map_err(|e| self.posix_error("mkdir", &e))?;
        }
        if SocketServer::is_listening(&self.path) {
            return Err(KmuxError::new("bad_request", format!("another kmux is already listening on {}", self.path)));
        }
        let _ = std::fs::remove_file(&self.path);
        let listener = UnixListener::bind(&self.path).map_err(|e| self.posix_error("bind", &e))?;
        let identity = identity_of(&self.path);
        let unbind = |error: KmuxError| {
            if identity_of(&self.path) == identity {
                let _ = std::fs::remove_file(&self.path);
            }
            error
        };
        let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        *running = Some(self.serve(listener, identity).map_err(unbind)?);
        Ok(())
    }

    pub fn start_with(&self, listener: UnixListener) -> Result<(), KmuxError> {
        let mut running = self.running.lock().unwrap();
        if running.is_some() {
            return Err(KmuxError::new("bad_request", format!("already listening on {}", self.path)));
        }
        *running = Some(self.serve(listener, identity_of(&self.path))?);
        Ok(())
    }

    fn serve(&self, listener: UnixListener, identity: Option<(u64, u64)>) -> Result<Running, KmuxError> {
        listener.set_nonblocking(true).map_err(|e| self.posix_error("fcntl", &e))?;
        let copy = listener.try_clone().map_err(|e| self.posix_error("dup", &e))?;
        let (wake, woken) = UnixStream::pair().map_err(|e| self.posix_error("socketpair", &e))?;
        let handler = self.handler.clone();
        let takeover = self.takeover.clone();
        let limits = self.limits;
        let thread = std::thread::Builder::new()
            .name("kmux.socket".into())
            .spawn(move || accept_loop(listener, woken, handler, takeover, limits))
            .map_err(|e| self.posix_error("thread", &e))?;
        Ok(Running { identity, listener: copy, wake, thread })
    }

    pub fn listener(&self) -> Option<OwnedFd> {
        let running = self.running.lock().unwrap();
        running.as_ref()?.listener.try_clone().ok().map(OwnedFd::from)
    }

    pub fn stop(&self) {
        self.shut(true);
    }

    pub fn stop_leaving_path(&self) {
        self.shut(false);
    }

    fn shut(&self, unlink: bool) {
        let Some(running) = self.running.lock().unwrap().take() else { return };
        let _ = running.wake.shutdown(Shutdown::Both);
        drop(running.wake);
        let _ = running.thread.join();
        if unlink && running.identity.is_some() && identity_of(&self.path) == running.identity {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    pub fn is_listening(path: &str) -> bool {
        UnixStream::connect(path).is_ok()
    }

    fn posix_error(&self, call: &str, error: &std::io::Error) -> KmuxError {
        KmuxError::new("bad_request", format!("{call} failed on {}: {}", self.path, crate::state_file::os_message(error)))
    }
}

fn identity_of(path: &str) -> Option<(u64, u64)> {
    std::fs::symlink_metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

impl Drop for SocketServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(listener: UnixListener, woken: UnixStream, handler: Handler, takeover: Option<(String, Takeover)>, limits: Limits) {
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        let full = active.load(Ordering::SeqCst) >= limits.connections;
        let mut fds = [
            libc::pollfd { fd: woken.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        let (count, timeout) = if full { (1, 50) } else { (2, -1) };
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), count as libc::nfds_t, timeout) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if fds[0].revents != 0 {
            return;
        }
        while active.load(Ordering::SeqCst) < limits.connections {
            let client = match listener.accept() {
                Ok((client, _)) => client,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(50));
                    break;
                }
            };
            if client.set_nonblocking(false).is_err() || client.set_read_timeout(Some(limits.idle)).is_err() {
                continue;
            }
            no_sigpipe(&client);
            let slot = Slot::take(&active);
            let (handler, takeover) = (handler.clone(), takeover.clone());
            let _ = std::thread::Builder::new().name("kmux.client".into()).spawn(move || {
                let _slot = slot;
                serve(client, handler, takeover, limits.line)
            });
        }
    }
}

struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(active: &Arc<AtomicUsize>) -> Slot {
        active.fetch_add(1, Ordering::SeqCst);
        Slot(active.clone())
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

enum Line {
    Complete,
    End,
    TooLong,
}

fn read_line(reader: &mut BufReader<UnixStream>, line: &mut Vec<u8>, limit: usize) -> Line {
    loop {
        let buffer = match reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Line::End,
        };
        if buffer.is_empty() {
            return Line::End;
        }
        let (taken, complete) = match buffer.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buffer.len(), false),
        };
        if line.len() + taken > limit + 1 {
            return Line::TooLong;
        }
        line.extend_from_slice(&buffer[..taken]);
        reader.consume(taken);
        if complete {
            return Line::Complete;
        }
    }
}

fn serve(client: UnixStream, handler: Handler, takeover: Option<(String, Takeover)>, limit: usize) {
    let Ok(writer) = client.try_clone() else { return };
    let mut reader = BufReader::with_capacity(65536, client);
    let mut line = Vec::new();
    let mut first = true;
    loop {
        line.clear();
        match read_line(&mut reader, &mut line, limit) {
            Line::Complete => {}
            Line::End => return,
            Line::TooLong => {
                let reply = json!({ "id": null, "ok": false, "error": { "code": "bad_request", "message": format!("request is too long (over {limit} bytes)") } });
                let _ = write_all(&writer, format!("{}\n", encode(&reply)).as_bytes());
                return;
            }
        }
        line.pop();
        if line.iter().all(|b| *b == b' ' || *b == b'\r') {
            continue;
        }
        if let Some((cmd, takeover)) = takeover.as_ref().filter(|_| std::mem::take(&mut first)) {
            let request = serde_json::from_slice::<Value>(&line).ok();
            if let Some(request) = request.filter(|r| r.get("cmd").and_then(Value::as_str) == Some(cmd)) {
                if reader.get_ref().set_read_timeout(None).is_err() {
                    return;
                }
                return takeover(request, reader, writer);
            }
        }
        let Some(reply) = respond(&line, &handler) else { return };
        let mut out = encode(&reply).into_bytes();
        out.push(b'\n');
        if !write_all(&writer, &out) {
            return;
        }
    }
}

fn respond(line: &[u8], handler: &Handler) -> Option<Value> {
    let Ok(request) = serde_json::from_slice::<Value>(line) else {
        return Some(json!({ "id": null, "ok": false, "error": { "code": "bad_request", "message": "request is not valid JSON" } }));
    };
    let (send, receive) = mpsc::channel();
    handler(request, Box::new(move |reply| {
        let _ = send.send(reply);
    }));
    receive.recv().ok()
}

#[cfg(target_vendor = "apple")]
fn no_sigpipe(client: &UnixStream) {
    let on: libc::c_int = 1;
    unsafe {
        libc::setsockopt(client.as_raw_fd(), libc::SOL_SOCKET, libc::SO_NOSIGPIPE, &on as *const _ as *const libc::c_void, std::mem::size_of::<libc::c_int>() as libc::socklen_t);
    }
}

#[cfg(not(target_vendor = "apple"))]
fn no_sigpipe(_: &UnixStream) {}

pub fn write_all(stream: &UnixStream, data: &[u8]) -> bool {
    #[cfg(target_vendor = "apple")]
    const FLAGS: libc::c_int = 0;
    #[cfg(not(target_vendor = "apple"))]
    const FLAGS: libc::c_int = libc::MSG_NOSIGNAL;
    let mut offset = 0;
    while offset < data.len() {
        let n = unsafe { libc::send(stream.as_raw_fd(), data[offset..].as_ptr() as *const libc::c_void, data.len() - offset, FLAGS) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return false;
        }
        offset += n as usize;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Duration;

    fn server(name: &str, limits: Limits) -> (SocketServer, String) {
        let dir = std::env::temp_dir().join(format!("kml-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sock").to_string_lossy().into_owned();
        let handler: Handler = Arc::new(|request: Value, reply: Box<dyn FnOnce(Value) + Send>| reply(json!({ "id": request["id"].clone(), "ok": true })));
        let mut server = SocketServer::new(&path, handler);
        server.limits = limits;
        (server, path)
    }

    fn limits() -> Limits {
        Limits { connections: 64, line: 1 << 20, idle: Duration::from_secs(60) }
    }

    fn read_reply(stream: &UnixStream) -> Option<String> {
        let mut reply = String::new();
        BufReader::new(stream.try_clone().unwrap()).read_line(&mut reply).ok()?;
        (!reply.is_empty()).then_some(reply)
    }

    #[test]
    fn a_line_over_the_limit_is_refused_and_the_connection_closed() {
        let (server, path) = server("long", Limits { line: 1024, ..limits() });
        server.start().unwrap();
        let mut stream = UnixStream::connect(&path).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = stream.write_all(&vec![b'x'; 4096]);
        let reply = read_reply(&stream).unwrap();
        assert!(reply.contains("bad_request") && reply.contains("too long"), "{reply}");
        let mut rest = Vec::new();
        let _ = stream.read_to_end(&mut rest);
        let mut short = UnixStream::connect(&path).unwrap();
        short.write_all(&[b' '; 1000]).unwrap();
        short.write_all(b"{\"id\":7}\n").unwrap();
        assert!(read_reply(&short).unwrap().contains("\"id\":7"));
    }

    #[test]
    fn an_idle_connection_is_closed() {
        let (server, path) = server("idle", Limits { idle: Duration::from_millis(100), ..limits() });
        server.start().unwrap();
        let mut stream = UnixStream::connect(&path).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        let mut rest = Vec::new();
        assert_eq!(stream.read_to_end(&mut rest).unwrap(), 0);
    }

    #[test]
    fn a_taken_over_connection_has_no_idle_limit() {
        let (mut server, path) = server("app", Limits { idle: Duration::from_millis(100), ..limits() });
        server.take_over(
            "app.hello",
            Arc::new(|_hello: Value, mut reader: BufReader<UnixStream>, writer: UnixStream| {
                let mut line = String::new();
                if reader.read_line(&mut line).is_ok() {
                    let _ = write_all(&writer, format!("echo {line}").as_bytes());
                }
            }),
        );
        server.start().unwrap();
        let mut stream = UnixStream::connect(&path).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.write_all(b"{\"cmd\":\"app.hello\"}\n").unwrap();
        std::thread::sleep(Duration::from_millis(400));
        stream.write_all(b"later\n").unwrap();
        assert_eq!(read_reply(&stream).unwrap(), "echo later\n");
    }

    #[test]
    fn connections_over_the_limit_wait_for_a_free_one() {
        let (server, path) = server("cap", Limits { connections: 2, ..limits() });
        server.start().unwrap();
        let first = UnixStream::connect(&path).unwrap();
        let mut second = UnixStream::connect(&path).unwrap();
        second.write_all(b"{\"id\":2}\n").unwrap();
        assert!(read_reply(&second).unwrap().contains("\"id\":2"));
        let mut third = UnixStream::connect(&path).unwrap();
        third.write_all(b"{\"id\":3}\n").unwrap();
        third.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        assert_eq!(read_reply(&third), None);
        drop(first);
        third.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert!(read_reply(&third).unwrap().contains("\"id\":3"));
    }

    #[test]
    fn a_connection_made_between_two_servers_on_one_listener_is_answered_by_the_second() {
        let (first, path) = server("handover", limits());
        first.start().unwrap();
        let kept = UnixListener::from(first.listener().unwrap());
        first.stop_leaving_path();
        assert!(Path::new(&path).exists());
        let mut waiting = UnixStream::connect(&path).unwrap();
        waiting.write_all(b"{\"id\":7}\n").unwrap();
        first.start_with(kept).unwrap();
        waiting.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert!(read_reply(&waiting).unwrap().contains("\"id\":7"));
        first.stop();
        assert!(!Path::new(&path).exists());
    }
}
