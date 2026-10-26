use crate::node::{read_line, write_line, DIAL, MAX_HEADER};
use crate::{Failure, ALPN, DEFAULT_PORT};
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};

pub struct Client {
    endpoint: Endpoint,
    conns: Mutex<HashMap<String, Connection>>,
}

impl Client {
    pub async fn start(secret: SecretKey) -> Result<Client, String> {
        let endpoint = Endpoint::builder(presets::Minimal).secret_key(secret).relay_mode(RelayMode::Disabled).bind().await.map_err(|e| format!("can't open the app's endpoint: {e}"))?;
        Ok(Client { endpoint, conns: Mutex::default() })
    }

    pub fn key(&self) -> String {
        self.endpoint.id().to_string()
    }

    fn conns(&self) -> std::sync::MutexGuard<'_, HashMap<String, Connection>> {
        self.conns.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn connect(&self, key: &str, addrs: &[String]) -> Result<(Connection, bool), Failure> {
        if let Some(conn) = self.conns().get(key).filter(|c| c.close_reason().is_none()).cloned() {
            return Ok((conn, false));
        }
        let id: EndpointId = key.parse().map_err(|_| Failure::new("bad_request", format!("{key} isn't a machine's key")))?;
        let mut addr = EndpointAddr::new(id);
        for socket in resolve(addrs).await {
            addr = addr.with_ip_addr(socket);
        }
        let conn = match tokio::time::timeout(DIAL, self.endpoint.connect(addr, ALPN)).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => return Err(Failure::new("unreachable", format!("can't reach it: {e}"))),
            Err(_) => return Err(Failure::new("unreachable", format!("can't reach it within {}s", DIAL.as_secs()))),
        };
        self.conns().insert(key.to_string(), conn.clone());
        Ok((conn, true))
    }

    pub async fn open(&self, key: &str, addrs: &[String], header: Value) -> Result<(SendStream, BufReader<RecvStream>, Value), Failure> {
        let (mut conn, mut fresh) = self.connect(key, addrs).await?;
        loop {
            match open(&conn, &header).await {
                Ok(opened) => return Ok(opened),
                Err(failure) => {
                    self.conns().remove(key);
                    if fresh || failure.code != "unreachable" {
                        return Err(failure);
                    }
                    (conn, fresh) = self.connect(key, addrs).await?;
                }
            }
        }
    }

    pub async fn link(&self, key: &str, addrs: &[String], cert: &Value) -> Result<std::os::unix::net::UnixStream, Failure> {
        let (mut send, mut recv, _) = self.open(key, addrs, json!({ "kind": "control", "cert": cert })).await?;
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().map_err(|e| Failure::new("failed", e.to_string()))?;
        let ours = ours.set_nonblocking(true).and_then(|()| tokio::net::UnixStream::from_std(ours)).map_err(|e| Failure::new("failed", e.to_string()))?;
        let (mut from_app, mut to_app) = ours.into_split();
        tokio::spawn(async move {
            let inbound = async move {
                let _ = tokio::io::copy(&mut recv, &mut to_app).await;
                let _ = to_app.shutdown().await;
            };
            let outbound = async move {
                let _ = tokio::io::copy(&mut from_app, &mut send).await;
                let _ = send.finish();
            };
            tokio::select! {
                () = inbound => {}
                () = outbound => {}
            }
        });
        Ok(theirs)
    }

    pub fn path(&self, key: &str) -> Value {
        let conns = self.conns();
        let Some(conn) = conns.get(key).filter(|c| c.close_reason().is_none()) else { return Value::Null };
        let paths = conn.paths();
        let Some(path) = paths.iter().find(|p| p.is_selected()) else { return Value::Null };
        let (remote, lan) = match path.remote_addr() {
            iroh::TransportAddr::Ip(socket) => (socket.to_string(), local(socket.ip())),
            other => (other.to_string(), false),
        };
        json!({ "path": if path.is_ip() { "direct" } else { "relay" }, "lan": path.is_ip() && lan, "rtt_ms": path.rtt().as_secs_f64() * 1000.0, "remote": remote })
    }

    pub async fn close(&self) {
        self.endpoint.close().await;
    }
}

async fn open(conn: &Connection, header: &Value) -> Result<(SendStream, BufReader<RecvStream>, Value), Failure> {
    let lost = |e: String| Failure::new("unreachable", format!("lost it: {e}"));
    let (mut send, recv) = conn.open_bi().await.map_err(|e| lost(e.to_string()))?;
    let mut recv = BufReader::new(recv);
    write_line(&mut send, header).await.map_err(lost)?;
    let ack = read_line(&mut recv, MAX_HEADER).await.map_err(lost)?;
    if ack["ok"] != true {
        let error = &ack["error"];
        return Err(Failure::new(error["code"].as_str().unwrap_or("failed"), format!("it refused this app: {}", error["message"].as_str().unwrap_or("refused"))));
    }
    Ok((send, recv, ack))
}

async fn resolve(addrs: &[String]) -> Vec<SocketAddr> {
    let mut found = vec![];
    for addr in addrs {
        if let Ok(socket) = addr.parse::<SocketAddr>() {
            found.push(socket);
            continue;
        }
        let named = if addr.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) { addr.clone() } else { format!("{addr}:{DEFAULT_PORT}") };
        if let Ok(Ok(addrs)) = tokio::time::timeout(Duration::from_secs(3), tokio::net::lookup_host(named)).await {
            found.extend(addrs);
        }
    }
    found
}

fn local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local() || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1])),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}
