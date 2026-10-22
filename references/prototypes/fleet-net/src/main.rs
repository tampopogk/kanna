//! A transport model, not a daemon: all six stream kinds share one kanna/1 connection.
use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use futures_util::StreamExt;
use iroh::{
    endpoint::{presets, Connection, RecvStream, SendStream},
    Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey,
};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use serde_json::{json, Value};
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

const ALPN: &[u8] = b"kanna/1";
const KINDS: [&str; 5] = ["file", "port", "sync", "job", "transfer"];
const BLOCK: usize = 64 * 1024;

#[derive(Parser, Debug)]
struct Cli {
    /// Keep private keys outside the repository. Omit for an ephemeral endpoint.
    #[arg(long)]
    key: Option<PathBuf>,
    /// Only this relay is used. Omit for LAN-only, with no external discovery.
    #[arg(long)]
    relay: Option<RelayUrl>,
    /// Disable all UDP transports to demonstrate guaranteed relay fallback.
    #[arg(long, requires = "relay")]
    relay_only: bool,
    /// LAN discovery (can dial an endpoint ID without supplying its addresses).
    #[arg(long)]
    mdns: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand, Debug)]
enum Command {
    Serve,
    Probe {
        #[arg(long)]
        peer: EndpointId,
        /// Enforced before measuring; a selected path of another kind fails the run.
        #[arg(long, value_enum)]
        expect: Expected,
        #[arg(long, default_value = "100")]
        samples: usize,
        #[arg(long, default_value = "16")]
        mib: u64,
        #[arg(long, default_value = "30")]
        settle_seconds: u64,
        #[arg(long, default_value = "unnamed")]
        label: String,
        #[arg(long)]
        output: PathBuf,
    },
}
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq)]
enum Expected {
    Direct,
    Relay,
}

fn key(path: &Option<PathBuf>) -> Result<SecretKey> {
    let Some(path) = path else {
        return Ok(SecretKey::generate());
    };
    if path.exists() {
        return Ok(std::fs::read_to_string(path)?.trim().parse()?);
    }
    use std::os::unix::fs::OpenOptionsExt;
    let secret = SecretKey::generate();
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(
        secret
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            .as_bytes(),
    )?;
    Ok(secret)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    ensure!(
        !(cli.relay_only && cli.mdns),
        "relay-only and mDNS are separate test scenarios"
    );
    let relay_mode = match &cli.relay {
        Some(url) => RelayMode::Custom([url.clone()].into_iter().collect()),
        None => RelayMode::Disabled,
    };
    // Minimal does not install n0 DNS/pkarr or public relays. Never use presets::N0 here.
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(key(&cli.key)?)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(relay_mode);
    if cli.relay_only {
        builder = builder.clear_ip_transports();
    }
    if cli.mdns {
        builder = builder.address_lookup(MdnsAddressLookup::builder());
    }
    let endpoint = builder.bind().await?;
    if cli.relay.is_some() {
        tokio::time::timeout(Duration::from_secs(30), endpoint.online())
            .await
            .context("relay registration timed out")?;
    }
    eprintln!(
        "{}",
        json!({"event":"endpoint", "id":endpoint.id().to_string(), "addresses":format!("{:?}",endpoint.addr()), "mdns":cli.mdns, "relay_only":cli.relay_only})
    );
    let result = match &cli.command {
        Command::Serve => serve(&endpoint).await,
        Command::Probe {
            peer,
            expect,
            samples,
            mib,
            settle_seconds,
            label,
            output,
        } => {
            ensure!(
                *samples > 0 && *mib > 0 && *mib <= 1024,
                "samples must be positive; mib must be 1..1024"
            );
            let mut addr = EndpointAddr::new(*peer);
            if let Some(url) = &cli.relay {
                addr = addr.with_relay_url(url.clone());
            }
            let start = Instant::now();
            let conn = tokio::time::timeout(Duration::from_secs(45), endpoint.connect(addr, ALPN))
                .await??;
            let connect_ms = start.elapsed().as_secs_f64() * 1000.;
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut stream = conn.path_events();
            let events_copy = events.clone();
            let monitor = tokio::spawn(async move {
                while let Some(event) = stream.next().await {
                    events_copy.lock().unwrap().push(json!({"after_connect_ms":start.elapsed().as_secs_f64()*1000.,"event":format!("{event:?}")}));
                }
            });
            let initial_paths = paths(&conn);
            let result = tokio::time::timeout(
                Duration::from_secs(900),
                measure(&conn, *expect, *samples, *mib, *settle_seconds),
            )
            .await;
            monitor.abort();
            let report = json!({
                "schema":1, "iroh":"1.4.1", "mdns_version":"0.6.1", "alpn":"kanna/1",
                "unix_seconds":SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                "label":label, "os":std::env::consts::OS, "arch":std::env::consts::ARCH,
                "peer":peer.to_string(), "relay":cli.relay.as_ref().map(ToString::to_string),
                "mdns":cli.mdns, "relay_only":cli.relay_only, "expected":format!("{expect:?}"),
                "connect_ms":connect_ms, "initial_paths":initial_paths, "final_paths":paths(&conn),
                "path_events":events.lock().unwrap().clone(),
                "measurement":match &result { Ok(Ok(v))=>v.clone(), Ok(Err(e))=>json!({"error":format!("{e:#}")}), Err(e)=>json!({"error":e.to_string()}) }
            });
            std::fs::write(output, serde_json::to_vec_pretty(&report)?)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            result??;
            Ok(())
        }
    };
    endpoint.close().await;
    result
}

