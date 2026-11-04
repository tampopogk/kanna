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

pub fn opened(request: &Value, reply: &Value) -> String {
    let place = whose_text(reply).unwrap_or_else(|| reply["window"].as_str().unwrap_or("?").to_string());
    if reply["background"] == true {
        return format!("opened {} in the background of {place}: {}", label(&reply["pane"]), state(&reply["pane"]));
    }
    if let Some(edge) = reply["dock"].as_str() {
        let frozen = if request["frozen"] == true { ", frozen," } else { "" };
        return format!("opened {} docked {edge}{frozen} in {place}: {}", label(&reply["pane"]), state(&reply["pane"]));
    }
    format!("opened {} in {place}, {}: {}", label(&reply["pane"]), reply["tab"].as_str().unwrap_or("?"), state(&reply["pane"]))
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

pub fn task(request: &Value, reply: &Value) -> String {
    let id = reply["task"].as_str().unwrap_or("?");
    match request["action"].as_str() {
        Some("list") => {
            let tasks = reply["tasks"].as_array().cloned().unwrap_or_default();
            let waiting = reply["shelf"].as_array().cloned().unwrap_or_default();
            let done = reply["done"].as_array().cloned().unwrap_or_default();
            let elsewhere = reply["elsewhere"].as_array().cloned().unwrap_or_default();
            if let (true, Some(query)) = (tasks.is_empty() && waiting.is_empty() && done.is_empty() && elsewhere.is_empty(), request["query"].as_str()) {
                return match &reply["found"] {
                    found if found["place"] == "done" => done_line(found),
                    _ => format!("no task matches \"{query}\""),
                };
            }
            if tasks.is_empty() && waiting.is_empty() && done.is_empty() && elsewhere.is_empty() {
                return "no tasks: add one with `kanna task new NAME \"FIRST MESSAGE\"`".into();
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            let script = |t: &Value| t["script"].as_str().or_else(|| t["id"].as_str().and_then(|id| reply["script"][id].as_str())).map(String::from);
            let state = |t: &Value| match (t["running"].as_bool(), t["status"].as_str()) {
                _ if script(t).is_some() => script(t).unwrap_or_default(),
                (Some(true) | None, Some("needs-you")) => "needs you".to_string(),
                (Some(true) | None, Some("working")) => "working".to_string(),
                _ if t["merged"] == true => "merged".to_string(),
                _ if listed(&t["stale"]) => "stale".to_string(),
                _ if t["ready"].is_object() => "ready".to_string(),
                _ if t["replied"] == true => "replied".to_string(),
                (Some(false), _) if !ids(&t["waits"]).is_empty() || t["conflict"].is_string() => "blocked".to_string(),
                (Some(false), _) => "stopped".to_string(),
                (_, Some(status)) if status != "exited" => status.to_string(),
                _ => "running".to_string(),
            };
            let program = |command: &str| command.split_whitespace().next().map(|a| a.rsplit('/').next().unwrap_or(a).to_string()).unwrap_or_default();
            let default = Some(program(reply["agent"].as_str().unwrap_or("claude")));
            let agent = |t: &Value| Some(program(t["agent"].as_str().unwrap_or(""))).filter(|a| !a.is_empty() && Some(a) != default.as_ref());
            let here = request["repo"].as_str();
            let repo = |t: &Value| t["repo"].as_str().filter(|r| here != Some(*r)).map(tilde);
            let waits = |t: &Value| {
                let named: Vec<String> = t["waits"].as_array().into_iter().flatten().filter_map(|w| w["name"].as_str().map(|n| format!("\"{n}\"")).or_else(|| w["id"].as_str().map(String::from))).collect();
                Some(named.join(", ")).filter(|w| !w.is_empty() && t["running"] == false).map(|w| format!("waits for {w}"))
            };
            let conflict = |t: &Value| t["conflict"].as_str().filter(|_| t["running"] == false).map(|_| "its blockers' branches conflict".to_string());
            let stacked = |t: &Value| match (names(&t["stale"]), names(&t["bases"])) {
                (stale, _) if !stale.is_empty() => Some(format!("behind {stale}")),
                (_, bases) if !bases.is_empty() => Some(format!("merges after {bases}")),
                _ => None,
            };
            let twice = |id: &str| {
                let machines = reply["twice"].as_object()?.iter().find(|(task, _)| task.starts_with(id))?.1.as_array()?.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ");
                Some(format!("ran twice: {machines}"))
            };
            let row = |t: &Value, id: &str, last: Option<String>| -> [String; 4] {
                let tail: Vec<String> = [agent(t), repo(t), waits(t).or_else(|| conflict(t)).or_else(|| stacked(t)).or(last), twice(id)].into_iter().flatten().collect();
                [id.into(), t["name"].as_str().unwrap_or("").into(), state(t), tail.join("  ")]
            };
            let desk: Vec<[String; 4]> = tasks.iter().map(|t| row(t, t["task"].as_str().or(t["id"].as_str()).unwrap_or("?"), t["leftAt"].as_f64().map(|at| ago(now - at)))).collect();
            let shelf: Vec<[String; 4]> = waiting.iter().map(|t| row(t, t["id"].as_str().unwrap_or("?"), None)).collect();
            let others: Vec<[String; 4]> = elsewhere.iter().map(|t| {
                let mut row = row(t, t["id"].as_str().unwrap_or("?"), t["machine"].as_str().map(|m| format!("on {m}")));
                row[2] = t["state"].as_str().unwrap_or("").into();
                row
            }).collect();
            let finished: Vec<[String; 4]> = done.iter().map(|t| {
                let tail: Vec<String> = [agent(t), repo(t), t["doneAt"].as_f64().map(|at| format!("done {}", ago(now - at)))].into_iter().flatten().collect();
                [t["id"].as_str().unwrap_or("?").into(), t["name"].as_str().unwrap_or("").into(), String::new(), tail.join("  ")]
            }).collect();
            let all: Vec<&[String; 4]> = desk.iter().chain(shelf.iter()).chain(others.iter()).chain(finished.iter()).collect();
            let width = |i: usize| all.iter().map(|r| r[i].chars().count()).max().unwrap_or(0);
            let line = |r: &[String; 4]| format!("{:w0$}  {:w1$}  {:w2$}  {}", r[0], r[1], r[2], r[3], w0 = width(0), w1 = width(1), w2 = width(2)).trim_end().to_string();
            let section = |head: &str, rows: &[[String; 4]]| match (rows.is_empty(), request["query"].is_string()) {
                (true, true) => None,
                (true, false) => Some(format!("{head}\n(empty)")),
                (false, _) => Some(std::iter::once(head.to_string()).chain(rows.iter().map(line)).collect::<Vec<_>>().join("\n")),
            };
            let more = reply["moreDone"].as_u64().map(|n| format!("\n… {n} more done: add words to search them"));
            let finished = Some(&finished).filter(|_| request["done"] == true).and_then(|rows| section("Done", rows)).map(|s| s + more.as_deref().unwrap_or(""));
            [section("Desk", &desk), section("Shelf", &shelf), Some(&others).filter(|rows| !rows.is_empty()).and_then(|rows| section("Other machines", rows)), finished].into_iter().flatten().collect::<Vec<_>>().join("\n\n")
        }
        Some("park") if reply["parked"] == Value::Bool(true) => format!("parked {id}: {} shows the Desk view, and {id} stays active", reply["desk"].as_str().unwrap_or("its Desk")),
        Some("park") => format!("{id} isn't shown: its Desk shows the Desk view already"),
        Some("activate") if reply["moved"] == Value::Bool(true) => format!("{id} is active on {}", reply["desk"].as_str().unwrap_or("the Desk")),
        Some("activate") => format!("{id} is on {}; brought it forward", reply["desk"].as_str().unwrap_or("a Desk")),
        Some("new") => format!(
            "added {} \"{}\" to the Shelf of {}{}",
            reply["task"]["id"].as_str().unwrap_or("?"),
            reply["task"]["name"].as_str().unwrap_or(""),
            tilde(reply["task"]["repo"].as_str().unwrap_or("?")),
            Some(ids(&reply["task"]["waits"])).filter(|w| !w.is_empty()).map_or(String::new(), |w| format!("; it waits for {w}"))
        ),
        Some("edit") => match ids(&reply["task"]["waits"]) {
            waits if waits.is_empty() => format!("{} waits for nothing: it can start", reply["task"]["id"].as_str().unwrap_or("?")),
            waits => format!("{} waits for {waits}", reply["task"]["id"].as_str().unwrap_or("?")),
        },
        Some("start") if reply["resumed"] == Value::Bool(true) => format!("started {} again{}: {}", reply["task"].as_str().unwrap_or("?"), place(reply), reply["pane"].as_str().unwrap_or("?")),
        Some("start") => format!(
            "started {} on the Shelf: {} in {}{}",
            reply["task"].as_str().unwrap_or("?"),
            reply["pane"].as_str().unwrap_or("?"),
            tilde(reply["folder"].as_str().unwrap_or("?")),
            reply["branch"].as_str().map_or(String::new(), |b| format!(" ({b})"))
        ),
        Some("ready") => format!("{} is ready at {}{}{}", reply["task"]["id"].as_str().unwrap_or("?"), reply["task"]["ready"]["head"].as_str().map_or("?", |h| &h[..h.len().min(9)]), reply["task"]["ready"]["branch"].as_str().filter(|b| !b.is_empty()).map_or(String::new(), |b| format!(" ({b})")), freed_lines(reply)),
        Some("unready") => format!("{} isn't ready", reply["task"]["id"].as_str().unwrap_or("?")),
        Some("setup") if reply["tab"].is_string() => format!("{id}'s setup runs again in {} (a Setup tab)", reply["pane"].as_str().unwrap_or("?")),
        Some("setup") => format!("{id}'s setup runs again in {}, then its agent", reply["pane"].as_str().unwrap_or("?")),
        Some("move") if reply["waiting"] == true => format!("{} moves to {} when it's reachable ({})", reply["task"].as_str().unwrap_or("?"), reply["to"].as_str().unwrap_or("?"), reply["why"].as_str().unwrap_or("away")),
        Some("move") => format!("{} carries on on {}{}", reply["task"].as_str().unwrap_or("?"), reply["to"].as_str().unwrap_or("?"), reply["folder"].as_str().map_or(String::new(), |f| format!(", in {f}"))),
        Some("stop") => format!("stopped {}{}; `kanna task start {}` starts it again", reply["task"].as_str().unwrap_or("?"), place(reply), reply["task"].as_str().unwrap_or("?")),
        Some("pick") => format!(
            "moved {} to the Desk: {} in {}{}",
            reply["task"].as_str().unwrap_or("?"),
            reply["pane"].as_str().unwrap_or("?"),
            tilde(reply["folder"].as_str().unwrap_or("?")),
            reply["branch"].as_str().map_or(String::new(), |b| format!(" ({b})"))
        ),
        Some("done") if reply["teardown"] == Value::Bool(true) => format!("{}'s teardown runs in {}: it's done when that succeeds, and kanna task list says if it fails", reply["task"].as_str().unwrap_or("?"), reply["pane"].as_str().unwrap_or("?")),
        Some("done") if reply.get("closed").is_none() => format!("took {} off the Shelf{}", reply["task"]["id"].as_str().unwrap_or("?"), freed_lines(reply)),
        Some("desk") => format!("{} {} on {}", if reply["new"] == Value::Bool(false) { "brought forward" } else { "opened" }, reply["desk"].as_str().unwrap_or("the Desk"), tilde(reply["project"].as_str().unwrap_or("?"))),
        Some("shelve") => format!(
            "moved {} to the Shelf as {}, {}: `kanna task pick {}` brings it back",
            reply["pane"].as_str().unwrap_or("?"),
            reply["task"]["id"].as_str().unwrap_or("?"),
            if reply["running"] == Value::Bool(false) { "stopped" } else { "still running" },
            reply["task"]["id"].as_str().unwrap_or("?")
        ),
        Some("send") => {
            let to = reply["to"]["id"].as_str().or(reply["to"].as_str()).unwrap_or("?");
            let warning = reply["warning"].as_str().map_or(String::new(), |why| format!("\nwarning: it waits: {why}"));
            let said: String = match (to, reply["state"].as_str().unwrap_or("")) {
                ("planner", "no-planner") => "queued for this repo's planner: none is open, so it gets it when one is".into(),
                ("planner", _) => "queued for this repo's planner: it gets it at its prompt".into(),
                (to, "working") => format!("queued for {to}: it's working, so it gets it when its turn is over"),
                (to, "idle") => format!("queued for {to}: it's at its prompt, so it gets it in a few seconds"),
                (to, "needs-you") => format!("queued for {to}: it's asking you something, so it gets it once that's answered and its turn is over"),
                (to, "stopped") => format!("queued for {to}: it's stopped; on the Shelf it runs for it, on the Desk it gets it when it's started"),
                (to, _) => format!("queued for {to}: it gets it at its prompt"),
            };
            said + &warning
        }
        Some("messages") => {
            let list = reply["messages"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                return "no messages".into();
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            let who = |v: &Value| v["id"].as_str().or(v.as_str()).unwrap_or("?").to_string();
            let rows: Vec<[String; 5]> = list
                .iter()
                .map(|m| {
                    let text: String = m["text"].as_str().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
                    let text = if text.chars().count() > 60 { format!("{}…", text.chars().take(59).collect::<String>()) } else { text };
                    let state = match (m["delivered"].is_null(), m["warning"].is_string()) {
                        (false, _) => "delivered",
                        (true, true) => "held",
                        (true, false) => "waiting",
                    };
                    [ago(now - m["at"].as_f64().unwrap_or(now)), who(&m["from"]), who(&m["to"]), state.into(), text]
                })
                .collect();
            let mut held: Vec<String> = list.iter().filter_map(|m| Some(format!("held for {}: {}", who(&m["to"]), m["warning"].as_str()?))).collect();
            held.dedup();
            let head = ["SENT", "FROM", "TO", "STATE", "TEXT"];
            let width = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0).max(head[i].len());
            let line = |r: [&str; 5]| format!("{:w0$}  {:w1$}  {:w2$}  {:w3$}  {}", r[0], r[1], r[2], r[3], r[4], w0 = width(0), w1 = width(1), w2 = width(2), w3 = width(3)).trim_end().to_string();
            std::iter::once(line(head)).chain(rows.iter().map(|r| line([&r[0], &r[1], &r[2], &r[3], &r[4]]))).chain(held).collect::<Vec<_>>().join("\n")
        }
        Some("name") => format!("named {} \"{}\"", request["task"].as_str().or(request["pane"].as_str()).unwrap_or("?"), request["name"].as_str().unwrap_or("")),
        Some("done") => format!("{} is done: closed {}{}", request["task"].as_str().unwrap_or("?"), join(&reply["closed"]), freed_lines(reply)),
        _ => String::new(),
    }
}

pub fn listed(list: &Value) -> bool {
    list.as_array().is_some_and(|l| !l.is_empty())
}

fn names(list: &Value) -> String {
    list.as_array().into_iter().flatten().filter_map(|w| w["name"].as_str().map(|n| format!("\"{n}\"")).or_else(|| w["id"].as_str().map(String::from))).collect::<Vec<_>>().join(", ")
}

fn ids(list: &Value) -> String {
    list.as_array().map_or(vec![], |l| l.iter().filter_map(|w| w["id"].as_str()).collect::<Vec<_>>()).join(", ")
}

pub fn freed(listed: &Value, done: &str) -> Vec<Value> {
    let tasks = listed["shelf"].as_array().cloned().unwrap_or_default();
    tasks
        .into_iter()
        .filter(|t| matches!(t["waits"].as_array().map(Vec::as_slice), Some([only]) if only["id"] == done || only["pane"] == done))
        .map(|t| serde_json::json!({ "id": t["id"], "name": t["name"] }))
        .collect()
}

fn freed_lines(reply: &Value) -> String {
    let freed = reply["freed"].as_array().cloned().unwrap_or_default();
    freed.iter().map(|t| format!("\n{} \"{}\" can start now: nothing it waits for is left", t["id"].as_str().unwrap_or("?"), t["name"].as_str().unwrap_or(""))).collect()
}

pub fn place(reply: &Value) -> &'static str {
    if reply["place"] == "shelf" { " on the Shelf" } else { "" }
}

pub fn name_out(reply: &mut Value, out: &Value) {
    let Some(tasks) = reply["tasks"].as_array_mut() else { return };
    for task in tasks {
        if let Some(known) = out.as_array().into_iter().flatten().find(|o| o["pane"] == task["id"]) {
            task["task"] = known["id"].clone();
            for key in ["repo", "merged", "replied", "ready", "stale", "bases", "branch"] {
                if let Some(value) = known.get(key) {
                    task[key] = value.clone();
                }
            }
        }
    }
}

pub const RECENT_DONE: usize = 5;

pub fn keep_recent_done(reply: &mut Value) {
    let Some(done) = reply.get_mut("done").and_then(Value::as_array_mut) else { return };
    let more = done.len().saturating_sub(RECENT_DONE);
    done.truncate(RECENT_DONE);
    if more > 0 {
        reply["moreDone"] = more.into();
    }
}

pub fn keep_named(reply: &mut Value) {
    if let Some(tasks) = reply["tasks"].as_array_mut() {
        tasks.retain(|task| task["task"].is_string());
    }
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

/// Whether a query word picks out a task: a start of its task ID (`task-1dfb`, or 4 hex digits or more without `task-`), its pane ID exactly, or part of its name, first message or branch, ignoring case.
pub fn task_matches(task: &Value, word: &str) -> bool {
    let word = word.to_lowercase();
    let start = id_start(&word);
    let id = [&task["task"], &task["id"]].iter().filter_map(|v| v.as_str()).any(|id| start.as_ref().is_some_and(|start| id.to_lowercase().starts_with(start.as_str())));
    let pane = [&task["id"], &task["held"]].iter().filter_map(|v| v.as_str()).any(|pane| pane.to_lowercase() == word);
    id || pane || [&task["name"], &task["message"], &task["branch"]].iter().filter_map(|v| v.as_str()).any(|text| text.to_lowercase().contains(&word))
}

fn id_start(word: &str) -> Option<String> {
    let (hex, least) = match word.strip_prefix("task-") {
        Some(hex) => (hex, 1),
        None => (word, 4),
    };
    (hex.len() >= least && hex.len() <= 8 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("task-{hex}"))
}

pub fn keep_matching(reply: &mut Value, query: &str) {
    let words: Vec<&str> = query.split_whitespace().collect();
    for list in ["tasks", "shelf", "elsewhere", "done"] {
        if let Some(tasks) = reply.get_mut(list).and_then(Value::as_array_mut) {
            tasks.retain(|task| words.iter().all(|word| task_matches(task, word)));
        }
    }
}

/// The task ID a query is, when it's only that and nothing listed matches it: a task that's done, or never was.
pub fn lone_task(reply: &Value, query: &str) -> Option<String> {
    let none = ["tasks", "shelf", "done"].iter().all(|list| reply[list].as_array().is_none_or(Vec::is_empty));
    let id = query.trim().to_lowercase();
    id_start(&id).filter(|_| none && !id.contains(char::is_whitespace))
}

fn done_line(task: &Value) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let when = task["doneAt"].as_f64().map_or(String::new(), |at| format!(" ({})", ago(now - at)));
    let name = task["name"].as_str().map_or(String::new(), |n| format!(" \"{n}\""));
    let branch = task["branch"].as_str().map_or(String::new(), |b| format!("; its branch {b} is still there"));
    format!("{}{name} is done{when}{branch}", task["short"].as_str().or(task["id"].as_str()).unwrap_or("?"))
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

#[cfg(test)]
mod blocked_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_stacked_task_lists_as_ready_or_stale_and_says_what_it_merges_after_or_is_behind() {
        let mark = json!({ "head": "0123456789abcdef", "branch": "screenshot", "at": 1 });
        let shelf = json!([
            { "id": "t1", "name": "A", "repo": "/r", "running": false, "waits": [], "ready": mark, "stale": [], "bases": [] },
            { "id": "t2", "name": "B", "repo": "/r", "running": false, "waits": [], "ready": null, "stale": [{ "id": "t1", "name": "A" }], "bases": [{ "id": "t1", "name": "A" }] },
            { "id": "t3", "name": "C", "repo": "/r", "running": false, "waits": [], "ready": mark, "stale": [], "bases": [{ "id": "t2", "name": "B" }] },
            { "id": "t4", "name": "D", "repo": "/r", "running": false, "waits": [], "conflict": "merging t1's branch a conflicts" }
        ]);
        let text = task(&json!({ "action": "list", "repo": "/r" }), &json!({ "tasks": [], "shelf": shelf }));
        assert_eq!(text, "Desk\n(empty)\n\nShelf\nt1  A  ready\nt2  B  stale    behind \"A\"\nt3  C  ready    merges after \"B\"\nt4  D  blocked  its blockers' branches conflict");
        let ready = task(&json!({ "action": "ready" }), &json!({ "task": { "id": "t1", "ready": mark }, "freed": [{ "id": "t2", "name": "B" }] }));
        assert_eq!(ready, "t1 is ready at 012345678 (screenshot)\nt2 \"B\" can start now: nothing it waits for is left");
        assert_eq!(task(&json!({ "action": "unready" }), &json!({ "task": { "id": "t1" } })), "t1 isn't ready");
        let mut desk = json!({ "tasks": [{ "id": "p1" }] });
        name_out(&mut desk, &json!([{ "id": "t3", "pane": "p1", "ready": mark, "stale": [], "bases": [{ "id": "t2", "name": "B" }] }]));
        assert_eq!(desk["tasks"][0]["bases"][0]["name"], "B", "a Desk task's row gets its stack too");
    }

    #[test]
    fn a_blocked_task_lists_as_blocked_with_what_it_waits_for_and_done_names_what_it_frees() {
        let listed = json!({ "shelf": [
            { "id": "t13", "name": "Diff viewer", "repo": "/r", "running": false, "waits": [{ "id": "t12", "pane": "p40" }] },
            { "id": "t14", "name": "Release notes", "repo": "/r", "running": false, "waits": [{ "id": "t8" }, { "id": "t12", "pane": "p40" }] },
            { "id": "t15", "name": "Free", "repo": "/r", "running": false, "waits": [] }
        ] });
        let text = task(&json!({ "action": "list", "repo": "/r" }), &json!({ "tasks": [], "shelf": listed["shelf"] }));
        assert!(text.contains("t13  Diff viewer    blocked  waits for t12\n"), "{text}");
        assert!(text.contains("blocked  waits for t8, t12\n"), "{text}");
        assert!(text.ends_with("t15  Free           stopped"), "{text}");
        assert_eq!(freed(&listed, "p40"), vec![json!({ "id": "t13", "name": "Diff viewer" })], "by its pane, and only where it was the last blocker");
        assert_eq!(freed(&listed, "t12").len(), 1);
        let done = task(&json!({ "action": "done", "task": "p40" }), &json!({ "closed": ["p40"], "freed": freed(&listed, "p40") }));
        assert_eq!(done, "p40 is done: closed p40\nt13 \"Diff viewer\" can start now: nothing it waits for is left");
        assert_eq!(task(&json!({ "action": "edit" }), &json!({ "task": { "id": "t14", "waits": [] } })), "t14 waits for nothing: it can start");
    }
}

#[cfg(test)]
mod list_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_desk_comes_first_then_the_shelf_saying_only_what_matters() {
        let reply = json!({
            "agent": "claude",
            "tasks": [
                { "id": "p1", "task": "t1", "name": "Fix login", "agent": "/usr/bin/claude --yolo", "running": true, "status": "idle", "merged": true, "repo": "/r" },
                { "id": "p2", "task": "t2", "name": "Diff viewer", "agent": "claude", "running": true, "status": "working", "merged": true, "repo": "/r" }
            ],
            "shelf": [
                { "id": "t3", "name": "Release notes", "repo": "/r", "agent": "claude", "running": false, "replied": true, "madeAt": 1, "waits": [] },
                { "id": "t4", "name": "Port", "repo": "/other", "agent": "codex", "running": false, "waits": [] },
                { "id": "t5", "name": "Later", "repo": "/r", "agent": "claude", "running": false, "waits": [{ "id": "t2", "name": "Diff viewer" }, { "id": "t9" }] }
            ]
        });
        let text = task(&json!({ "action": "list", "repo": "/r" }), &reply);
        assert_eq!(
            text,
            "Desk\nt1  Fix login      merged\nt2  Diff viewer    working\n\nShelf\nt3  Release notes  replied\nt4  Port           stopped  codex  /other\nt5  Later          blocked  waits for \"Diff viewer\", t9"
        );
        let empty = task(&json!({ "action": "list" }), &json!({ "tasks": [], "shelf": [reply["shelf"][0].clone()] }));
        assert!(empty.starts_with("Desk\n(empty)\n\nShelf\nt3"), "{empty}");
        let found = task(&json!({ "action": "list", "query": "release" }), &json!({ "tasks": [], "shelf": [reply["shelf"][0].clone()] }));
        assert!(found.starts_with("Shelf\n"), "{found}");
    }

    #[test]
    fn a_desk_task_is_named_with_its_repo_and_state_and_other_repos_go() {
        let mut reply = json!({ "tasks": [{ "id": "p1" }, { "id": "p9" }] });
        name_out(&mut reply, &json!([{ "id": "t1", "pane": "p1", "repo": "/r", "merged": true, "replied": false }]));
        keep_named(&mut reply);
        assert_eq!(reply["tasks"], json!([{ "id": "p1", "task": "t1", "repo": "/r", "merged": true, "replied": false }]));
    }
}

