use super::{usage, Command, Flow};
use crate::args::Argv;
use crate::output::join;
use crate::target::Session;
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "machine",
    group: "Information",
    summary: "Your machines: pair one, list them, check one's link, give an address or a relay, or revoke one. kanna --host MACHINE COMMAND runs any command on one.",
    usage: "kanna machine [list]\n       kanna machine pair\n       kanna machine pair CODE [--peer KEY[@HOST[:PORT]]]\n       kanna machine name NAME\n       kanna machine ping MACHINE\n       kanna machine addr MACHINE HOST[:PORT]\n       kanna machine relay URL [--token-file FILE]|none\n       kanna machine revoke MACHINE\n       kanna --host MACHINE COMMAND ...",
    options: &[
        ("--peer KEY[@HOST[:PORT]]", "pair CODE: the machine to pair with, as `kanna machine pair` prints it there, when it isn't on this LAN."),
        ("--host MACHINE", "Before any command: run it on that machine (its name or key) instead of this one. KANNA_HOST does the same."),
    ],
    notes: "Each machine's daemon has a key of its own (device.key in its state folder) and listens for the others over iroh, on UDP port 7470 when it's free. Machines on a LAN find each other by themselves.\nTo add a machine, run `kanna machine pair` on it: it shows a six-digit code and waits. On one of your machines (or the first one, which starts them), run `kanna machine pair CODE`; Kanna.app there asks you to confirm, and the new machine joins. A code works once, for five minutes, and three wrong codes close it.\nA machine is known by its name, the first part of its host name unless `kanna machine name` changed it before pairing. A name that comes back with another key is refused; a revoked machine is never let back in, under any name, and has to be paired with a new key.\n`kanna machine addr` gives an address to try for a machine that isn't on this LAN, such as one on your VPN. ping opens a connection and one stream of each kind, and says whether the path is direct.\n`kanna machine relay URL` names the iroh-relay this machine meets the others through when they aren't on its LAN; run it on each machine, and the app dials them through it too. A relay that admits only holders of a secret (iroh-relay's access.shared_token) takes it with --token-file FILE; the token stays in this machine's kanna.db and is never put in the peer log. iroh moves to a direct path by itself whenever one opens. `none` goes back to the LAN and given addresses only.",
    examples: &[
        ("kanna machine list", "This machine, your other machines and how each is reached."),
        ("kanna machine pair", "On a new machine: show a code to pair it with."),
        ("kanna machine pair 042917", "On your Mac: add the machine showing 042917."),
        ("kanna machine ping r720-1", "Whether r720-1 answers, by which path, and that each kind of stream opens."),
        ("kanna machine addr r720-1 10.0.0.5", "Also try r720-1 at 10.0.0.5, port 7470."),
        ("kanna machine relay https://relay.example.com:8443", "Meet your other machines through your own relay when away."),
        ("kanna machine revoke old-laptop", "Shut old-laptop out of every machine for good."),
    ],
    parse,
    show,
    flow: Flow { online: Some(online), ..Flow::PLAIN },
};

