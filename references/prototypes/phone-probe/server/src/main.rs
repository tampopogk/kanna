use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, RelayMap, RelayMode, SecretKey};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use qrcode::render::unicode;
use qrcode::QrCode;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const ALPN: &[u8] = b"kanna/1";
const PORT: u16 = 7479;

fn secret(path: &Path) -> SecretKey {
    if let Ok(text) = std::fs::read_to_string(path) {
        let bytes: Vec<u8> = (0..64).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect();
        return SecretKey::from_bytes(&bytes.try_into().unwrap());
    }
    let key = SecretKey::generate();
    let text: String = key.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(path, text).unwrap();
    key
}

fn log(value: Value) {
    println!("{value}");
}

async fn serve(mut send: SendStream, recv: RecvStream, remote: String) {
    let mut recv = BufReader::new(recv);
    let mut line = String::new();
    if recv.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    log(json!({ "event": "stream", "from": remote, "header": line.trim() }));
    let ack = format!("{}\n", json!({ "ok": true, "kind": "probe" }));
    if send.write_all(ack.as_bytes()).await.is_err() {
        return;
    }
    loop {
        line.clear();
        match recv.read_line(&mut line).await {
            Ok(n) if n > 0 => {
                log(json!({ "event": "line", "from": remote, "line": serde_json::from_str::<Value>(line.trim()).unwrap_or(json!(line.trim())) }));
                if send.write_all(line.as_bytes()).await.is_err() {
                    return;
                }
            }
            _ => return,
        }
    }
}

async fn connection(conn: Connection) {
    let remote = conn.remote_id().to_string();
    let paths: Vec<String> = conn.paths().iter().map(|p| format!("{:?}", p.remote_addr())).collect();
    log(json!({ "event": "connected", "from": remote, "paths": paths }));
    while let Ok((send, recv)) = conn.accept_bi().await {
        tokio::spawn(serve(send, recv, remote.clone()));
    }
    log(json!({ "event": "closed", "from": remote, "reason": conn.close_reason().map(|r| r.to_string()) }));
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).map_or(".", String::as_str);
    let key = secret(&Path::new(dir).join("probe.key"));
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Custom(RelayMap::empty()))
        .bind_addr(SocketAddr::from(([0, 0, 0, 0], PORT)))
        .unwrap()
        .bind()
        .await
        .unwrap();
    let mdns = MdnsAddressLookup::builder().service_name("kanna").build(endpoint.id()).unwrap();
    endpoint.address_lookup().unwrap().add(mdns.clone());
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let addrs: Vec<String> = endpoint.addr().ip_addrs().filter(|a| a.is_ipv4() && !a.ip().is_loopback()).map(|a| a.to_string()).collect();
    let key = endpoint.id().to_string();
    let instance = z32::encode(endpoint.id().as_bytes());
    let payload = json!({ "key": key, "addrs": addrs, "instance": instance }).to_string();
    let qr = QrCode::new(payload.as_bytes()).unwrap();
    println!("{}", qr.render::<unicode::Dense1x2>().dark_color(unicode::Dense1x2::Light).light_color(unicode::Dense1x2::Dark).quiet_zone(true).build());
    log(json!({ "event": "listening", "key": key, "addrs": addrs, "instance": instance }));
    while let Some(incoming) = endpoint.accept().await {
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => connection(conn).await,
                Err(e) => log(json!({ "event": "failed", "error": e.to_string() })),
            }
        });
    }
}