fn shown_path(path: &str) -> String {
    let here = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    match path.strip_prefix(&format!("{}/", here.trim_end_matches('/'))) {
        Some(inside) if !here.is_empty() => inside.to_string(),
        _ => path.to_string(),
    }
}

fn stop_lines(first: Option<u64>, last: Option<u64>) -> String {
    match (first, last) {
        (Some(first), Some(last)) if last > first => format!("{first}–{last}"),
        (Some(first), _) => first.to_string(),
        _ => "?".into(),
    }
}

pub fn toured(args: &Value, reply: &Value) -> String {
    if reply["done"] == true {
        return format!("tour done · {} stops", reply["of"].as_u64().unwrap_or(0));
    }
    match args["action"].as_str().unwrap_or("show") {
        "end" => (if reply["ended"] == true { "tour ended" } else { "no tour" }).to_string(),
        "show" => {
            let stops = reply["stops"].as_array().cloned().unwrap_or_default();
            if stops.is_empty() {
                return "no tour".into();
            }
            let at = reply["at"].as_u64().unwrap_or(1) as usize;
            let place = |stop: &Value| format!("{}:{}", shown_path(stop["path"].as_str().unwrap_or("?")), stop_lines(stop["first"].as_u64(), stop["last"].as_u64()));
            let mut out = format!("at {at} of {} · {}", stops.len(), place(&stops[at - 1]));
            for (i, stop) in stops.iter().enumerate() {
                let marker = if i + 1 == at { "▸" } else { " " };
                let text: String = stop["text"].as_str().unwrap_or("").chars().take(60).collect();
                out += &format!("\n{marker} {:>2}  {}  {text}", i + 1, place(stop));
            }
            out
        }
        action => {
            let (at, of) = (reply["tour"]["at"].as_u64().unwrap_or(0), reply["tour"]["of"].as_u64().unwrap_or(0));
            if reply.get("pane").is_none() {
                return format!("added {} · at {at} of {of}", reply["added"].as_u64().unwrap_or(0));
            }
            let opened = if reply["opened"] == true { "opened " } else { "" };
            let place = format!("{}:{}", shown_path(reply["path"].as_str().unwrap_or("?")).rsplit('/').next().unwrap_or("?"), stop_lines(reply["first"].as_u64(), reply["last"].as_u64()));
            let thread = reply["thread"].as_str().map(|t| format!(" · {t}")).unwrap_or_default();
            let step = format!("{at} of {of} · {opened}{} · {place}{thread}", label(&reply["pane"]));
            match action {
                "new" => format!("tour of {of} · {step}"),
                "add" => format!("added {} · {step}", args["stops"].as_array().map(Vec::len).unwrap_or(0)),
                _ => step,
            }
        }
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

fn others(addrs: &Value) -> String {
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