fn online(request: &Value, mux: &mut Session, json_output: bool) -> Option<Result<(), Failure>> {
    (request["action"] == "offer").then(|| offer(mux, json_output))
}

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let peer = args.option("--peer")?;
    let token_file = args.option("--token-file")?;
    let action = args.positional();
    let machine = |what: &str, args: &mut Argv| args.positional().ok_or_else(|| usage("machine", &format!("machine {what} needs a MACHINE: its name or key")));
    let request = match action.as_deref() {
        None | Some("list") => json!({ "action": "status" }),
        Some("pair") => match args.positional() {
            None => json!({ "action": "offer" }),
            Some(code) => json!({ "action": "pair", "code": code, "peer": peer }),
        },
        Some("name") => json!({ "action": "name", "name": args.positional().ok_or_else(|| usage("machine", "machine name needs a NAME"))? }),
        Some(what @ ("ping" | "revoke" | "sync")) => json!({ "action": what, "host": machine(what, args)? }),
        Some("addr") => {
            let host = machine("addr", args)?;
            json!({ "action": "addr", "host": host, "addr": args.positional().ok_or_else(|| usage("machine", "machine addr needs a HOST or HOST:PORT"))? })
        }
        Some("relay") => match args.positional().ok_or_else(|| usage("machine", "machine relay needs a relay's URL, or none"))?.as_str() {
            "none" => json!({ "action": "relay", "url": null }),
            url => {
                let token = match &token_file {
                    Some(path) => Some(std::fs::read_to_string(path).map_err(|e| usage("machine", &format!("can't read the relay's token from {path}: {e}")))?.trim().to_string()),
                    None => None,
                };
                json!({ "action": "relay", "url": url, "token": token })
            }
        },
        Some(other) => return Err(usage("machine", &format!("unknown action \"{other}\" (list, pair, name, ping, addr, relay or revoke)"))),
    };
    if token_file.is_some() && (request["action"] != "relay" || request["url"].is_null()) {
        return Err(usage("machine", "--token-file goes with relay URL"));
    }
    if peer.is_some() && request["action"] != "pair" {
        return Err(usage("machine", "--peer goes with pair CODE"));
    }
    Ok(request)
}

