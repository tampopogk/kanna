use serde_json::Value;

pub fn join(list: &Value) -> String {
    list.as_array().map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

pub fn label(pane: &Value) -> String {
    let id = pane["id"].as_str().unwrap_or("?");
    match pane["name"].as_str() {
        Some(name) => format!("{id} ({name})"),
        None => id.to_string(),
    }
}

pub fn state(pane: &Value) -> String {
    let state = pane["state"].as_str().unwrap_or("?");
    match (pane["exitCode"].as_i64(), pane["error"].as_str()) {
        (Some(code), _) => format!("{state} ({code})"),
        (_, Some(error)) => format!("{state}: {error}"),
        _ => state.to_string(),
    }
}

pub fn whose(reply: &Value) -> Option<(String, Vec<String>)> {
    let workspace = reply.get("workspace").filter(|w| w.is_object())?;
    let id = workspace["id"].as_str().unwrap_or("?");
    let place = &workspace["where"];
    let mut tags: Vec<String> = workspace["task"].as_str().map(String::from).into_iter().collect();
    tags.push(place_named(place, workspace["desk"].as_str()));
    let name = match reply["window"].as_str() {
        Some(window) => format!("{id}'s {window}"),
        None => id.to_string(),
    };
    Some((name, tags))
}

pub fn place_of(place: &Value) -> &'static str {
    match place {
        Value::String(place) if place == "shelf" => "on the Shelf",
        place if place.get("planner").is_some() => "a Desk's planner",
        _ => "on the Desk",
    }
}

pub fn place_named(place: &Value, desk: Option<&str>) -> String {
    match (place.get("planner"), desk) {
        (Some(_), Some(desk)) => format!("{desk}'s planner"),
        _ => place_of(place).into(),
    }
}

pub fn tagged(name: &str, tags: &[String]) -> String {
    if tags.is_empty() { name.to_string() } else { format!("{name} ({})", tags.join(", ")) }
}

pub fn hidden(reply: &Value) -> bool {
    reply["window"].is_null()
}

pub fn whose_text(reply: &Value) -> Option<String> {
    whose(reply).map(|(name, tags)| tagged(&name, &tags))
}

pub fn moved(reply: &Value) -> String {
    if reply["background"] == true {
        return format!("{} is in the background of {}", reply["pane"].as_str().unwrap_or("?"), whose_text(reply).unwrap_or_else(|| "its workspace".into()));
    }
    let dock = &reply["dock"];
    if dock.is_object() {
        return format!(
            "{} is docked {} in {} (width {})",
            dock["pane"].as_str().unwrap_or("?"),
            dock["edge"].as_str().unwrap_or("?"),
            reply["workspace"]["id"].as_str().or(reply["window"].as_str()).unwrap_or("?"),
            dock["width"].as_str().unwrap_or("?")
        );
    }
    layout(reply)
}

pub fn shorts(value: &Value) -> Vec<(String, String)> {
    match value {
        Value::Object(map) => {
            let own = map.get("id").and_then(Value::as_str).zip(map.get("short").and_then(Value::as_str)).map(|(id, short)| (id.to_string(), short.to_string()));
            own.into_iter().chain(map.values().flat_map(shorts)).collect()
        }
        Value::Array(list) => list.iter().flat_map(shorts).collect(),
        _ => vec![],
    }
}

pub fn shorten(value: &mut Value, shorts: &[(String, String)]) {
    match value {
        Value::String(text) => *text = short_text(text, shorts),
        Value::Array(list) => list.iter_mut().for_each(|v| shorten(v, shorts)),
        Value::Object(map) => map.iter_mut().filter(|(key, _)| !["folder", "branch", "cwd", "path", "repo"].contains(&key.as_str())).for_each(|(_, v)| shorten(v, shorts)),
        _ => {}
    }
}

pub fn short_text(text: &str, shorts: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (id, short) in shorts.iter().filter(|(id, short)| id != short) {
        let mut from = 0;
        while let Some(at) = out[from..].find(id.as_str()).map(|at| at + from) {
            if out[..at].ends_with('/') {
                from = at + id.len();
                continue;
            }
            out.replace_range(at..at + id.len(), short);
            from = at + short.len();
        }
    }
    out
}

pub fn tilde(path: &str) -> String {
    match std::env::var("HOME").ok().filter(|h| !h.is_empty()) {
        Some(home) if path == home => "~".into(),
        Some(home) if path.starts_with(&format!("{home}/")) => format!("~{}", &path[home.len()..]),
        _ => path.into(),
    }
}

pub fn ago(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor().max(0.0) as u64;
    match minutes {
        0 => "just now".into(),
        m if m < 60 => format!("{m}m ago"),
        m if m < 60 * 24 => format!("{}h ago", m / 60),
        m => format!("{}d ago", m / 60 / 24),
    }
}

