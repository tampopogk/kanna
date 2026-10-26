use kanna_net::{Client, SecretKey};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

const WARMUP: usize = 10;

const GAP: Duration = Duration::from_millis(10);

struct Args {
    socket: String,
    peer: String,
    addrs: Vec<String>,
    relay: Option<String>,
    relay_only: bool,
    samples: usize,
    label: String,
    output: Option<String>,
}

fn args() -> Args {
    let mut out = Args { socket: String::new(), peer: String::new(), addrs: vec![], relay: None, relay_only: false, samples: 100, label: "run".into(), output: None };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        let mut value = || argv.next().unwrap_or_else(|| panic!("{arg} needs a value"));
        match arg.as_str() {
            "--socket" => out.socket = value(),
            "--peer" => out.peer = value(),
            "--addr" => out.addrs.push(value()),
            "--relay" => out.relay = Some(value()),
            "--relay-only" => out.relay_only = true,
            "--samples" => out.samples = value().parse().expect("--samples N"),
            "--label" => out.label = value(),
            "--output" => out.output = Some(value()),
            other => panic!("unknown argument {other}: terminal-echo --socket LOCAL.sock --peer KEY [--addr HOST:PORT] [--relay URL [--relay-only]] [--samples N] [--label NAME] [--output FILE]"),
        }
    }
    assert!(!out.socket.is_empty() && !out.peer.is_empty(), "--socket and --peer are needed");
    out
}

struct Link {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next: u64,
    received: u64,
}

impl Link {
    fn send(&mut self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.stream.write_all(line.as_bytes()).unwrap();
    }

    fn line(&mut self) -> (Value, usize) {
        let mut text = String::new();
        let n = self.reader.read_line(&mut text).unwrap();
        assert!(n > 0, "the link closed");
        self.received += n as u64;
        (serde_json::from_str(&text).unwrap(), n)
    }

    fn until(&mut self, mut wanted: impl FnMut(&Value) -> bool) -> (Value, usize) {
        loop {
            let (message, n) = self.line();
            if wanted(&message) {
                return (message, n);
            }
        }
    }

    fn call(&mut self, cmd: &str, args: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(json!({ "id": id, "cmd": cmd, "args": args }));
        let (reply, _) = self.until(|m| m["id"] == id && m.get("ok").is_some());
        assert_eq!(reply["ok"], true, "{cmd}: {reply}");
        reply
    }

    fn tell(&mut self, cmd: &str, args: Value) {
        self.send(json!({ "cmd": cmd, "args": args }));
    }

    fn open(&mut self, name: &str, cmd: &str) -> (String, String) {
        let reply = self.call("open", json!({ "type": "term", "name": name, "cmd": cmd }));
        (reply["pane"]["id"].as_str().unwrap().to_string(), reply["pane"]["terminal"].as_str().unwrap().to_string())
    }
}

fn data(message: &Value) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(message["args"]["data"].as_str().unwrap_or_default()).unwrap_or_default()
}

fn is(message: &Value, cmd: &str, terminal: &str) -> bool {
    message["cmd"] == cmd && message["args"]["terminal"] == terminal
}

fn certify(socket: &str, key: &str) -> Value {
    let stream = UnixStream::connect(socket).unwrap_or_else(|e| panic!("can't reach the local daemon at {socket}: {e}"));
    (&stream).write_all(format!("{}\n", json!({ "id": 1, "cmd": "fleet", "args": { "action": "certify", "key": key } })).as_bytes()).unwrap();
    let mut text = String::new();
    BufReader::new(&stream).read_line(&mut text).unwrap();
    let reply: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(reply["ok"], true, "certify: {reply}");
    reply["cert"].clone()
}

