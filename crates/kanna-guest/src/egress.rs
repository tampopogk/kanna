use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

const MAX_HEAD: usize = 16 * 1024;

pub struct JobProxy {
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl JobProxy {
    pub fn start(upstream: u16, capability: &str) -> io::Result<JobProxy> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let authorization = format!("Proxy-Authorization: Basic {}\r\n", base64(format!("job:{capability}").as_bytes()));
        let stopped = stop.clone();
        std::thread::Builder::new().name("kanna-guest job proxy".into()).spawn(move || {
            for client in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(client) = client else { continue };
                let authorization = authorization.clone();
                let _ = std::thread::Builder::new().name("kanna-guest job proxy conn".into()).spawn(move || {
                    let _ = forward(client, upstream, &authorization);
                });
            }
        })?;
        Ok(JobProxy { port, stop })
    }
}

impl Drop for JobProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn read_head(stream: &mut TcpStream) -> io::Result<Option<Vec<u8>>> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_HEAD || stream.read(&mut byte)? == 0 {
            return Ok(None);
        }
        head.push(byte[0]);
    }
    Ok(Some(head))
}

fn forward(mut client: TcpStream, upstream: u16, authorization: &str) -> io::Result<()> {
    let Some(head) = read_head(&mut client)? else { return Ok(()) };
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut words = text.split_whitespace();
    let (method, target) = (words.next().unwrap_or_default(), words.next().unwrap_or_default());
    if method != "CONNECT" {
        if let Some(rest) = target.strip_prefix("https://") {
            return fetch_https(client, upstream, authorization, &text, method, rest);
        }
    }
    let mut server = TcpStream::connect(("127.0.0.1", upstream))?;
    server.write_all(&head)?;
    let (mut client_read, mut server_write) = (client.try_clone()?, server.try_clone()?);
    let up = std::thread::spawn(move || {
        let _ = io::copy(&mut client_read, &mut server_write);
        let _ = server_write.shutdown(Shutdown::Write);
    });
    let _ = io::copy(&mut server, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    let _ = up.join();
    Ok(())
}

fn tls_config() -> io::Result<Arc<rustls::ClientConfig>> {
    static CONFIG: OnceLock<Result<Arc<rustls::ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            use rustls_platform_verifier::BuilderVerifierExt;
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .and_then(|b| b.with_platform_verifier())
                .map_err(|e| e.to_string())?
                .with_no_client_auth();
            Ok(Arc::new(config))
        })
        .clone()
        .map_err(io::Error::other)
}

fn refuse(client: &mut TcpStream, status: &str) -> io::Result<()> {
    client.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
}

fn fetch_https(mut client: TcpStream, upstream: u16, authorization: &str, head: &str, method: &str, rest: &str) -> io::Result<()> {
    let (authority, path) = rest.split_at(rest.find(['/', '?']).unwrap_or(rest.len()));
    let path = if path.is_empty() { "/".to_string() } else if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
    let authority = authority.rsplit_once('@').map(|(_, a)| a).unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.ends_with(']') || authority.starts_with('[') => (host, port.parse::<u16>().unwrap_or(0)),
        _ => (authority, 443),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() || port == 0 || host.contains(|c: char| c.is_whitespace() || c.is_control()) {
        return refuse(&mut client, "400 Bad Request");
    }
    let body_length = head.lines().find_map(|line| line.split_once(':').filter(|(name, _)| name.trim().eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse::<u64>().ok())).unwrap_or(0);
    if head.lines().any(|line| line.split_once(':').is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("transfer-encoding"))) {
        return refuse(&mut client, "411 Length Required");
    }
    let mut server = TcpStream::connect(("127.0.0.1", upstream))?;
    let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    server.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{authorization}\r\n").as_bytes())?;
    let Some(answer) = read_head(&mut server)? else { return refuse(&mut client, "502 Bad Gateway") };
    let status = String::from_utf8_lossy(&answer).lines().next().unwrap_or_default().to_string();
    if status.split_whitespace().nth(1) != Some("200") {
        client.write_all(&answer)?;
        return Ok(());
    }
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let connection = rustls::ClientConnection::new(tls_config()?, name).map_err(io::Error::other)?;
    let mut tls = rustls::StreamOwned::new(connection, server);
    let mut request = format!("{method} {path} HTTP/1.1\r\n");
    let mut has_host = false;
    for line in head.split("\r\n").skip(1).filter(|l| !l.is_empty()) {
        let name = line.split_once(':').map(|(n, _)| n.trim().to_ascii_lowercase()).unwrap_or_default();
        match name.as_str() {
            "proxy-authorization" | "proxy-connection" | "connection" | "keep-alive" => continue,
            "host" => has_host = true,
            _ => {}
        }
        request.push_str(line);
        request.push_str("\r\n");
    }
    if !has_host {
        request.push_str(&format!("Host: {}\r\n", if port == 443 { host.to_string() } else { target.clone() }));
    }
    request.push_str("Connection: close\r\n\r\n");
    tls.write_all(request.as_bytes())?;
    io::copy(&mut (&mut client).take(body_length), &mut tls)?;
    tls.flush()?;
    match io::copy(&mut tls, &mut client) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {}
        Err(e) => return Err(e),
    }
    let _ = client.shutdown(Shutdown::Write);
    Ok(())
}

pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"job:abc123"), "am9iOmFiYzEyMw==");
    }

    fn upstream() -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
                    head.push(byte[0]);
                }
                tx.send(String::from_utf8(head).unwrap()).unwrap();
                stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\npong").unwrap();
            }
        });
        (port, rx)
    }

    fn ask(port: u16, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    }

    #[test]
    fn a_connect_without_credentials_is_passed_as_is_so_zig_falls_back_to_absolute_form() {
        let (port, heads) = upstream();
        let proxy = JobProxy::start(port, "cap1").unwrap();
        let request = "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n";
        assert!(ask(proxy.port, request).ends_with("pong"));
        assert_eq!(heads.recv().unwrap(), request);
    }

    #[test]
    fn a_connect_with_credentials_is_passed_as_is() {
        let (port, heads) = upstream();
        let proxy = JobProxy::start(port, "cap1").unwrap();
        let request = "CONNECT example.com:443 HTTP/1.1\r\nproxy-authorization: Basic other\r\n\r\n";
        ask(proxy.port, request);
        assert_eq!(heads.recv().unwrap(), request);
    }

    #[test]
    fn an_absolute_https_request_becomes_an_authenticated_tunnel_carrying_tls() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let head = read_head(&mut stream).unwrap().unwrap();
            stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
            let mut first = [0u8; 1];
            stream.read_exact(&mut first).unwrap();
            (String::from_utf8(head).unwrap(), first[0])
        });
        let proxy = JobProxy::start(port, "cap1").unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", proxy.port)).unwrap();
        stream.write_all(b"GET https://deps.example.org/zlib.tar.gz HTTP/1.1\r\nHost: deps.example.org\r\n\r\n").unwrap();
        let (head, first) = seen.join().unwrap();
        assert_eq!(head, "CONNECT deps.example.org:443 HTTP/1.1\r\nHost: deps.example.org:443\r\nProxy-Authorization: Basic am9iOmNhcDE=\r\n\r\n");
        assert_eq!(first, 0x16);
    }

    #[test]
    fn a_refused_tunnel_is_answered_with_the_refusal() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_head(&mut stream).unwrap();
            stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").unwrap();
        });
        let proxy = JobProxy::start(port, "cap1").unwrap();
        let reply = ask(proxy.port, "GET https://evil.example/x HTTP/1.1\r\n\r\n");
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
    }

    #[test]
    fn the_port_closes_when_the_job_ends() {
        let (port, _heads) = upstream();
        let proxy = JobProxy::start(port, "cap1").unwrap();
        let job_port = proxy.port;
        drop(proxy);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(TcpStream::connect(("127.0.0.1", job_port)).is_err());
    }
}