fn paths(conn: &Connection) -> Value {
    json!(conn
        .paths()
        .iter()
        .map(|p| {
            let stats = p.stats();
            json!({"id":format!("{:?}",p.id()),"remote":p.remote_addr().to_string(),
            "local":format!("{:?}",p.local_addr()),"selected":p.is_selected(),
            "kind":if p.is_ip(){"direct"}else{"relay"},"rtt_ms":p.rtt().as_secs_f64()*1000.,
            "udp_tx_bytes":stats.udp_tx.bytes,"udp_rx_bytes":stats.udp_rx.bytes,
            "lost_packets":stats.lost_packets,"sent_packets":stats.sent_packets,
            "cwnd":stats.cwnd,"mtu":stats.current_mtu})
        })
        .collect::<Vec<_>>())
}

fn selected(conn: &Connection) -> Option<Expected> {
    conn.paths().iter().find(|p| p.is_selected()).map(|p| {
        if p.is_ip() {
            Expected::Direct
        } else {
            Expected::Relay
        }
    })
}
fn check_path(conn: &Connection, expected: Expected) -> Result<()> {
    ensure!(
        selected(conn) == Some(expected),
        "selected path changed: expected {expected:?}, got {:?}",
        selected(conn)
    );
    Ok(())
}
async fn line(send: &mut SendStream, value: Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    send.write_all(&bytes).await?;
    Ok(())
}
async fn read_line(recv: &mut BufReader<RecvStream>) -> Result<Value> {
    let mut s = String::new();
    ensure!(recv.read_line(&mut s).await? > 0, "unexpected EOF");
    Ok(serde_json::from_str(&s)?)
}
async fn serve(endpoint: &Endpoint) -> Result<()> {
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else {return Ok(())};
                tokio::spawn(async move {
                    let result: Result<()> = async {
                        let conn = incoming.await?;
                        eprintln!("accepted {}",conn.remote_id());
                        loop {
                            let (send, recv) = match conn.accept_bi().await {Ok(v)=>v,Err(_)=>return Ok(())};
                            tokio::spawn(async move { if let Err(e)=handle(send,recv).await {eprintln!("stream: {e:#}");} });
                        }
                    }.await;
                    if let Err(e)=result {eprintln!("connection: {e:#}");}
                });
            }
        }
    }
}
async fn handle(mut send: SendStream, recv: RecvStream) -> Result<()> {
    let mut recv = BufReader::new(recv);
    let header = read_line(&mut recv).await?;
    let kind = header["kind"].as_str().context("kind missing")?;
    if kind == "control" {
        loop {
            let mut s = String::new();
            if recv.read_line(&mut s).await? == 0 {
                break;
            }
            let msg: Value = serde_json::from_str(&s)?;
            let response = match msg["cmd"].as_str() {
                Some("app.hello") => {
                    json!({"id":msg["id"],"ok":true,"protocol":1,"daemon":{"machine":"fleet-net-model"},"panes":[],"layout":{}})
                }
                Some("model.echo") => json!({"id":msg["id"],"ok":true,"data":msg["args"]["data"]}),
                _ => bail!("unknown control command"),
            };
            line(&mut send, response).await?;
        }
    } else {
        ensure!(KINDS.contains(&kind), "unknown kind {kind}");
        send.set_priority(-1)?;
        let bytes = header["bytes"].as_u64().context("bytes missing")?;
        match header["op"].as_str() {
            Some("push") => {
                receive_bytes(&mut recv, bytes).await?;
                line(&mut send, json!({"ok":true,"bytes":bytes})).await?;
            }
            Some("pull") => {
                send_bytes(&mut send, bytes).await?;
            }
            _ => bail!("unknown bulk operation"),
        }
    }
    send.finish()?;
    Ok(())
}
async fn send_bytes(send: &mut SendStream, mut bytes: u64) -> Result<()> {
    let block = vec![0x5a; BLOCK];
    while bytes > 0 {
        let n = bytes.min(BLOCK as u64) as usize;
        send.write_all(&block[..n]).await?;
        bytes -= n as u64;
    }
    Ok(())
}
async fn receive_bytes(recv: &mut BufReader<RecvStream>, mut bytes: u64) -> Result<()> {
    let mut block = vec![0; BLOCK];
    let expected = vec![0x5a; BLOCK];
    while bytes > 0 {
        let n = bytes.min(BLOCK as u64) as usize;
        recv.read_exact(&mut block[..n]).await?;
        ensure!(block[..n] == expected[..n], "payload mismatch");
        bytes -= n as u64;
    }
    let mut extra = [0];
    ensure!(recv.read(&mut extra).await? == 0, "trailing payload bytes");
    Ok(())
}
struct Control {
    send: SendStream,
    recv: BufReader<RecvStream>,
    seq: u64,
}
impl Control {
    async fn open(conn: &Connection) -> Result<Self> {
        let (mut send, recv) = conn.open_bi().await?;
        line(&mut send, json!({"kind":"control"})).await?;
        line(&mut send,json!({"id":0,"cmd":"app.hello","args":{"protocols":{"min":1,"max":1},"client":"fleet-net-probe","commit":"model"}})).await?;
        let mut recv = BufReader::new(recv);
        let reply = read_line(&mut recv).await?;
        ensure!(
            reply["ok"] == true && reply["protocol"] == 1,
            "hello failed"
        );
        Ok(Self { send, recv, seq: 0 })
    }
    async fn echo(&mut self) -> Result<f64> {
        self.seq += 1;
        let start = Instant::now();
        line(
            &mut self.send,
            json!({"id":self.seq,"cmd":"model.echo","args":{"data":"eA=="}}),
        )
        .await?;
        let reply =
            tokio::time::timeout(Duration::from_secs(15), read_line(&mut self.recv)).await??;
        ensure!(
            reply["id"] == self.seq && reply["ok"] == true && reply["data"] == "eA==",
            "echo mismatch"
        );
        Ok(start.elapsed().as_secs_f64() * 1000.)
    }
}
fn distribution(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({"n":0});
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let pct = |p: f64| sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    json!({"n":values.len(),"p50_ms":pct(0.5),"p95_ms":pct(0.95),"p99_ms":pct(0.99),"max_ms":sorted.last(),"samples_ms":values})
}
async fn bulk(conn: &Connection, kind: &str, op: &str, bytes: u64) -> Result<Value> {
    let start = Instant::now();
    let (mut send, recv) = conn.open_bi().await?;
    // Prioritise control over queued bulk without serialising independent streams.
    send.set_priority(-1)?;
    line(&mut send, json!({"kind":kind,"op":op,"bytes":bytes})).await?;
    let mut recv = BufReader::new(recv);
    if op == "push" {
        send_bytes(&mut send, bytes).await?;
        send.finish()?;
        let ack = read_line(&mut recv).await?;
        ensure!(
            ack["ok"] == true && ack["bytes"] == bytes,
            "bulk ack mismatch"
        );
    } else {
        send.finish()?;
        receive_bytes(&mut recv, bytes).await?;
    }
    let seconds = start.elapsed().as_secs_f64();
    Ok(
        json!({"kind":kind,"direction":op,"bytes":bytes,"seconds":seconds,"mbps":bytes as f64*8./seconds/1_000_000.}),
    )
}
async fn measure(
    conn: &Connection,
    expect: Expected,
    samples: usize,
    mib: u64,
    settle_seconds: u64,
) -> Result<Value> {
    let mut control = Control::open(conn).await?; // This is the connection's first stream.
    let settle = Instant::now();
    loop {
        control.echo().await?;
        if selected(conn) == Some(expect) {
            break;
        }
        ensure!(
            settle.elapsed() < Duration::from_secs(settle_seconds),
            "did not reach {expect:?}: {}",
            paths(conn)
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let settle_ms = settle.elapsed().as_secs_f64() * 1000.;
    // Warm up separately, retaining connect/settle timings in the report.
    for _ in 0..10 {
        control.echo().await?;
    }
    let settled_paths = paths(conn);
    let mut idle = Vec::new();
    for _ in 0..samples {
        check_path(conn, expect)?;
        idle.push(control.echo().await?);
        check_path(conn, expect)?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut bulk_results = Vec::new();
    for kind in KINDS {
        for op in ["push", "pull"] {
            check_path(conn, expect)?;
            let before = paths(conn);
            let bulk_conn = conn.clone();
            let work =
                tokio::spawn(async move { bulk(&bulk_conn, kind, op, mib * 1024 * 1024).await });
            let mut loaded = Vec::new();
            // Finish each request/reply even if bulk completes mid-echo. Cancelling an
            // in-flight read would leave a stale reply on the shared control stream.
            while !work.is_finished() {
                loaded.push(control.echo().await?);
                check_path(conn, expect)?;
            }
            let mut result = work.await??;
            check_path(conn, expect)?;
            result["echo_under_load"] = distribution(&loaded);
            result["paths_before"] = before;
            result["paths_after"] = paths(conn);
            eprintln!("{kind} {op}: {:.2} Mbps", result["mbps"].as_f64().unwrap());
            bulk_results.push(result);
        }
    }
    control.send.finish()?;
    Ok(
        json!({"settle_ms":settle_ms,"settled_paths":settled_paths,"echo_idle":distribution(&idle),"bulk":bulk_results}),
    )
}