fn echo(link: &mut Link, terminal: &str, samples: usize, key: u8) -> Vec<f64> {
    let mut times = vec![];
    for i in 0..WARMUP + samples {
        let started = Instant::now();
        link.tell("input", json!({ "terminal": terminal, "data": base64(&[key]) }));
        link.until(|m| is(m, "output", terminal) && data(m).contains(&key));
        if i >= WARMUP {
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        std::thread::sleep(GAP);
    }
    link.tell("input", json!({ "terminal": terminal, "data": base64(b"\r") }));
    times
}

fn round_trips(link: &mut Link, samples: usize) -> Vec<f64> {
    let mut times = vec![];
    for i in 0..WARMUP + samples {
        let started = Instant::now();
        link.call("capabilities", json!({}));
        if i >= WARMUP {
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        std::thread::sleep(GAP);
    }
    times
}

fn base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn attach(link: &mut Link, terminal: &str, args: Value) -> Value {
    let mut args = args;
    args["terminal"] = json!(terminal);
    let started = Instant::now();
    link.tell("attach", args);
    link.next += 1;
    let id = link.next;
    link.send(json!({ "id": id, "cmd": "capabilities", "args": {} }));
    let (mut kind, mut bytes, mut wire, mut offset, mut more) = (Value::Null, 0, 0, Value::Null, Value::Null);
    loop {
        let (message, n) = link.line();
        if message["id"] == id && message.get("ok").is_some() {
            break;
        }
        if message["args"]["terminal"] != terminal || !matches!(message["cmd"].as_str(), Some("snapshot" | "output" | "grid")) {
            continue;
        }
        if kind.is_null() {
            kind = message["cmd"].clone();
        }
        wire += n;
        bytes += data(&message).len();
        if message["args"].get("offset").is_some() {
            offset = message["args"]["offset"].clone();
        }
        if message["cmd"] == "snapshot" {
            more = message["args"]["more"].clone();
        }
    }
    json!({ "kind": kind, "bytes": bytes, "wire_bytes": wire, "ms": started.elapsed().as_secs_f64() * 1000.0, "offset": offset, "more": more })
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn summary(times: &[f64]) -> Value {
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({ "n": sorted.len(), "p50": percentile(&sorted, 50.0), "p95": percentile(&sorted, 95.0), "max": sorted.last() })
}

fn main() {
    let args = args();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let secret = SecretKey::generate();
    let client = if args.relay_only { runtime.block_on(Client::start_relay_only(secret)) } else { runtime.block_on(Client::start(secret)) }.unwrap();
    let cert = certify(&args.socket, &client.key());
    let dialed = Instant::now();
    let stream = runtime.block_on(client.link(&args.peer, &args.addrs, args.relay.as_deref(), &cert)).unwrap_or_else(|f| panic!("can't link: {}: {}", f.code, f.message));
    let reader = BufReader::new(stream.try_clone().unwrap());
    let mut link = Link { stream, reader, next: 0, received: 0 };
    link.send(json!({ "id": 0, "cmd": "app.hello", "args": { "client": "terminal-echo", "protocols": [1, 1] } }));
    let (hello, _) = link.until(|m| m["id"] == 0);
    assert_eq!(hello["ok"], true, "hello: {hello}");
    let linked_ms = dialed.elapsed().as_secs_f64() * 1000.0;
    let run = hello["daemon"]["run"].clone();

    let (echo_pane, terminal) = link.open("echo-bench", "cat");
    link.tell("attach", json!({ "terminal": terminal }));
    link.until(|m| is(m, "grid", &terminal));
    let rtt = round_trips(&mut link, args.samples);
    let idle = echo(&mut link, &terminal, args.samples, b'k');
    let path_idle = client.path(&args.peer);

    let (flood_pane, flood) = link.open("flood-bench", "while :; do seq 1 2000; done");
    link.tell("attach", json!({ "terminal": flood }));
    let loaded = echo(&mut link, &terminal, args.samples, b'j');
    link.call("close", json!({ "pane": flood_pane }));

    let (long_pane, long) = link.open("scrollback-bench", "seq 1 200000; exec cat");
    std::thread::sleep(Duration::from_secs(2));
    let full = attach(&mut link, &long, json!({}));
    link.tell("detach", json!({ "terminal": long }));
    let bounded = attach(&mut link, &long, json!({ "scrollback": 1000 }));
    link.tell("detach", json!({ "terminal": long }));
    link.tell("input", json!({ "terminal": long, "data": base64(b"typed while away\r") }));
    std::thread::sleep(Duration::from_millis(500));
    let replayed = attach(&mut link, &long, json!({ "scrollback": 1000, "from": bounded["offset"], "run": run }));
    link.call("close", json!({ "pane": long_pane }));
    link.call("close", json!({ "pane": echo_pane }));
    let path = client.path(&args.peer);

    let report = json!({
        "label": args.label,
        "relay": args.relay,
        "relay_only": args.relay_only,
        "path_idle": path_idle,
        "path": path,
        "linked_ms": linked_ms,
        "link_rtt": summary(&rtt),
        "echo_idle": summary(&idle),
        "echo_loaded": summary(&loaded),
        "attach_full": full,
        "attach_bounded": bounded,
        "reconnect_from_offset": replayed,
        "samples": { "link_rtt": rtt, "echo_idle": idle, "echo_loaded": loaded },
    });
    if let Some(output) = &args.output {
        std::fs::write(output, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
    let ms = |v: &Value| format!("{:.2}", v.as_f64().unwrap_or(f64::NAN));
    println!(
        "{}: {} · link RTT p50 {} p95 {} · echo idle p50 {} p95 {} · echo loaded p50 {} p95 {} · attach full {} B {} ms · bounded {} B {} ms · reconnect {} {} B {} ms",
        args.label,
        path["path"].as_str().unwrap_or("?"),
        ms(&report["link_rtt"]["p50"]),
        ms(&report["link_rtt"]["p95"]),
        ms(&report["echo_idle"]["p50"]),
        ms(&report["echo_idle"]["p95"]),
        ms(&report["echo_loaded"]["p50"]),
        ms(&report["echo_loaded"]["p95"]),
        full["wire_bytes"],
        ms(&full["ms"]),
        bounded["wire_bytes"],
        ms(&bounded["ms"]),
        replayed["kind"].as_str().unwrap_or("?"),
        replayed["wire_bytes"],
        ms(&replayed["ms"]),
    );
}