pub fn what(pane: &Value) -> Option<String> {
    if let Some(app) = pane["app"].as_str() {
        return Some(match pane["device"].as_str() {
            Some(device) => format!("{app} on {device}"),
            None => app.into(),
        });
    }
    if let Some(path) = pane["path"].as_str() {
        return Some(match std::env::var("HOME") {
            Ok(home) if !home.is_empty() && path.starts_with(&format!("{home}/")) => format!("~{}", &path[home.len()..]),
            _ => path.into(),
        });
    }
    pane["cmd"].as_str().or(pane["url"].as_str()).map(String::from)
}

pub fn pane_state(reply: &Value) -> String {
    let pane = &reply["pane"];
    let what = what(pane).map(|c| format!(" ({c})")).unwrap_or_default();
    format!("{}{what} is {}", label(pane), state(pane))
}

pub fn layout(reply: &Value) -> String {
    expression(&reply["layout"], true)
}

pub fn expression(node: &Value, top: bool) -> String {
    if let Some(pane) = node["pane"].as_str() {
        return sized(pane.to_string(), node);
    }
    let separator = if node["split"] == "column" { " / " } else { " | " };
    let inner = node["children"].as_array().map(|kids| kids.iter().map(|k| expression(k, false)).collect::<Vec<_>>().join(separator)).unwrap_or_default();
    if top { inner } else { sized(format!("({inner})"), node) }
}

fn sized(text: String, node: &Value) -> String {
    match node["size"].as_str() {
        Some(size) => format!("{text}:{size}"),
        None => text,
    }
}

