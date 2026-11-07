use kanna_net::{hex, read_key, unhex, Client, LineRecv, LineSend, Relay, SecretKey};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn usage() -> ! {
    eprintln!("usage: phone-client DIR pair OFFER-JSON\n       phone-client DIR hello\n       phone-client DIR path\n       phone-client DIR read REPO [AFTER]\n       phone-client DIR send REPO ID TEXT\n       phone-client DIR choose REPO ID SEQ CHOICE\n       phone-client DIR wait REPO KIND SECONDS [AFTER]\n       phone-client DIR status REPO STATUS SECONDS\n       phone-client DIR listen SECONDS\n       phone-client DIR tasks REPO\n       phone-client DIR task TASK\n       phone-client DIR tell TASK ID TEXT\n       phone-client DIR choosetask TASK ID SEQ CHOICE\n       phone-client DIR pick TASK DESK [ROOM]\n       phone-client DIR machines REPO\n       phone-client DIR stats REPO\n       phone-client DIR move REPO MACHINE\n       phone-client DIR quiz-map REPO\n       phone-client DIR quiz-rounds REPO\n       phone-client DIR quiz-answer ROUND N PICK\n       phone-client DIR quiz-ahead REPO [ABOUT]\n       phone-client DIR quiz-again REPO\n\nPHONE_MACHINE=NAME opens the link to that machine of the last hello's, not the paired one. PHONE_RELAY_ONLY=1 reaches it only through the relay; PHONE_RELAY_TOKEN gives another token.");
    std::process::exit(2)
}

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("phone-client: {message}");
    std::process::exit(1)
}

fn machine(dir: &Path) -> Value {
    let text = std::fs::read_to_string(dir.join("machine.json")).unwrap_or_else(|e| fail(format!("not paired: {e}")));
    let paired: Value = serde_json::from_str(&text).unwrap_or_else(|e| fail(e));
    let Ok(wanted) = std::env::var("PHONE_MACHINE") else { return paired };
    let known = std::fs::read_to_string(dir.join("machines.json")).unwrap_or_else(|e| fail(format!("no hello yet: {e}")));
    let known: Value = serde_json::from_str(&known).unwrap_or_else(|e| fail(e));
    known.as_array().into_iter().flatten().find(|m| m["name"] == wanted.as_str()).cloned().unwrap_or_else(|| fail(format!("no machine {wanted} in {known}")))
}

struct Link {
    send: LineSend,
    recv: LineRecv,
    next: u64,
    notices: Vec<Value>,
}

