use serde_json::{json, Value};

pub fn join(list: &Value) -> String {
    list.as_array().map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

fn label(pane: &Value) -> String {
    let id = pane["id"].as_str().unwrap_or("?");
    match pane["name"].as_str() {
        Some(name) => format!("{id} ({name})"),
        None => id.to_string(),
    }
}

fn state(pane: &Value) -> String {
    let state = pane["state"].as_str().unwrap_or("?");
    match (pane["exitCode"].as_i64(), pane["error"].as_str()) {
        (Some(code), _) => format!("{state} ({code})"),
        (_, Some(error)) => format!("{state}: {error}"),
        _ => state.to_string(),
    }
}

fn whose(reply: &Value) -> Option<(String, Vec<String>)> {
    let workspace = reply.get("workspace").filter(|w| w.is_object())?;
    let id = workspace["id"].as_str().unwrap_or("?");
    let place = &workspace["where"];
    let mut tags: Vec<String> = workspace["task"].as_str().map(String::from).into_iter().collect();
    match place.as_str() {
        Some("backlog") => tags.push("in the backlog".into()),
        Some(_) => tags.push("on the Shelf".into()),
        None => {}
    }
    let name = match (reply["window"].as_str(), place["desk"].as_str()) {
        (Some(window), Some(desk)) if window == desk => format!("{id} on {window}"),
        (Some(window), _) => format!("{id}'s {window}"),
        (None, _) => id.to_string(),
    };
    Some((name, tags))
}

fn tagged(name: &str, tags: &[String]) -> String {
    if tags.is_empty() { name.to_string() } else { format!("{name} ({})", tags.join(", ")) }
}

fn hidden(reply: &Value) -> bool {
    reply["workspace"]["where"].is_string()
}

fn whose_text(reply: &Value) -> Option<String> {
    whose(reply).map(|(name, tags)| tagged(&name, &tags))
}

pub fn opened(request: &Value, reply: &Value) -> String {
    if reply["window"].is_null() && reply.get("window").is_some() && whose(reply).is_none() {
        return format!("opened {}, unplaced until Kanna.app opens: {}", label(&reply["pane"]), state(&reply["pane"]));
    }
    let place = whose_text(reply).unwrap_or_else(|| format!("window {}", reply["window"].as_str().unwrap_or("?")));
    if let Some(edge) = reply["dock"].as_str() {
        let frozen = if request["frozen"] == true { ", frozen," } else { "" };
        return format!("opened {} docked {edge}{frozen} in {place}: {}", label(&reply["pane"]), state(&reply["pane"]));
    }
    format!("opened {} in {place}, tab {}: {}", label(&reply["pane"]), reply["tab"].as_str().unwrap_or("?"), state(&reply["pane"]))
}

pub fn commented(args: &Value, reply: &Value) -> String {
    let file = args["path"].as_str().and_then(|p| p.rsplit('/').next()).unwrap_or("?");
    let lines = match (reply["first"].as_u64(), reply["last"].as_u64()) {
        (Some(first), Some(last)) if last > first => format!("{first}–{last}"),
        (Some(first), _) => first.to_string(),
        _ => "?".into(),
    };
    let opened = if reply["opened"] == true { "opened " } else { "" };
    let place = whose_text(reply).map(|w| format!(" in {w}")).unwrap_or_default();
    format!("{opened}{}{place} · {file}:{lines}", label(&reply["pane"]))
}

pub fn marked(args: &Value, reply: &Value) -> String {
    if args.get("type").is_some() {
        return opened(args, reply);
    }
    let pane = &reply["pane"];
    let to = pane["agent"].as_str().map(|agent| format!(", sent to {agent}")).unwrap_or_default();
    match pane["markup"] == true {
        true => format!("comments on in {}{to}", label(pane)),
        false => format!("comments off in {}", label(pane)),
    }
}

pub fn moved(reply: &Value) -> String {
    let dock = &reply["dock"];
    if dock.is_object() {
        return format!(
            "{} is docked {} in {} (width {})",
            dock["pane"].as_str().unwrap_or("?"),
            dock["edge"].as_str().unwrap_or("?"),
            reply["window"].as_str().unwrap_or("?"),
            dock["width"].as_str().unwrap_or("?")
        );
    }
    layout(reply)
}

pub fn closed(args: &Value, reply: &Value) -> String {
    let panes = join(&reply["closed"]);
    match (args["window"].as_str(), args["tab"].as_str()) {
        (Some(window), _) => format!("closed window {window} (panes {panes})"),
        (_, Some(tab)) => format!("closed tab {tab} (panes {panes})"),
        _ => format!("closed {panes}"),
    }
}

pub fn focused(args: &Value, reply: &Value) -> String {
    let window = reply["window"].as_str().unwrap_or("?");
    let place = match whose(reply) {
        Some((name, tags)) if hidden(reply) => format!(" in {}", tagged(&name, &tags)),
        Some((name, mut tags)) => {
            tags.push("key window".into());
            format!(" in {}", tagged(&name, &tags))
        }
        None => format!(" (key window {window})"),
    };
    match (args["pane"].as_str(), args["tab"].as_str()) {
        (Some(pane), _) => format!("focused {pane}{place}"),
        (_, Some(tab)) => format!("switched to {tab}{place}"),
        _ => format!("key window is now {window}"),
    }
}

pub fn zoomed(args: &Value, reply: &Value) -> String {
    let pane = args["pane"].as_str().unwrap_or("pane");
    let zoomed = reply["zoomed"] == serde_json::json!(true);
    match (whose(reply), zoomed) {
        (Some((name, tags)), true) if hidden(reply) => format!("{pane} is zoomed in {name} ({}; run again to unzoom)", tags.join(", ")),
        (Some((name, tags)), true) => format!("{pane} is zoomed in {} (run again to unzoom)", tagged(&name, &tags)),
        (Some((name, tags)), false) => format!("{pane} is no longer zoomed in {}", tagged(&name, &tags)),
        (None, true) => format!("{pane} is zoomed (run again to unzoom)"),
        (None, false) => format!("{pane} is no longer zoomed"),
    }
}

pub fn window(args: &Value, reply: &Value) -> String {
    let window = reply["window"].as_str().or(args["window"].as_str()).unwrap_or("?");
    let named = |ws: &Value| tagged(ws["id"].as_str().unwrap_or("?"), &ws["task"].as_str().map(String::from).into_iter().collect::<Vec<_>>());
    match (args["action"].as_str(), whose(reply)) {
        (Some("assign"), Some(_)) => {
            let ws = &reply["workspace"];
            let place = match ws["where"].as_str() {
                Some("backlog") => "in the backlog: hidden until it's pulled".to_string(),
                Some(_) => "on the Shelf: hidden until it's pulled".to_string(),
                None => format!("on {}", ws["where"]["desk"].as_str().unwrap_or("?")),
            };
            let tags: Vec<String> = ws["task"].as_str().map(String::from).into_iter().chain([place]).collect();
            format!("assigned {window} to {}", tagged(ws["id"].as_str().unwrap_or("?"), &tags))
        }
        _ => match reply["was"]["id"].as_str() {
            Some(_) if reply["was"]["hidden"] == true => format!("{window} is free (out of {}, and open again)", named(&reply["was"])),
            Some(_) => format!("{window} is free (out of {})", named(&reply["was"])),
            None => format!("{window} is free"),
        },
    }
}

pub fn renamed(_: &Value, reply: &Value) -> String {
    format!("{} is now \"{}\"", reply["tab"].as_str().unwrap_or("?"), reply["title"].as_str().unwrap_or(""))
}

pub fn pane_renamed(_: &Value, reply: &Value) -> String {
    let pane = &reply["pane"];
    let id = pane["id"].as_str().unwrap_or("?");
    match pane["name"].as_str() {
        Some(name) => format!("{id} is now \"{name}\""),
        None => format!("{id} has no name"),
    }
}

pub fn task(request: &Value, reply: &Value) -> String {
    let id = reply["task"].as_str().unwrap_or("?");
    match request["action"].as_str() {
        Some("list") => {
            let tasks = reply["tasks"].as_array().cloned().unwrap_or_default();
            let waiting = reply["backlog"].as_array().cloned().unwrap_or_default();
            if let (true, Some(query)) = (tasks.is_empty() && waiting.is_empty(), request["query"].as_str()) {
                return format!("no task matches \"{query}\"");
            }
            if tasks.is_empty() && waiting.is_empty() {
                return "no tasks: add one with `kanna task new NAME \"FIRST MESSAGE\"`, or dock an agent with `kanna open --dock --cmd claude`".into();
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            let setup = |t: &Value| t["setup"].as_str().or_else(|| t["id"].as_str().and_then(|id| reply["setup"][id].as_str())).map(String::from);
            let state = |t: &Value| match (t["running"].as_bool(), t["status"].as_str()) {
                _ if setup(t).is_some() => setup(t).unwrap_or_default(),
                (Some(false), _) => "stopped".to_string(),
                (_, Some("needs-you")) => "needs you".to_string(),
                (_, Some(status)) if status != "exited" => status.to_string(),
                _ => "running".to_string(),
            };
            let rows: Vec<[String; 5]> = tasks
                .iter()
                .map(|t| {
                    let place = t["window"].as_str().map_or("Shelf".to_string(), |w| format!("Desk {w}"));
                    let parked = t["parkedAt"].as_f64().map_or(String::new(), |at| format!("parked {}", ago(now - at)));
                    [t["task"].as_str().or(t["id"].as_str()).unwrap_or("?").into(), t["name"].as_str().unwrap_or("").into(), place, state(t), parked]
                })
                .chain(waiting.iter().map(|t| {
                    let made = t["madeAt"].as_f64().map_or(String::new(), |at| format!("made {} · {}", ago(now - at), tilde(t["repo"].as_str().unwrap_or(""))));
                    let waits = ids(&t["waits"]);
                    let (state, made) = match (t["running"].as_bool(), waits.is_empty()) {
                        (Some(false), false) => ("blocked".to_string(), format!("{made} · waits for {waits}")),
                        _ => (state(t), made),
                    };
                    [t["id"].as_str().unwrap_or("?").into(), t["name"].as_str().unwrap_or("").into(), "Backlog".into(), state, made]
                }))
                .collect();
            let head = ["TASK", "NAME", "WHERE", "STATE", ""];
            let width = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0).max(head[i].len());
            let line = |r: [&str; 5]| format!("{:w0$}  {:w1$}  {:w2$}  {:w3$}  {}", r[0], r[1], r[2], r[3], r[4], w0 = width(0), w1 = width(1), w2 = width(2), w3 = width(3)).trim_end().to_string();
            std::iter::once(line(head)).chain(rows.iter().map(|r| line([&r[0], &r[1], &r[2], &r[3], &r[4]]))).collect::<Vec<_>>().join("\n")
        }
        Some("park") if reply["parked"] == Value::Bool(true) => format!("parked {id}; {} is an empty Desk", reply["window"].as_str().unwrap_or("?")),
        Some("park") => format!("{id} is already on the Shelf"),
        Some("pull") if reply["moved"] == Value::Bool(true) => format!("pulled {id} onto {}", reply["window"].as_str().unwrap_or("?")),
        Some("pull") => format!("{id} is on {}; brought it forward", reply["window"].as_str().unwrap_or("?")),
        Some("new") => format!(
            "added {} \"{}\" to the backlog of {}{}",
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
            "started {} in the backlog: {} in {}{}",
            reply["task"].as_str().unwrap_or("?"),
            reply["pane"].as_str().unwrap_or("?"),
            tilde(reply["folder"].as_str().unwrap_or("?")),
            reply["branch"].as_str().map_or(String::new(), |b| format!(" ({b})"))
        ),
        Some("setup") if reply["tab"].is_string() => format!("{id}'s setup runs again in {} (a Setup tab)", reply["pane"].as_str().unwrap_or("?")),
        Some("setup") => format!("{id}'s setup runs again in {}, then its agent", reply["pane"].as_str().unwrap_or("?")),
        Some("stop") => format!("stopped {}{}; `kanna task start {}` starts it again", reply["task"].as_str().unwrap_or("?"), place(reply), reply["task"].as_str().unwrap_or("?")),
        Some("shelve") => format!(
            "moved {} to the {}: {} in {}{}",
            reply["task"].as_str().unwrap_or("?"),
            reply["window"].as_str().map_or("Shelf".to_string(), |w| format!("Desk {w}")),
            reply["pane"].as_str().unwrap_or("?"),
            tilde(reply["folder"].as_str().unwrap_or("?")),
            reply["branch"].as_str().map_or(String::new(), |b| format!(" ({b})"))
        ),
        Some("done") if reply.get("closed").is_none() => format!("took {} off the backlog{}", reply["task"]["id"].as_str().unwrap_or("?"), freed_lines(reply)),
        Some("desk") => format!(
            "{} {} on {}",
            if reply["new"] == Value::Bool(false) { "brought forward" } else { "opened" },
            reply["window"].as_str().unwrap_or("?"),
            tilde(reply["project"].as_str().unwrap_or("?"))
        ),
        Some("defer") => format!(
            "moved {} to the backlog as {}, {}: shelve it to bring it back",
            reply["pane"].as_str().unwrap_or("?"),
            reply["task"]["id"].as_str().unwrap_or("?"),
            if reply["running"] == Value::Bool(false) { "stopped" } else { "still running" }
        ),
        Some("name") => format!("named {} \"{}\"", request["task"].as_str().or(request["pane"].as_str()).unwrap_or("?"), request["name"].as_str().unwrap_or("")),
        Some("done") => format!("{} is done: closed {}{}", request["task"].as_str().unwrap_or("?"), join(&reply["closed"]), freed_lines(reply)),
        _ => String::new(),
    }
}

fn ids(list: &Value) -> String {
    list.as_array().map_or(vec![], |l| l.iter().filter_map(|w| w["id"].as_str()).collect::<Vec<_>>()).join(", ")
}

pub fn freed(backlog: &Value, done: &str) -> Vec<Value> {
    let tasks = backlog["tasks"].as_array().cloned().unwrap_or_default();
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

fn place(reply: &Value) -> &'static str {
    if reply["place"] == "backlog" { " in the backlog" } else { "" }
}

pub fn name_out(reply: &mut Value, out: &Value) {
    let Some(tasks) = reply["tasks"].as_array_mut() else { return };
    for task in tasks {
        if let Some(known) = out.as_array().into_iter().flatten().find(|o| o["pane"] == task["id"]) {
            task["task"] = known["id"].clone();
        }
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

pub fn keep_matching(reply: &mut Value, query: &str) {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let matches = |task: &Value| {
        let text = [&task["name"], &task["message"]].iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n").to_lowercase();
        words.iter().all(|word| text.contains(word.as_str()))
    };
    for list in ["tasks", "backlog"] {
        if let Some(tasks) = reply[list].as_array_mut() {
            tasks.retain(matches);
        }
    }
}

fn tilde(path: &str) -> String {
    match std::env::var("HOME").ok().filter(|h| !h.is_empty()) {
        Some(home) if path == home => "~".into(),
        Some(home) if path.starts_with(&format!("{home}/")) => format!("~{}", &path[home.len()..]),
        _ => path.into(),
    }
}

fn ago(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor().max(0.0) as u64;
    match minutes {
        0 => "just now".into(),
        m if m < 60 => format!("{m}m ago"),
        m if m < 60 * 24 => format!("{}h ago", m / 60),
        m => format!("{}d ago", m / 60 / 24),
    }
}

pub fn tabs(_: &Value, reply: &Value) -> String {
    format!("{}: {}", whose_text(reply).unwrap_or_else(|| reply["window"].as_str().unwrap_or("?").to_string()), join(&reply["tabs"]))
}

pub fn capabilities(reply: &Value) -> String {
    let mut out = format!("pane types: {}\ncommands: {}", join(&reply["paneTypes"]), join(&reply["commands"]));
    if let Some(app) = reply["app"].as_bool() {
        out += if app {
            "\nKanna.app: attached"
        } else {
            "\nKanna.app: not attached (terminals work without it; the CLI starts it for commands that need it)"
        };
    }
    out
}

fn what(pane: &Value) -> Option<String> {
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

pub fn scoped(reply: &Value, pane: &str) -> Option<Value> {
    let empty = vec![];
    let all = |key: &str| reply[key].as_array().unwrap_or(&empty).clone();
    let (windows, hidden, workspaces, panes) = (all("windows"), all("hiddenWindows"), all("workspaces"), all("panes"));
    let row = panes.iter().find(|p| p["id"] == pane || p["name"] == pane)?;
    let id = row["id"].as_str().unwrap_or(pane).to_string();
    let shown = row["name"].as_str().unwrap_or(&id).to_string();
    let named = |v: &Value| v == shown.as_str() || v == id.as_str();
    let has = |list: &Value, p: &str| list.as_array().is_some_and(|l| l.iter().any(|x| x == p));
    let mut out = json!({ "pane": id, "at": null, "windows": [], "hiddenWindows": [], "workspaces": [], "panes": [row] });
    if let Some(ws) = workspaces.iter().find(|w| has(&w["panes"], &id)) {
        let ours = |w: &&Value| w["shows"] == ws["id"] || w["workspace"] == ws["id"];
        out["at"] = json!("workspace");
        out["workspaces"] = json!([ws]);
        out["windows"] = Value::Array(windows.iter().filter(ours).cloned().collect());
        out["hiddenWindows"] = Value::Array(hidden.iter().filter(ours).cloned().collect());
        out["panes"] = Value::Array(panes.iter().filter(|p| p["id"].as_str().is_some_and(|p| has(&ws["panes"], p))).cloned().collect());
        return Some(out);
    }
    let holds = |w: &Value| named(&w["lead"]["pane"]) || named(&w["dock"]["pane"]) || w["tabs"].as_array().is_some_and(|t| t.iter().any(|t| layout_panes(&t["layout"]).iter().any(|p| named(&json!(p)))));
    let found = windows.iter().find(|w| holds(w)).map(|w| (w, "windows")).or_else(|| hidden.iter().find(|w| holds(w)).map(|w| (w, "hiddenWindows")));
    if let Some((window, key)) = found {
        out["at"] = json!("window");
        out[key] = json!([window]);
        let shows: Vec<Value> = workspaces.iter().filter(|w| w["id"] == window["shows"]).cloned().collect();
        let mut inside: Vec<String> = window["tabs"].as_array().unwrap_or(&empty).iter().flat_map(|t| layout_panes(&t["layout"])).collect();
        inside.extend(window["lead"]["pane"].as_str().map(String::from));
        inside.extend(shows.iter().flat_map(|w| w["panes"].as_array().cloned().unwrap_or_default()).filter_map(|p| p.as_str().map(String::from)));
        out["workspaces"] = Value::Array(shows);
        out["panes"] = Value::Array(panes.iter().filter(|p| inside.iter().any(|i| p["id"] == i.as_str() || p["name"] == i.as_str())).cloned().collect());
    }
    Some(out)
}

fn layout_panes(node: &Value) -> Vec<String> {
    match node["pane"].as_str() {
        Some(pane) => vec![pane.to_string()],
        None => node["children"].as_array().map_or(vec![], |kids| kids.iter().flat_map(layout_panes).collect()),
    }
}

pub fn workspace(request: &Value, reply: &Value) -> String {
    match request["action"].as_str() {
        Some("list") if request["all"] == true => everything(reply),
        Some("list") => own(reply),
        _ => String::new(),
    }
}

fn own(reply: &Value) -> String {
    let empty = vec![];
    let pane = reply["panes"].as_array().and_then(|p| p.iter().find(|p| p["id"] == reply["pane"])).map_or_else(|| reply["pane"].as_str().unwrap_or("?").to_string(), label);
    let mut out = Vec::new();
    match reply["at"].as_str() {
        Some("workspace") => {
            for ws in reply["workspaces"].as_array().unwrap_or(&empty) {
                out.extend(workspace_block(ws, reply));
            }
        }
        Some("window") => {
            for window in ["windows", "hiddenWindows"].iter().flat_map(|k| reply[*k].as_array().cloned().unwrap_or_default()) {
                out.push(window_line(&window, reply));
                out.extend(focus_line(&window, "  "));
                out.extend(tab_lines(&window["tabs"], "  "));
            }
            for ws in reply["workspaces"].as_array().unwrap_or(&empty) {
                out.push(String::new());
                out.extend(workspace_block(ws, reply));
            }
        }
        _ => out.push(format!("{pane} isn't in a window or a workspace")),
    }
    out.join("\n")
}

fn everything(reply: &Value) -> String {
    let empty = vec![];
    let windows = reply["windows"].as_array().unwrap_or(&empty);
    let panes = reply["panes"].as_array().unwrap_or(&empty);
    if windows.is_empty() && panes.is_empty() {
        return "no windows (open one with `kanna open`)".into();
    }
    let mut sections: Vec<String> = Vec::new();
    if windows.is_empty() {
        sections.push("no windows: Kanna.app isn't open (these terminals keep running)".into());
    } else {
        sections.push(std::iter::once("Windows".to_string()).chain(windows.iter().map(|w| window_line(w, reply))).collect::<Vec<_>>().join("\n"));
    }
    let mut workspaces: Vec<&Value> = reply["workspaces"].as_array().unwrap_or(&empty).iter().collect();
    workspaces.sort_by_key(|w| match w["where"].as_str() {
        None => 0,
        Some("shelf") => 1,
        Some(_) => 2,
    });
    if !workspaces.is_empty() {
        let blocks: Vec<String> = workspaces.iter().map(|ws| workspace_block(ws, reply).join("\n")).collect();
        sections.push(format!("Workspaces\n{}", blocks.join("\n")));
    }
    let free: Vec<String> = windows
        .iter()
        .filter(|w| w["shows"].is_null() && w["workspace"].is_null() && w["tabs"].as_array().is_some_and(|t| !t.is_empty()))
        .map(|w| std::iter::once(w["id"].as_str().unwrap_or("?").to_string()).chain(focus_line(w, "  ")).chain(tab_lines(&w["tabs"], "  ")).collect::<Vec<_>>().join("\n"))
        .collect();
    if !free.is_empty() {
        sections.push(format!("Free windows\n{}", free.join("\n")));
    }
    let mut placed: Vec<String> = workspaces.iter().flat_map(|w| w["panes"].as_array().cloned().unwrap_or_default()).filter_map(|p| p.as_str().map(String::from)).collect();
    for window in windows.iter().chain(reply["hiddenWindows"].as_array().unwrap_or(&empty)) {
        placed.extend(window["tabs"].as_array().unwrap_or(&empty).iter().flat_map(|t| layout_panes(&t["layout"])));
        placed.extend(window["lead"]["pane"].as_str().map(String::from));
    }
    let unplaced: Vec<String> = panes.iter().filter(|p| !placed.iter().any(|q| p["id"] == q.as_str() || p["name"] == q.as_str())).map(label).collect();
    if !unplaced.is_empty() {
        sections.push(format!("Unplaced panes\n{}", unplaced.join("\n")));
    }
    sections.join("\n\n")
}

fn named_pane(reply: &Value, shown: &str) -> String {
    reply["panes"].as_array().and_then(|p| p.iter().find(|p| p["id"] == shown || p["name"] == shown)).map_or_else(|| shown.to_string(), label)
}

fn workspace_name(ws: &Value) -> String {
    let id = ws["id"].as_str().unwrap_or("?");
    match (ws["task"].as_str(), ws["name"].as_str()) {
        (Some(task), Some(name)) => format!("{id} ({task}, {name})"),
        (Some(task), None) => format!("{id} ({task})"),
        _ => id.to_string(),
    }
}

fn window_line(window: &Value, reply: &Value) -> String {
    let empty = vec![];
    let workspaces = reply["workspaces"].as_array().unwrap_or(&empty);
    let ws = |id: &Value| workspaces.iter().find(|w| w["id"] == *id).map_or_else(|| id.as_str().unwrap_or("?").to_string(), workspace_name);
    let mut line = format!("{}{}", if window["key"] == true { "*" } else { " " }, window["id"].as_str().unwrap_or("?"));
    let lead = window["lead"]["pane"].as_str().map(|p| format!("planning terminal {}", named_pane(reply, p)));
    let what = match (window["shows"].is_string(), window["workspace"].is_string(), window["desk"] == true || lead.is_some()) {
        (true, _, _) => Some(format!("Desk, showing {}", ws(&window["shows"]))),
        (false, true, _) => Some(format!("{}'s window", ws(&window["workspace"]))),
        (false, false, true) => Some("Desk".to_string()),
        _ => Some("free".to_string()),
    };
    let parts: Vec<String> = what.into_iter().chain(lead).collect();
    line += &format!("  {}", parts.join(", "));
    line
}

fn focus_line(holder: &Value, indent: &str) -> Option<String> {
    let focused = holder["focused"].as_str()?;
    let zoomed = if holder["zoomed"].as_str() == Some(focused) { ", zoomed" } else { "" };
    Some(format!("{indent}focus: {focused}{zoomed}"))
}

fn tab_lines(tabs: &Value, indent: &str) -> Vec<String> {
    tabs.as_array().map_or(vec![], |tabs| {
        tabs.iter()
            .map(|tab| format!("{indent}{}{} \"{}\"  {}", if tab["active"] == true { "*" } else { " " }, tab["id"].as_str().unwrap_or("?"), tab["title"].as_str().unwrap_or(""), expression(&tab["layout"], true)))
            .collect()
    })
}

fn workspace_block(ws: &Value, reply: &Value) -> Vec<String> {
    let empty = vec![];
    let place = match (ws["where"].as_str(), ws["where"]["desk"].as_str()) {
        (Some("backlog"), _) => "in the backlog".to_string(),
        (Some(_), _) => "on the Shelf".to_string(),
        (None, Some(desk)) => format!("on Desk {desk}"),
        (None, None) => "on a Desk".to_string(),
    };
    let mut out = vec![format!("{}, {place}", workspace_name(ws))];
    let dock = &ws["dock"];
    if let Some(pane) = dock["id"].as_str().or(dock["pane"].as_str()) {
        out.push(format!(
            "  dock: {}  {} {}{}",
            named_pane(reply, pane),
            dock["edge"].as_str().unwrap_or("?"),
            dock["width"].as_str().unwrap_or("?"),
            if dock["frozen"] == true { "  frozen" } else { "" }
        ));
    }
    out.extend(focus_line(ws, "  "));
    out.extend(tab_lines(&ws["tabs"], "  "));
    for own in ws["windows"].as_array().unwrap_or(&empty).iter().filter_map(Value::as_str) {
        let find = |key: &str| reply[key].as_array().and_then(|l| l.iter().find(|w| w["id"] == own)).cloned();
        let (window, hidden) = match (find("windows"), find("hiddenWindows")) {
            (Some(w), _) => (Some(w), false),
            (None, w) => (w, true),
        };
        out.push(format!("  window {own}{}", if hidden { ", hidden with it" } else { "" }));
        if let Some(window) = window {
            out.extend(focus_line(&window, "    "));
            out.extend(tab_lines(&window["tabs"], "    "));
        }
    }
    out
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

pub fn quiz(request: &Value, reply: &Value) -> String {
    match request["action"].as_str() {
        Some("open") => format!("{} {} in {}", if reply["opened"] == true { "opened" } else { "brought forward" }, reply["pane"].as_str().unwrap_or("?"), reply["window"].as_str().unwrap_or("?")),
        Some("start") => reply["round"].as_str().unwrap_or("?").to_string(),
        Some("answer") => format!("{} in {}", reply["answer"].as_str().unwrap_or("?"), reply["round"].as_str().unwrap_or("?")),
        _ => {
            let rounds = reply["rounds"].as_array().cloned().unwrap_or_default();
            if rounds.is_empty() {
                return if reply["misses"] == true { "no misses to bring back".into() } else { "no quiz rounds in this repo yet".into() };
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            let mut out = vec![];
            for round in &rounds {
                let head: String = round["head"].as_str().unwrap_or("?").chars().take(7).collect();
                out.push(format!("{}  {head}  {}", round["round"].as_str().unwrap_or("?"), round["at"].as_f64().map_or(String::new(), |at| ago(now - at))));
                for answer in round["answers"].as_array().into_iter().flatten() {
                    let mark = if answer["right"] == true { "✓" } else { "✗" };
                    let line = answer["doc"]["line"].as_u64().map_or(String::new(), |l| format!(":{l}"));
                    let again = answer["again"].as_str().map_or(String::new(), |a| format!("  (again: {a})"));
                    let question = answer["doc"]["question"].as_str().map_or(String::new(), |q| format!("  {q}"));
                    out.push(format!("  {:<4} {mark} {:<12} {}{line}{question}{again}", answer["answer"].as_str().unwrap_or("?"), answer["kind"].as_str().unwrap_or("?"), answer["file"].as_str().unwrap_or("?")));
                }
            }
            out.join("\n")
        }
    }
}

pub fn status(args: &Value, reply: &Value) -> String {
    if args["action"] == "update" {
        let mut out = vec![match reply["site"].as_str() {
            Some(site) => format!("checked {site}"),
            None => "no rules site is set".to_string(),
        }];
        let fetched = join(&reply["fetched"]);
        out.push(if fetched.is_empty() { "nothing new".into() } else { format!("fetched: {fetched}") });
        for problem in reply["problems"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            out.push(format!("problem: {problem}"));
        }
        for rules in reply["rules"].as_array().into_iter().flatten() {
            out.push(format!("{:<12} {:<14} {}", rules["agent"].as_str().unwrap_or("?"), rules["version"].as_str().unwrap_or("?"), rules["source"].as_str().unwrap_or("?")));
        }
        return out.join("\n");
    }
    let mut out = vec![];
    let who = reply["pane"].as_str().map(|p| format!("{p}: ")).unwrap_or_default();
    match reply["agent"].as_str() {
        None => return format!("{who}not an agent Kanna knows (its foreground program has no rules){}", reply["status"].as_str().map(|s| format!("; status {s}")).unwrap_or_default()),
        Some(agent) => {
            let version = reply["agentVersion"].as_str().map(|v| format!(" {v}")).unwrap_or_else(|| " (version unknown)".into());
            let exe = reply["executable"].as_str().map(|e| format!(" ({e})")).unwrap_or_default();
            out.push(format!("{who}{agent}{version}{exe}"));
        }
    }
    let judged = reply["judged"].as_str().unwrap_or("?");
    if reply["source"].as_str() == Some("osc7501") {
        out.push(format!("status: {}", reply["status"].as_str().or(reply["reported"]["status"].as_str()).unwrap_or("?")));
        out.push("source: OSC 7501, the agent's own report".into());
        out.push(format!("record: {}", program_record(&reply["reported"])));
        for child in reply["records"].as_array().into_iter().flatten().filter(|r| r["id"].as_str().is_some_and(|id| !id.is_empty())) {
            out.push(format!("record {:?}: {}", child["id"].as_str().unwrap_or_default(), program_record(child)));
        }
        let rule = reply["rule"].as_str().unwrap_or("none matched");
        out.push(format!("rules: {} ({}), which would judge {judged} (rule: {rule})", reply["rules"]["version"].as_str().unwrap_or("?"), reply["rules"]["source"].as_str().unwrap_or("?")));
        return out.join("\n");
    }
    out.push(format!("rules: {} ({})", reply["rules"]["version"].as_str().unwrap_or("?"), reply["rules"]["source"].as_str().unwrap_or("?")));
    out.push("source: rules (no OSC 7501 report from the agent)".into());
    match reply["status"].as_str() {
        Some(status) if status != judged => out.push(format!("status: {status} (this screen judges {judged})")),
        _ => out.push(format!("status: {judged}")),
    }
    match reply["rule"].as_str() {
        Some(rule) => {
            out.push(format!("rule: {rule}"));
            if let Some(matched) = reply["matched"].as_str() {
                out.push(format!("matched: {}", matched.trim_end()));
            }
        }
        None => out.push("rule: none matched, so idle".into()),
    }
    let excluded = join(&reply["excluded"]);
    if !excluded.is_empty() {
        out.push(format!("not for this version: {excluded}"));
    }
    out.join("\n")
}

fn program_record(record: &Value) -> String {
    let mut parts = vec![format!("state={}", record["state"].as_str().unwrap_or("?"))];
    if let Some(kind) = record["kind"].as_str() {
        parts.push(format!("kind={kind}"));
    }
    if let Some(progress) = record["progress"].as_u64() {
        parts.push(format!("progress={progress}"));
    }
    for (key, name) in [("app", "app"), ("title", "title"), ("message", "msg")] {
        if let Some(text) = record[key].as_str().filter(|t| !t.is_empty()) {
            parts.push(format!("{name}={text:?}"));
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explain_says_why() {
        let reply = json!({ "pane": "p3", "agent": "claude", "agentVersion": "2.1.296", "executable": "/bin/claude", "status": "working",
            "rules": { "version": "2026.10.10.1", "source": "bundled" }, "judged": "working", "rule": "claude/working/title", "matched": "◐ Claude Code", "excluded": [] });
        assert_eq!(
            status(&json!({ "action": "explain" }), &reply),
            "p3: claude 2.1.296 (/bin/claude)\nrules: 2026.10.10.1 (bundled)\nsource: rules (no OSC 7501 report from the agent)\nstatus: working\nrule: claude/working/title\nmatched: ◐ Claude Code"
        );
    }

    #[test]
    fn explain_shows_the_agents_own_report() {
        let reply = json!({ "pane": "p3", "agent": "claude", "agentVersion": "2.1.300", "executable": "/bin/claude", "status": "needs-you",
            "rules": { "version": "2026.10.10.1", "source": "bundled" }, "judged": "idle", "rule": null, "matched": null, "excluded": [],
            "source": "osc7501",
            "reported": { "id": "", "state": "blocked", "kind": "permission", "progress": null, "app": "claude-code", "title": "", "message": "Edit \"main.rs\"?", "status": "needs-you" },
            "records": [
                { "id": "", "state": "blocked", "kind": "permission", "progress": null, "app": "claude-code", "title": "", "message": "Edit \"main.rs\"?", "status": "needs-you" },
                { "id": "build", "state": "working", "kind": null, "progress": 40, "app": "", "title": "", "message": "", "status": "working" }
            ] });
        assert_eq!(
            status(&json!({ "action": "explain" }), &reply),
            "p3: claude 2.1.300 (/bin/claude)\nstatus: needs-you\nsource: OSC 7501, the agent's own report\nrecord: state=blocked kind=permission app=\"claude-code\" msg=\"Edit \\\"main.rs\\\"?\"\nrecord \"build\": state=working progress=40\nrules: 2026.10.10.1 (bundled), which would judge idle (rule: none matched)"
        );
    }
}

#[cfg(test)]
mod blocked_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_blocked_task_lists_as_blocked_with_what_it_waits_for_and_done_names_what_it_frees() {
        let backlog = json!({ "tasks": [
            { "id": "t13", "name": "Diff viewer", "repo": "/r", "running": false, "waits": [{ "id": "t12", "pane": "p40" }] },
            { "id": "t14", "name": "Release notes", "repo": "/r", "running": false, "waits": [{ "id": "t8" }, { "id": "t12", "pane": "p40" }] },
            { "id": "t15", "name": "Free", "repo": "/r", "running": false, "waits": [] }
        ] });
        let text = task(&json!({ "action": "list" }), &json!({ "tasks": [], "backlog": backlog["tasks"] }));
        assert!(text.contains("t13   Diff viewer    Backlog  blocked") && text.contains("waits for t12"), "{text}");
        assert!(text.contains("waits for t8, t12"), "{text}");
        assert!(text.contains("t15   Free           Backlog  stopped"), "{text}");
        assert_eq!(freed(&backlog, "p40"), vec![json!({ "id": "t13", "name": "Diff viewer" })], "by its pane, and only where it was the last blocker");
        assert_eq!(freed(&backlog, "t12").len(), 1);
        let done = task(&json!({ "action": "done", "task": "p40" }), &json!({ "closed": ["p40"], "freed": freed(&backlog, "p40") }));
        assert_eq!(done, "p40 is done: closed p40\nt13 \"Diff viewer\" can start now: nothing it waits for is left");
        assert_eq!(task(&json!({ "action": "edit" }), &json!({ "task": { "id": "t14", "waits": [] } })), "t14 waits for nothing: it can start");
    }
}

#[cfg(test)]
mod workspace_tests {
    use super::*;
    use serde_json::json;

    fn reply() -> Value {
        json!({
            "windows": [
                { "id": "w31", "key": true, "desk": true, "focused": "diff", "zoomed": "diff", "shows": "ws-1", "lead": { "pane": "p280", "width": "2/5" },
                  "dock": { "pane": "Fix login", "edge": "left", "width": "2/5", "frozen": true },
                  "tabs": [{ "id": "t211", "title": "diff", "active": true, "layout": { "pane": "diff" } }, { "id": "t212", "title": "login.md", "active": false, "layout": { "split": "row", "children": [{ "pane": "p5", "size": "1/2" }, { "pane": "p6", "size": "1/2" }] } }] },
                { "id": "w19", "key": false, "desk": true, "focused": "p157", "zoomed": null, "lead": { "pane": "p157", "width": "46%" }, "tabs": [] },
                { "id": "w45", "key": false, "focused": "notes", "zoomed": null, "tabs": [{ "id": "t247", "title": "notes", "active": true, "layout": { "pane": "notes" } }] }
            ],
            "hiddenWindows": [{ "id": "w40", "key": false, "focused": "p9", "zoomed": null, "workspace": "ws-3", "tabs": [{ "id": "t30", "title": "Tab 1", "active": true, "layout": { "pane": "p9" } }] }],
            "workspaces": [
                { "id": "ws-3", "task": "task-22a4", "name": "Skills show diffs", "where": "shelf", "hidden": true, "dock": { "pane": "Skills show diffs", "id": "p401", "edge": "left", "width": "2/5", "frozen": true },
                  "focused": "p406", "zoomed": null, "tabs": [{ "id": "t246", "title": "diff --merge-base main", "active": true, "layout": { "pane": "p406" }, "panes": ["p406"] }], "windows": ["w40"], "panes": ["p406", "p9", "p401"] },
                { "id": "ws-1", "task": "task-1dfb", "name": "Fix login", "where": { "desk": "w31" }, "hidden": false, "dock": { "pane": "Fix login", "id": "p407", "edge": "left", "width": "2/5", "frozen": true },
                  "focused": "diff", "zoomed": "diff",
                  "tabs": [{ "id": "t211", "title": "diff", "active": true, "layout": { "pane": "diff" }, "panes": ["diff"] }, { "id": "t212", "title": "login.md", "active": false, "layout": { "split": "row", "children": [{ "pane": "p5", "size": "1/2" }, { "pane": "p6", "size": "1/2" }] }, "panes": ["p5", "p6"] }],
                  "windows": [], "panes": ["p400", "p5", "p6", "p407"] },
                { "id": "ws-4", "where": "backlog", "hidden": true, "dock": { "pane": "p185", "id": "p185", "edge": "left", "width": "2/5" }, "focused": "p185", "zoomed": null, "tabs": [], "windows": [], "panes": ["p185"] }
            ],
            "panes": [
                { "id": "p157", "name": null, "type": "term" }, { "id": "p185", "name": null, "type": "term" }, { "id": "p280", "name": null, "type": "term" },
                { "id": "p400", "name": "diff", "type": "term" }, { "id": "p401", "name": "Skills show diffs", "type": "term" }, { "id": "p406", "name": null, "type": "term" },
                { "id": "p407", "name": "Fix login", "type": "term" }, { "id": "p5", "name": null, "type": "md" }, { "id": "p6", "name": null, "type": "term" },
                { "id": "p9", "name": null, "type": "term" }, { "id": "p411", "name": "notes", "type": "md" }, { "id": "p500", "name": "logs", "type": "term" }
            ]
        })
    }

    fn mine(pane: &str) -> String {
        workspace(&json!({ "action": "list", "pane": pane }), &scoped(&reply(), pane).unwrap())
    }

    #[test]
    fn your_workspace_is_shown_as_it_will_be_seen() {
        assert_eq!(
            mine("p6"),
            "ws-1 (task-1dfb, Fix login), on Desk w31\n  dock: p407 (Fix login)  left 2/5  frozen\n  focus: diff, zoomed\n  *t211 \"diff\"  diff\n   t212 \"login.md\"  p5:1/2 | p6:1/2"
        );
        assert_eq!(mine("Fix login"), mine("p6"), "its agent sees the same");
        assert_eq!(
            mine("p401"),
            "ws-3 (task-22a4, Skills show diffs), on the Shelf\n  dock: p401 (Skills show diffs)  left 2/5  frozen\n  focus: p406\n  *t246 \"diff --merge-base main\"  p406\n  window w40, hidden with it\n    focus: p9\n    *t30 \"Tab 1\"  p9"
        );
        assert_eq!(mine("p185"), "ws-4, in the backlog\n  dock: p185  left 2/5\n  focus: p185", "a workspace with no task");
    }

    #[test]
    fn from_a_window_that_is_no_workspaces_it_is_that_window() {
        assert_eq!(mine("notes"), " w45  free\n  focus: notes\n  *t247 \"notes\"  notes");
        assert_eq!(mine("p157"), " w19  Desk, planning terminal p157\n  focus: p157");
        assert!(mine("p280").starts_with("*w31  Desk, showing ws-1 (task-1dfb, Fix login), planning terminal p280\n  focus: diff, zoomed\n"), "{}", mine("p280"));
        assert!(mine("p280").ends_with("\n\nws-1 (task-1dfb, Fix login), on Desk w31\n  dock: p407 (Fix login)  left 2/5  frozen\n  focus: diff, zoomed\n  *t211 \"diff\"  diff\n   t212 \"login.md\"  p5:1/2 | p6:1/2"), "the planner sees the workspace its Desk shows");
        assert_eq!(mine("p500"), "p500 (logs) isn't in a window or a workspace");
        assert!(scoped(&reply(), "p999").is_none());
    }

    #[test]
    fn your_workspace_as_json_is_the_reply_cut_to_it() {
        let ws = scoped(&reply(), "p9").unwrap();
        assert_eq!((ws["pane"].clone(), ws["at"].clone()), (json!("p9"), json!("workspace")));
        assert_eq!(ws["workspaces"].as_array().map(|w| w.iter().map(|w| w["id"].clone()).collect::<Vec<_>>()), Some(vec![json!("ws-3")]));
        assert_eq!((ws["windows"].clone(), ws["hiddenWindows"][0]["id"].clone()), (json!([]), json!("w40")));
        assert_eq!(ws["panes"].as_array().map(|p| p.iter().map(|p| p["id"].clone()).collect::<Vec<_>>()), Some(vec![json!("p401"), json!("p406"), json!("p9")]));
    }

    #[test]
    fn all_is_every_window_then_every_workspace_then_free_windows_and_panes_in_none() {
        let text = workspace(&json!({ "action": "list", "all": true }), &reply());
        assert!(text.starts_with("Windows\n*w31  Desk, showing ws-1 (task-1dfb, Fix login), planning terminal p280\n w19  Desk, planning terminal p157\n w45  free\n\nWorkspaces\nws-1 (task-1dfb, Fix login), on Desk w31\n"), "{text}");
        assert!(text.contains("\nws-3 (task-22a4, Skills show diffs), on the Shelf\n"), "{text}");
        assert!(text.contains("\nws-4, in the backlog\n  dock: p185  left 2/5\n  focus: p185\n\nFree windows\nw45\n  focus: notes\n  *t247 \"notes\"  notes\n\nUnplaced panes\np500 (logs)"), "{text}");
    }

    #[test]
    fn replies_name_the_workspace_then_its_task_and_where_it_is() {
        let hidden = json!({ "pane": { "id": "p5", "state": "running" }, "window": null, "tab": "t9", "workspace": { "id": "ws-3", "task": "task-1dfb", "where": "shelf", "hidden": true } });
        assert_eq!(opened(&json!({}), &hidden), "opened p5 in ws-3 (task-1dfb, on the Shelf), tab t9: running");
        let shown = json!({ "pane": { "id": "p5", "state": "running" }, "window": "w31", "tab": "t9", "workspace": { "id": "ws-3", "task": "task-1dfb", "where": { "desk": "w31" }, "hidden": false } });
        assert_eq!(opened(&json!({}), &shown), "opened p5 in ws-3 on w31 (task-1dfb), tab t9: running");
        assert_eq!(focused(&json!({ "pane": "spec" }), &shown), "focused spec in ws-3 on w31 (task-1dfb, key window)");
        assert_eq!(zoomed(&json!({ "pane": "spec" }), &json!({ "zoomed": true, "workspace": hidden["workspace"] })), "spec is zoomed in ws-3 (task-1dfb, on the Shelf; run again to unzoom)");
        let free = json!({ "pane": { "id": "p5", "state": "running" }, "window": "w4", "tab": "t9", "workspace": null });
        assert_eq!(opened(&json!({}), &free), "opened p5 in window w4, tab t9: running");
        assert_eq!(window(&json!({ "action": "assign", "window": "w41" }), &json!({ "window": "w41", "workspace": hidden["workspace"] })), "assigned w41 to ws-3 (task-1dfb, on the Shelf: hidden until it's pulled)");
    }
}