pub fn agents(args: &Value, reply: &Value) -> String {
    let lines = |key: &str, label: &str| reply[key].as_array().filter(|a| !a.is_empty()).map(|a| format!("{label}:\n{}", a.iter().filter_map(Value::as_str).map(|p| format!("  {}", tilde(p))).collect::<Vec<_>>().join("\n")));
    let changes = changed_skills(reply).map(|c| format!("{c}\n"));
    let conflicts = reply["conflicts"].as_array().filter(|a| !a.is_empty()).map(|a| format!("your changes conflict with Kanna's, so these are as you left them (kanna agents update to merge them):\n{}", a.iter().filter_map(|c| c["path"].as_str()).map(|p| format!("  {}", tilde(p))).collect::<Vec<_>>().join("\n")));
    match args["action"].as_str() {
        Some("setup") => [
            Some("Kanna's skills are installed; Kanna asks before it updates them".to_string()),
            changes.map(|c| c.trim_end().to_string()),
            lines("kept", "left alone (yours; kanna agents update --force puts Kanna's back)"),
            conflicts,
            (reply["settings"] != true).then(|| "~/.claude/settings.json isn't JSON, so Claude's hook and sandbox entry weren't added".to_string()),
        ].into_iter().flatten().collect::<Vec<_>>().join("\n"),
        Some("update") => [
            Some(if changes.is_some() { "updated Kanna's skills".to_string() } else { "Kanna's skills are up to date".to_string() }),
            changes.map(|c| c.trim_end().to_string()),
            lines("kept", "left alone (yours; --force puts Kanna's back)"),
            conflicts,
        ].into_iter().flatten().collect::<Vec<_>>().join("\n"),
        Some("remove") => [
            Some("took Kanna's skills and sandbox entry back out; Kanna won't ask again (kanna agents setup to set them up again)".to_string()),
            lines("kept", "left alone (changed since Kanna installed them)"),
        ].into_iter().flatten().collect::<Vec<_>>().join("\n"),
        _ => {
            let skills = reply["skills"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            let state = match reply["setup"].as_str() {
                Some("yes") => match changed_skills(reply) {
                    Some(changes) => format!("set up; this Kanna has newer skills (kanna agents update):\n{changes}"),
                    None => "set up, and up to date".to_string(),
                },
                Some("no") => "not set up (you said no), so agents don't name their tasks, plan or answer comments: kanna agents setup".to_string(),
                _ => "not set up yet, so agents don't name their tasks, plan or answer comments; Kanna's planning terminal asks, or: kanna agents setup".to_string(),
            };
            format!("{state}\nskills: {}", if skills.is_empty() { "none in this Kanna".into() } else { skills })
        }
    }
}

pub fn changed_skills(reply: &Value) -> Option<String> {
    let changes = reply["changes"].as_object().filter(|c| !c.is_empty())?;
    let width = changes.keys().map(|k| k.chars().count()).max().unwrap_or(0);
    Some(changes.iter().map(|(skill, change)| {
        let change = match change.as_str().unwrap_or("") {
            "merged" => "updated, merged with your changes",
            "conflict" => "updated; your changes conflict, kept yours",
            other => other,
        };
        format!("  {skill:width$}  {change}")
    }).collect::<Vec<_>>().join("\n"))
}

pub fn home_relative(path: &str) -> String {
    match std::env::var("HOME").ok().filter(|h| !h.is_empty()).and_then(|home| path.strip_prefix(&home).map(String::from)) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

pub fn offer(offer: &Value) -> String {
    let code = offer["code"].as_str().unwrap_or("?");
    let minutes = offer["expires_in"].as_u64().unwrap_or(300).div_ceil(60);
    let peer = match crate::machine::reachable(&offer["addrs"]) {
        Some(addr) => format!("{}@{addr}", offer["key"].as_str().unwrap_or("?")),
        None => offer["key"].as_str().unwrap_or("?").to_string(),
    };
    format!(
        "To add this machine ({}) to your machines, run on one of them, or on the one that starts them:\n  kanna machine pair {code}\nIt finds this one on the LAN. From another network:\n  kanna machine pair {code} --peer {peer}\n{}The code works once, for {minutes} minutes. Waiting…",
        offer["name"].as_str().unwrap_or("?"),
        others(&offer["addrs"])
    )
}

pub fn others(addrs: &Value) -> String {
    let all: Vec<&str> = addrs.as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if all.len() < 2 {
        return String::new();
    }
    format!("(this machine's addresses: {}; put the one the other machine can reach after the @)\n", all.join(", "))
}

fn short_key(key: &Value) -> String {
    key.as_str().unwrap_or("?").chars().take(10).collect()
}

pub fn accounts(request: &Value, reply: &Value) -> String {
    let what = |a: &Value| {
        let kind = match a["kind"].as_str() {
            Some("claude-subscription") => "Claude subscription",
            Some("claude-api-key") => "Anthropic API key",
            Some("codex-chatgpt") => "Codex, ChatGPT login",
            Some("codex-api-key") => "OpenAI API key",
            _ => "account",
        };
        match (a["label"].as_str().filter(|l| Some(*l) != a["kind"].as_str()), a["holder_name"].as_str()) {
            (Some(label), Some(holder)) => format!("{kind} ({label}), held by {holder}"),
            (Some(label), None) => format!("{kind} ({label})"),
            (None, Some(holder)) => format!("{kind}, held by {holder}"),
            (None, None) => kind.to_string(),
        }
    };
    let machines = join(&reply["machines"]);
    match request["action"].as_str() {
        Some("add") => {
            let replaced = reply["replaced"].as_array().filter(|r| !r.is_empty()).map_or(String::new(), |r| format!(" in place of {}", r.iter().map(&what).collect::<Vec<_>>().join(", ")));
            let there = if machines.is_empty() { String::new() } else { format!(" and on {machines}") };
            format!("added {}{replaced} ({}); your agents use it here{there}", what(&reply["account"]), reply["account"]["id"].as_str().unwrap_or("?"))
        }
        Some("remove") => {
            let gone = reply["removed"].as_array().map(|r| r.iter().map(&what).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            if machines.is_empty() { format!("removed {gone}") } else { format!("removed {gone}; it comes off {machines} as each one hears it") }
        }
        _ => {
            let mut out = vec![];
            for account in reply["accounts"].as_array().into_iter().flatten() {
                let broken = account["broken"].as_str().map_or(String::new(), |why| format!("  broken: {why}"));
                out.push(format!("{:<7} {}  {}{broken}", account["agent"].as_str().unwrap_or("?"), account["id"].as_str().unwrap_or("?"), what(account)));
            }
            for account in reply["received"].as_array().into_iter().flatten() {
                out.push(format!("{:<7} {}  {}, from {}", account["agent"].as_str().unwrap_or("?"), account["id"].as_str().unwrap_or("?"), what(account), account["from"].as_str().unwrap_or("?")));
            }
            if out.is_empty() {
                out.push("no accounts: your agents use each machine's own sign-in (kanna accounts add claude, or codex)".into());
            } else if reply["accounts"].as_array().is_some_and(|a| !a.is_empty()) {
                out.push(if machines.is_empty() { "no other machine to go to yet".to_string() } else { format!("going to: {machines}") });
            }
            out.join("\n")
        }
    }
}

fn release(release: &Value) -> String {
    match (release["version"].as_str(), release["commit"].as_str()) {
        (Some(version), Some(commit)) => format!("{version} ({})", short_commit(commit)),
        (Some(version), None) => version.to_string(),
        _ => String::new(),
    }
}

fn short_commit(commit: &str) -> String {
    let dirty = if commit.ends_with("-dirty") { "-dirty" } else { "" };
    format!("{}{dirty}", commit.chars().take(8).collect::<String>())
}

pub fn machine(request: &Value, reply: &Value) -> String {
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