pub fn show(request: &Value, reply: &Value) -> String {
    match request["action"].as_str() {
        Some("pair") => format!("added {} to your machines", reply["name"].as_str().unwrap_or("?")),
        Some("name") => format!("this machine is {} now", reply["name"].as_str().unwrap_or("?")),
        Some("addr") => format!("will try {} for {}", reply["addr"].as_str().unwrap_or("?"), request["host"].as_str().unwrap_or("?")),
        Some("sync") => format!("swapped peer logs with {}", reply["name"].as_str().unwrap_or("?")),
        Some("relay") => match reply["relay"].as_str() {
            Some(url) => format!("this machine meets the others through {url} when away"),
            None => "this machine uses no relay: only the LAN and addresses you give".into(),
        },
        Some("revoke") => {
            let told = join(&reply["told"]);
            let told = if told.is_empty() { "no other machine was reachable to hear it yet".to_string() } else { format!("told {told}") };
            let accounts = match reply["accounts"].as_bool() {
                Some(true) => "\nit dropped the agent accounts it was given; if it may be in someone else's hands, rotate them too (kanna accounts)",
                Some(false) => "\nit couldn't be told to drop the agent accounts it was given: rotate them (kanna accounts)",
                None => "",
            };
            format!("revoked {} for good; {told}{accounts}", reply["name"].as_str().unwrap_or("?"))
        }
        Some("ping") => {
            let streams = reply["streams"].as_object().map(|s| s.iter().map(|(kind, r)| format!("{kind} {}", if r["ok"] == true { "ok".to_string() } else { r["error"]["code"].as_str().unwrap_or("failed").to_string() })).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            let rtt = reply["rtt_ms"].as_f64().map_or(String::new(), |ms| format!(", {ms:.1} ms round trip"));
            format!("{}: {} to {}{rtt}\nstreams: {streams}", reply["name"].as_str().unwrap_or("?"), reply["path"].as_str().unwrap_or("no path"), reply["remote"].as_str().unwrap_or("?"))
        }
        _ => {
            let mut out = vec![];
            if let Some(broken) = reply["broken"].as_str() {
                out.push(format!("your machines are off here: {broken}"));
            }
            out.push(format!("{} (this machine)  key {}  UDP port {}", reply["name"].as_str().unwrap_or("?"), reply["key"].as_str().unwrap_or("?"), reply["port"]));
            if let Some(relay) = reply["relay"].as_str() {
                let state = if reply["home_relay"].as_str().is_some() { "connected" } else { "not connected yet" };
                out.push(format!("relay {relay} ({state})"));
            }
            let peers: Vec<&Value> = reply["peers"].as_array().map(|p| p.iter().filter(|p| p["me"] != true).collect()).unwrap_or_default();
            if reply["paired"] != true {
                out.push("not paired with any machine: kanna machine pair".into());
            } else if peers.is_empty() {
                out.push("no other machines yet".into());
            }
            for peer in peers {
                let state = if peer["revoked"] == true {
                    "revoked".to_string()
                } else if peer["conflict"] == true {
                    "name claimed by two keys".to_string()
                } else {
                    peer["path"].as_str().map_or("not connected".to_string(), String::from)
                };
                let runs = release(&peer["release"]);
                let runs = if runs.is_empty() { String::new() } else { format!("  Kanna {runs}") };
                out.push(format!("  {:<14} {:<14} {}  {}{runs}", peer["name"].as_str().unwrap_or("?"), state, short_key(&peer["key"]), join(&peer["addrs"])));
            }
            let phones: Vec<&Value> = reply["phones"].as_array().map(|p| p.iter().collect()).unwrap_or_default();
            if !phones.is_empty() {
                out.push("phones:".into());
            }
            for phone in phones {
                out.push(format!("  {:<14} {:<14} {}", phone["name"].as_str().unwrap_or("?"), if phone["revoked"] == true { "revoked" } else { "paired" }, short_key(&phone["key"])));
            }
            for nearby in reply["nearby"].as_array().into_iter().flatten().filter(|n| n["pairing"] == true) {
                out.push(format!("waiting to pair on this LAN: {}  {}", short_key(&nearby["key"]), join(&nearby["addrs"])));
            }
            if let Some(code) = reply["offer"]["code"].as_str() {
                out.push(format!("this machine is waiting to pair, code {code}"));
            }
            out.join("\n")
        }
    }
}

fn short_commit(commit: &str) -> String {
    let dirty = if commit.ends_with("-dirty") { "-dirty" } else { "" };
    format!("{}{dirty}", commit.chars().take(8).collect::<String>())
}

fn release(release: &Value) -> String {
    match (release["version"].as_str(), release["commit"].as_str()) {
        (Some(version), Some(commit)) => format!("{version} ({})", short_commit(commit)),
        (Some(version), None) => version.to_string(),
        _ => String::new(),
    }
}

fn short_key(key: &Value) -> String {
    key.as_str().unwrap_or("?").chars().take(10).collect()
}

pub fn others(addrs: &Value) -> String {
    let all: Vec<&str> = addrs.as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if all.len() < 2 {
        return String::new();
    }
    format!("(this machine's addresses: {}; put the one the other machine can reach after the @)\n", all.join(", "))
}

pub fn offer_text(offer: &Value) -> String {
    let code = offer["code"].as_str().unwrap_or("?");
    let minutes = offer["expires_in"].as_u64().unwrap_or(300).div_ceil(60);
    let peer = match reachable(&offer["addrs"]) {
        Some(addr) => format!("{}@{addr}", offer["key"].as_str().unwrap_or("?")),
        None => offer["key"].as_str().unwrap_or("?").to_string(),
    };
    format!(
        "To add this machine ({}) to your machines, run on one of them, or on the one that starts them:\n  kanna machine pair {code}\nIt finds this one on the LAN. From another network:\n  kanna machine pair {code} --peer {peer}\n{}The code works once, for {minutes} minutes. Waiting…",
        offer["name"].as_str().unwrap_or("?"),
        others(&offer["addrs"])
    )
}

pub fn offer(mux: &mut Session, json_output: bool) -> Result<(), Failure> {
    let offer = mux.call("machine", json!({ "action": "offer" }))?;
    if json_output {
        println!("{offer}");
    } else {
        println!("{}", offer_text(&offer));
    }
    let paired = mux.call("machine", json!({ "action": "wait" }))?;
    if json_output {
        println!("{paired}");
    } else {
        println!("paired: {} joined your machines through {}", offer["name"].as_str().unwrap_or("this machine"), paired["name"].as_str().unwrap_or("?"));
    }
    Ok(())
}

pub fn reachable(addrs: &Value) -> Option<String> {
    let all: Vec<&str> = addrs.as_array()?.iter().filter_map(Value::as_str).collect();
    let lan = |a: &&&str| a.starts_with("192.168.") || a.starts_with("10.") || a.starts_with("172.");
    all.iter().find(lan).or_else(|| all.iter().find(|a| !a.starts_with("127.") && !a.starts_with('['))).map(|a| a.to_string())
}
