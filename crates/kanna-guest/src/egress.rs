use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

fn forward(mut client: TcpStream, upstream: u16, authorization: &str) -> io::Result<()> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_HEAD || client.read(&mut byte)? == 0 {
            return Ok(());
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let has_auth = text.lines().any(|line| line.to_ascii_lowercase().starts_with("proxy-authorization:"));
    let mut server = TcpStream::connect(("127.0.0.1", upstream))?;
    if has_auth {
        server.write_all(&head)?;
    } else {
        server.write_all(&head[..head.len() - 2])?;
        server.write_all(authorization.as_bytes())?;
        server.write_all(b"\r\n")?;
    }
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
    fn a_connect_without_credentials_gets_the_jobs() {
        let (port, heads) = upstream();
        let proxy = JobProxy::start(port, "cap1").unwrap();
        let reply = ask(proxy.port, "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n");
        assert!(reply.ends_with("pong"));
        let head = heads.recv().unwrap();
        assert_eq!(head, "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Basic am9iOmNhcDE=\r\n\r\n");
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
    fn the_port_closes_when_the_job_ends() {
        let (port, _heads) = upstream();
        let proxy = JobProxy::start(port, "cap1").unwrap();
        let job_port = proxy.port;
        drop(proxy);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(TcpStream::connect(("127.0.0.1", job_port)).is_err());
    }
}