impl Link {
    async fn open(client: &Client, dir: &Path) -> Link {
        let machine = machine(dir);
        let relay = relay_of(dir);
        let addrs: Vec<String> = machine["addrs"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        let (send, recv, _) = client.lines(machine["key"].as_str().unwrap_or_default(), &addrs, relay.as_ref(), json!({ "kind": "phone", "phone": kanna_net::phone::VERSION })).await.unwrap_or_else(|f| fail(format!("{} [{}]", f.message, f.code)));
        Link { send, recv, next: 1, notices: vec![] }
    }

    async fn call(&mut self, cmd: &str, args: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send.write(&json!({ "id": id, "cmd": cmd, "args": args })).await.unwrap_or_else(|f| fail(f.message));
        loop {
            let line = tokio::time::timeout(Duration::from_secs(15), self.recv.read()).await.unwrap_or_else(|_| fail(format!("no reply to {cmd}"))).unwrap_or_else(|f| fail(f.message));
            if line["id"] == id {
                return line;
            }
            self.notices.push(line);
        }
    }

    async fn next_line(&mut self, limit: Duration) -> Option<Value> {
        match self.notices.is_empty() {
            false => Some(self.notices.remove(0)),
            true => tokio::time::timeout(limit, self.recv.read()).await.ok()?.ok(),
        }
    }
}

fn relay_of(dir: &Path) -> Option<Relay> {
    let text = std::fs::read_to_string(dir.join("machine.json")).ok()?;
    let paired: Value = serde_json::from_str(&text).ok()?;
    let token = std::env::var("PHONE_RELAY_TOKEN").ok().or_else(|| paired["relay_token"].as_str().map(String::from));
    Relay::new(paired["relay"].as_str()?, token.as_deref()).ok()
}

fn refresh_relay(dir: &Path, hello: &Value) {
    let Ok(text) = std::fs::read_to_string(dir.join("machine.json")) else { return };
    let Ok(mut paired) = serde_json::from_str::<Value>(&text) else { return };
    if hello["relay"].is_string() {
        paired["relay"] = hello["relay"].clone();
        paired["relay_token"] = hello["relay_token"].clone();
        std::fs::write(dir.join("machine.json"), paired.to_string()).unwrap_or_else(|e| fail(e));
    }
}

fn own_key(dir: &Path) -> SecretKey {
    read_key(&dir.join("device.key")).unwrap_or_else(|e| fail(e))
}

fn repo_of(hello: &Value, wanted: &str) -> String {
    let projects = hello["projects"].as_array().cloned().unwrap_or_default();
    projects.iter().find(|p| p["repo"] == wanted || p["name"] == wanted).and_then(|p| p["repo"].as_str().map(String::from)).unwrap_or_else(|| fail(format!("no project {wanted} in {hello}")))
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (dir, action) = match args.as_slice() {
        [dir, action, ..] => (PathBuf::from(dir), action.as_str()),
        _ => usage(),
    };
    let rest = &args[2..];
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| fail(e));
    let secret = own_key(&dir);
    let started = if std::env::var_os("PHONE_RELAY_ONLY").is_some() { Client::start_relay_only(secret.clone()).await } else { Client::start(secret.clone()).await };
    let client = started.unwrap_or_else(|e| fail(e));
    if action == "instance" {
        println!("{}", kanna_net::phone::instance_of(&hex(&unhex(rest.first().map(String::as_str).unwrap_or_default()).unwrap_or_default())).unwrap_or_default());
        return;
    }
    if action == "pair" {
        let offer: Value = serde_json::from_str(rest.first().unwrap_or_else(|| usage())).unwrap_or_else(|e| fail(e));
        let joined = kanna_net::phone::join(&client, &secret, &offer).await.unwrap_or_else(|f| fail(format!("{} [{}]", f.message, f.code)));
        std::fs::write(dir.join("machine.json"), joined.to_string()).unwrap_or_else(|e| fail(e));
        println!("{joined}");
        return;
    }
    let mut link = Link::open(&client, &dir).await;
    let hello = link.call("hello", json!({})).await;
    std::fs::write(dir.join("machines.json"), hello["machines"].to_string()).unwrap_or_else(|e| fail(e));
    refresh_relay(&dir, &hello);
    match (action, rest) {
        ("hello", _) => println!("{hello}"),
        ("path", _) => println!("{}", client.path(hello["key"].as_str().unwrap_or_default())),
        ("read", [repo, after @ ..]) => {
            let after: i64 = after.first().and_then(|a| a.parse().ok()).unwrap_or(0);
            println!("{}", link.call("chat.read", json!({ "repo": repo_of(&hello, repo), "after": after })).await);
        }
        ("send", [repo, id, text]) => println!("{}", link.call("chat.send", json!({ "repo": repo_of(&hello, repo), "id": id, "text": text })).await),
        ("choose", [repo, id, seq, choice]) => println!("{}", link.call("chat.choose", json!({ "repo": repo_of(&hello, repo), "id": id, "seq": seq.parse::<i64>().unwrap_or(0), "choice": choice })).await),
        ("wait", [repo, kind, seconds, after @ ..]) => {
            let repo = repo_of(&hello, repo);
            let after: i64 = after.first().and_then(|a| a.parse().ok()).unwrap_or(0);
            let end = std::time::Instant::now() + Duration::from_secs(seconds.parse().unwrap_or(10));
            while std::time::Instant::now() < end {
                let read = link.call("chat.read", json!({ "repo": repo, "after": after })).await;
                if let Some(item) = read["items"].as_array().into_iter().flatten().find(|i| i["kind"] == kind.as_str()) {
                    println!("{item}");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            fail(format!("no {kind} item in time"))
        }
        ("status", [repo, status, seconds]) => {
            let repo = repo_of(&hello, repo);
            let end = std::time::Instant::now() + Duration::from_secs(seconds.parse().unwrap_or(10));
            let mut last = Value::Null;
            while std::time::Instant::now() < end {
                last = link.call("chat.read", json!({ "repo": repo, "after": 1_000_000 })).await;
                if last["status"] == status.as_str() {
                    println!("{status}");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            fail(format!("status stayed {}", last["status"]))
        }
        ("tasks", [repo]) => println!("{}", link.call("tasks.list", json!({ "repo": repo_of(&hello, repo) })).await),
        ("task", [task]) => println!("{}", link.call("task.read", json!({ "task": task })).await),
        ("tell", [task, id, text]) => println!("{}", link.call("task.send", json!({ "task": task, "id": id, "text": text })).await),
        ("choosetask", [task, id, seq, choice]) => println!("{}", link.call("task.choose", json!({ "task": task, "id": id, "seq": seq.parse::<i64>().unwrap_or(0), "choice": choice })).await),
        ("pick", [task, desk, room @ ..]) => println!("{}", link.call("task.pick", json!({ "task": task, "desk": desk, "room": room.first() })).await),
        ("quiz-map", [repo]) => println!("{}", link.call("quiz.map", json!({ "repo": repo_of(&hello, repo) })).await),
        ("quiz-rounds", [repo]) => println!("{}", link.call("quiz.rounds", json!({ "repo": repo_of(&hello, repo) })).await),
        ("quiz-answer", [round, n, pick]) => println!("{}", link.call("quiz.answer", json!({ "round": round, "n": n.parse::<i64>().unwrap_or(0), "pick": pick.parse::<i64>().unwrap_or(0) })).await),
        ("quiz-ahead", [repo, about @ ..]) => println!("{}", link.call("quiz.ahead", json!({ "repo": repo_of(&hello, repo), "about": about.first() })).await),
        ("quiz-again", [repo]) => println!("{}", link.call("quiz.ahead", json!({ "repo": repo_of(&hello, repo), "again": true })).await),
        ("machines", [repo]) => println!("{}", link.call("machines", json!({ "repo": repo_of(&hello, repo) })).await),
        ("stats", [repo]) => println!("{}", link.call("stats", json!({ "repo": repo_of(&hello, repo) })).await),
        ("move", [repo, machine]) => println!("{}", link.call("planner.move", json!({ "repo": repo_of(&hello, repo), "machine": machine })).await),
        ("listen", [seconds]) => {
            let end = std::time::Instant::now() + Duration::from_secs(seconds.parse().unwrap_or(10));
            while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
                match link.next_line(left).await {
                    Some(line) => println!("{line}"),
                    None => break,
                }
            }
        }
        _ => usage(),
    }
}
