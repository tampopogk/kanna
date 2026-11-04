use super::{caller, repo_here, usage, Call, Command, Flow};
use crate::output::{self, ago, join, tilde};
use crate::target::Session;
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "task",
    group: "Tabs and windows",
    summary: "Make, edit, list, start, stop, set up, pick, shelve, activate, park, name, message or end tasks: an agent docked with its tabs, on Kanna.app's Desk or on its repo's Shelf.",
    usage: "kanna task new NAME [FIRST-MESSAGE] [--agent CMD] [--after TASK,...] [--on MACHINE,...]\n       kanna task edit TASK --after TASK,...|none\n       kanna task ready set|clear [TASK]\n       kanna task start TASK\n       kanna task stop [TASK]\n       kanna task setup [TASK]\n       kanna task pick TASK\n       kanna task shelve [TASK]\n       kanna task move TASK MACHINE\n       kanna task list [QUERY] [--all] [--include-done]\n       kanna task activate TASK\n       kanna task park [TASK]\n       kanna task name [TASK] NAME\n       kanna task send TASK|planner TEXT\n       kanna task messages [TASK]\n       kanna task subscribe|unsubscribe EVENT [TASK]\n       kanna task events [TASK]\n       kanna task done [TASK] [--keep]",
    options: &[
        ("--agent CMD", "new: the agent to start (default: claude)."),
        ("--after TASK,...|none", "new, edit: the tasks it waits for; it can't start until each is done or ready. edit: replaces the list, and none clears it."),
        ("--on MACHINE,...", "new: the machines it may run on (see `kanna machine`); without it, any of your machines with a clone of the repo and a free runner."),
        ("--include-done", "list: also the done tasks, newest first: with QUERY, every one that matches; without, the 5 done last."),
        ("--keep", "done: end it without the repo's tasks.teardown, keeping its worktree and branch."),
        ("--all", "list: every repo's tasks, not only the one you're in."),
    ],
    notes: "A task is a window's docked pane (usually an agent) and its tabs. TASK is its ID as kanna task list shows it (task-1dfb: the first 4 of its 8 hex digits, more if another task starts the same way), any start of the ID that only one task has, with or without task- (1dfb), the full ID, or its agent's pane name or ID; its worktree and first branch are named after its full ID. Each Kanna.app has its own Desk; the Shelf is every app's. Pick claims a task for Kanna.app's Desk, and shelve gives it back to the Shelf. A Desk holds up to three of a repo's tasks, one of them active. Activate and park are Kanna.app's, which the CLI starts if it isn't running: activating shows the task as it was on the Desk your terminal is on, else on its project's Desk (the one Kanna.app had in front last), else a new one, and the task active before stays on the Desk; activating a task a Desk shows brings that window forward instead. Parking (⌘.) shows its Desk's Desk view, the planning agent with the Desk's tasks and the Shelf, and the task stays active and running. Done closes the task's panes, ending its agent and terminals; its Desk shows the Desk view. Whether a task runs and where it is are separate. Start runs a task where it is: a Shelf task gets its worktree, branch and agent and stays on the Shelf, running, and any number can run there; a stopped task's agent comes back, and a claude agent resumes its conversation. Stop ends a task's agent and hangs up its terminals, and leaves it where it is, its tabs, focus and zoom as they were and its worktree and branch kept; starting it again reruns the terminals that were running. Pick (Move to Desk) moves a Shelf task to the Desk, starting it if it's stopped; an app's Desk holds three tasks per repo. Shelve (Move to Shelf) moves a Desk task back to the Shelf as it is, running or stopped, freeing its place on the Desk. The old verbs defer and pull still work, as shelve and activate. A Shelf task can wait for other tasks (--after): it's blocked, and starting or picking it is refused until each is done or ready; `task edit` changes what it waits for until it starts, and `task done` names the tasks it frees. Ready marks a started task's work built, its tests passing, at its worktree's head (refused with uncommitted changes), and `task ready clear` clears it. A blocked task whose blockers are ready starts stacked on them: from the one blocker's ready head, or from main with each one's merged in (if they conflict, it stays blocked). When a ready task's branch moves it stops being ready, and every task stacked on it is stale, down the chain: each gets a message from kanna to merge what moved, rerun its tests and run `task ready set` again, which is refused until its bases are in its branch and none is stale. A stacked task merges after its bases, as `task list` says; once a base is in main, the task is told to merge main. A repo's tasks.setup (`kanna config schema`) runs in a new worktree's agent terminal before the agent starts, once per worktree; if it fails the task stops, `task start` starts the agent anyway, and `task setup` runs it again (in a Setup tab of a running task). Done runs tasks.teardown in the task's agent pane, its tabs closed: the task is Done once it succeeds, and a merged task's worktree and branch are removed (an unmerged one's kept); if it fails, the task stays where it is with its pane showing why, `task done` runs it again and `task done --keep` ends the task without it. A task's name is its docked pane's name, and a Shelf task's own until it starts; `task name` sets either, and a Shelf task keeps its new name when it starts. A Shelf task's first message never changes: to change it, make a new task and take the old one off with `task done`. Send leaves a message for a task's agent, or for the repo's planning agent (planner), from the task you run it in, the planner, or else you: Kanna types it into the agent's terminal as a prompt, [message from SENDER] TEXT, once the agent's turn is over and it's at its prompt (never while it asks you something, nor while you're typing there), several at once as one prompt, oldest first. A stopped Shelf task with a message runs for it, as one of the repo's shelf.runners, and a running one gets it rather than being stopped at its turn's end; a stopped Desk task gets it when you start it; a done task takes none. `task messages` lists a task's messages, sent and received, delivered or waiting, or every message. Run from a task's terminal, `task park`, `task name` and `task done` mean that task, and `kanna open`, `kanna comment`, `kanna focus` and `kanna zoom` act in its own workspace, which a Desk shows while the task is active on it.",
    examples: &[
        ("kanna task new \"Fix the page_comments pinch\" \"The mark drifts when you pinch; find out why and add a test\"", "Add a task to this repo's Shelf."),
        ("kanna task new \"Release notes\" \"Write the 0.9 notes\" --after 1dfb,c0ff", "Add a task that can't start until task-1dfb and task-c0ff are done."),
        ("kanna task edit task-3f9a --after none", "Let a blocked task start: it waits for nothing now."),
        ("kanna task new \"UI tests\" \"Run the iOS UI tests and fix what fails\" --on mini", "Add a task that only the machine named mini takes."),
        ("kanna task start 3f9a", "Start a Shelf task where it is: its own worktree and branch, and its agent with the first message."),
        ("kanna task pick 3f9a", "Pick a task off the Shelf onto the Desk, starting it if it's stopped."),
        ("kanna task stop 3f9a", "End a task's agent and hang up its terminals, leaving it and its tabs where they are; start it again to resume its conversation and rerun its terminals."),
        ("kanna task setup 3f9a", "Run the repo's tasks.setup in task-3f9a's worktree again: then its agent, if it's stopped."),
        ("kanna task list", "This repo's tasks, the Desk's then the Shelf's, and their state: working, idle, needs you, stopped, blocked, ready, stale (with what it's behind), merged (its branch is in main) or replied (its message to the planner waits); a stacked task says what it merges after."),
        ("kanna task ready set task-3f9a", "Its work is finished and its tests pass (from its own terminal, plain `kanna task ready set`), so the tasks waiting for it can start on its branch."),
        ("kanna task ready clear task-3f9a", "Its work isn't finished after all: tasks already stacked on it aren't affected."),
        ("kanna task list --all", "Every repo's tasks, naming the repo of those that aren't this one's."),
        ("kanna task list resume agent", "Only the tasks with both words in their name, first message or branch."),
        ("kanna task list task-1dfb", "The task task-1dfb, found by a start of its ID (or its agent's pane ID, like term-12); if it's done, when, and its branch if that's still there."),
        ("kanna task list send enter --include-done", "Also the done tasks with both words, to see if the work was done already."),
        ("kanna task list --include-done", "Also the 5 tasks done last."),
        ("kanna task park agent", "Show the Desk view of the Desk the task docked as agent is active on; it stays active. From its terminal, leave out agent."),
        ("kanna task activate agent", "Make the task docked as agent the active one on your Desk (the one your terminal is on), else on its project's Desk."),
        ("kanna task name agent \"Tab bar light mode\"", "Name the task docked as agent; from its terminal, leave out agent."),
        ("kanna task name task-3f9a \"Search the Shelf\"", "Rename task-3f9a, waiting on the Shelf; it starts with that name."),
        ("kanna task shelve agent", "Move the task docked as agent back to the Shelf; from its terminal, leave out agent."),
        ("kanna task move 3f9a r720-1", "Move task-3f9a from this machine's Shelf to r720-1 at its turn's end: its branch, uncommitted work and conversation go, and it carries on there."),
        ("kanna task send 71af \"Is your spec at docs/specs/x.md final?\"", "Leave task-71af a message: it gets it when its turn is over, and answers with kanna task send."),
        ("kanna task send planner \"The spec is final\"", "Tell this repo's planning agent something, when the user asks you to."),
        ("kanna task messages 71af", "task-71af's messages: sent, received, and whether each was delivered."),
        ("kanna task subscribe main-moved 71af", "task-71af also hears every move of main."),
        ("kanna task events 71af", "What task-71af hears, and every event message it was sent."),
        ("kanna task done agent", "End the task docked as agent: close its agent and tabs."),
        ("kanna task done 3f9a --keep", "End task-3f9a without running the repo's tasks.teardown, keeping its worktree and branch."),
    ],
    parse: |args| {
        let action = args.positional().ok_or_else(|| usage("task", "say what: new, edit, ready, start, stop, setup, pick, shelve, move, list, activate, park, name, send, messages, subscribe, unsubscribe, events or done"))?;
        let action = match action.as_str() {
            "defer" => "shelve".to_string(),
            "pull" => "activate".to_string(),
            _ => action.clone(),
        };
        let agent = args.option("--agent")?;
        if agent.is_some() && action != "new" {
            return Err(usage("task", "--agent is for task new"));
        }
        let after = args.option("--after")?;
        if after.is_some() && !["new", "edit"].contains(&action.as_str()) {
            return Err(usage("task", "--after is for task new and task edit"));
        }
        let after = after.map(|list| if list == "none" { vec![] } else { list.split(',').map(str::trim).filter(|id| !id.is_empty()).map(String::from).collect::<Vec<_>>() });
        let on = args.option("--on")?;
        if on.is_some() && action != "new" {
            return Err(usage("task", "--on is for task new"));
        }
        let mut out = json!({ "action": action });
        match action.as_str() {
            "list" => {
                if args.flag("--all") {
                    out["all"] = json!(true);
                }
                if let Ok(repo) = repo_here() {
                    out["repo"] = json!(repo);
                }
                if args.flag("--include-done") {
                    out["done"] = json!(true);
                }
                if let Some(query) = args.rest() {
                    out["query"] = json!(query);
                }
            }
            "new" => {
                out["name"] = json!(args.positional().ok_or_else(|| usage("task", "new needs a NAME: kanna task new NAME [FIRST-MESSAGE]"))?);
                if let Some(message) = args.positional() {
                    out["message"] = json!(message);
                }
                out["repo"] = json!(repo_here()?);
                if let Some(agent) = agent {
                    out["agent"] = json!(agent);
                }
                if let Some(after) = after {
                    out["after"] = json!(after);
                }
                if let Some(on) = on {
                    out["on"] = json!(on.split(',').map(str::trim).filter(|m| !m.is_empty()).collect::<Vec<_>>());
                }
                if let Some(by) = caller() {
                    out["by"] = json!(by);
                }
            }
            "start" => {
                out["task"] = json!(args.positional().ok_or_else(|| usage("task", "start needs a TASK (see `kanna task list`)"))?);
            }
            "edit" => {
                out["task"] = json!(args.positional().ok_or_else(|| usage("task", "edit needs a TASK from the Shelf (see `kanna task list`)"))?);
                out["after"] = json!(after.ok_or_else(|| usage("task", "edit needs --after TASK,... or --after none"))?);
            }
            "pick" => out["task"] = json!(args.positional().ok_or_else(|| usage("task", "pick needs a TASK from the Shelf (see `kanna task list`)"))?),
            "move" => {
                out["task"] = json!(args.positional().ok_or_else(|| usage("task", "move needs a TASK from the Shelf and a MACHINE: kanna task move TASK MACHINE"))?);
                out["to"] = json!(args.positional().ok_or_else(|| usage("task", "move needs a MACHINE (see `kanna machine`): kanna task move TASK MACHINE"))?);
            }
            "stop" => {
                let task = args.positional().or_else(|| caller());
                out["task"] = json!(task.ok_or_else(|| usage("task", "stop needs a TASK, or run it from the task's terminal"))?);
            }
            "ready" => {
                match args.positional().as_deref() {
                    Some("set") => {}
                    Some("clear") => out["action"] = json!("unready"),
                    _ => return Err(usage("task", "ready needs set or clear: kanna task ready set|clear [TASK]")),
                }
                let task = args.positional().or_else(|| caller());
                out["task"] = json!(task.ok_or_else(|| usage("task", "ready set and ready clear need a TASK, or run them from the task's terminal"))?);
            }
            "setup" => {
                let task = args.positional().or_else(|| caller());
                out["task"] = json!(task.ok_or_else(|| usage("task", "setup needs a TASK, or run it from the task's terminal"))?);
            }
            "park" => {
                let task = args.positional().or_else(|| caller());
                out["task"] = json!(task.ok_or_else(|| usage("task", "park needs a TASK, or run it from the task's terminal"))?);
            }
            "activate" => out["task"] = json!(args.positional().ok_or_else(|| usage("task", "activate needs a TASK (see `kanna task list`)"))?),
            "shelve" => {
                let task = args.positional().or_else(|| caller());
                out["task"] = json!(task.ok_or_else(|| usage("task", "shelve needs a TASK, or run it from the task's terminal"))?);
            }
            "name" => {
                let first = args.positional().ok_or_else(|| usage("task", "name needs a NAME: kanna task name [TASK] NAME"))?;
                let (task, name) = match args.positional() {
                    Some(name) => (first, name),
                    None => (caller().ok_or_else(|| usage("task", "say which task: kanna task name TASK NAME"))?, first),
                };
                out["task"] = json!(task);
                out["name"] = json!(name);
            }
            "send" => {
                out["to"] = json!(args.positional().ok_or_else(|| usage("task", "send needs a TASK or planner, and TEXT: kanna task send TASK TEXT"))?);
                out["text"] = json!(args.rest().ok_or_else(|| usage("task", "send needs TEXT: kanna task send TASK TEXT"))?);
                if let Ok(repo) = repo_here() {
                    out["repo"] = json!(repo);
                }
                if let Some(by) = caller() {
                    out["by"] = json!(by);
                }
            }
            "messages" => {
                if let Some(task) = args.positional() {
                    out["task"] = json!(task);
                }
            }
            "subscribe" | "unsubscribe" => {
                out["event"] = json!(args.positional().ok_or_else(|| usage("task", &format!("{action} needs an EVENT: main-conflicts, main-moved, base-merged or base-moved")))?);
                let task = args.positional().or_else(|| caller());
                out["task"] = json!(task.ok_or_else(|| usage("task", &format!("{action} needs a TASK, or run it from the task's terminal")))?);
            }
            "events" => {
                out["action"] = json!("hearing");
                let task = args.positional().or_else(|| caller());
                out["task"] = json!(task.ok_or_else(|| usage("task", "events needs a TASK, or run it from the task's terminal"))?);
            }
            "done" => {
                if args.flag("--keep") {
                    out["keep"] = json!(true);
                }
                let task = args.positional().or_else(|| caller());
                out["task"] = json!(task.ok_or_else(|| usage("task", "done needs a TASK (see `kanna task list`)"))?);
            }
            other => return Err(usage("task", &format!("unknown action \"{other}\" (new, edit, ready, start, stop, setup, pick, shelve, move, list, activate, park, name, send, messages, subscribe, unsubscribe, events or done)"))),
        }
        if ["edit", "ready", "start", "stop", "setup", "pick", "shelve", "move", "done", "park", "activate", "subscribe", "unsubscribe"].contains(&action.as_str()) || (action == "name" && out.get("task").is_some()) {
            if let Some(by) = caller() {
                out["by"] = json!(by);
            }
        }
        Ok(out)
    },
    show,
    flow: Flow { wire: Some(wire), before: Some(before), after: Some(after), ..Flow::PLAIN },
};

fn locate(request: &mut Value, mux: &mut Session) -> Result<Option<(String, Vec<(String, String)>)>, Failure> {
    let action = request["action"].as_str().unwrap_or("").to_string();
    let Some(reference) = request["task"].as_str().filter(|_| ["park", "activate", "shelve", "done", "name"].contains(&action.as_str())).map(String::from) else { return Ok(None) };
    let found = match mux.call("tasks", json!({ "action": "resolve", "task": reference })) {
        Ok(found) => Some(found["task"].clone()),
        Err(failure) if failure.message.ends_with("[ambiguous]") => return Err(failure),
        Err(_) => None,
    };
    let on_shelf = found.as_ref().is_some_and(|t| t["place"] == "shelf");
    if action == "shelve" && on_shelf {
        return Err(fail(exit::FAILED, format!("{reference} is on the Shelf: `kanna task pick {reference}` moves it to the Desk")));
    }
    let pane = found.as_ref().and_then(|t| t["pane"].as_str()).map(String::from);
    let id = found.as_ref().and_then(|t| t["id"].as_str()).map(String::from);
    match (action.as_str(), id.as_deref()) {
        ("name", Some(id)) => request["task"] = json!(id),
        ("name", None) => {
            request.as_object_mut().map(|r| r.remove("task"));
            request["pane"] = json!(reference);
        }
        ("done", Some(id)) if on_shelf => request["task"] = json!(id),
        (_, Some(_)) => request["task"] = json!(pane.unwrap_or(reference)),
        (_, None) => {}
    }
    Ok(id.map(|id| (id, found.as_ref().map(output::shorts).unwrap_or_default())))
}

fn after(request: &Value, reply: &mut Value, mux: &mut Session, call: &mut Call) -> Result<(), Failure> {
    if let (Some(waiting), Some(done)) = (&call.waiting, request["task"].as_str()) {
        reply["freed"] = Value::Array(freed(waiting, done));
    }
    if request["action"] == "list" {
        let mut asked = json!({ "action": "list", "merged": true, "done": request["done"] == true });
        if let Some(repo) = request["repo"].as_str().filter(|_| request["all"] != true) {
            asked["repo"] = json!(repo);
        }
        if let Ok(found) = mux.call("tasks", asked.clone()) {
            reply["shelf"] = found["shelf"].clone();
            reply["elsewhere"] = found["elsewhere"].clone();
            reply["twice"] = found["twice"].clone();
            reply["agent"] = found["agent"].clone();
            name_out(reply, &found["desk"]);
            if asked["repo"].is_string() {
                keep_named(reply);
            }
            call.shorts.extend(output::shorts(&found));
            call.listed = true;
            reply["script"] = found["script"].clone();
            if request["done"] == true {
                reply["done"] = found["done"].clone();
            }
        }
        if let Some(query) = request["query"].as_str() {
            keep_matching(reply, query);
            if let Some(id) = lone_task(reply, query) {
                if let Ok(shown) = mux.call("tasks", json!({ "action": "show", "task": id })) {
                    reply["found"] = shown["task"].clone();
                }
            }
        } else {
            keep_recent_done(reply);
        }
    }
    Ok(())
}

fn before(request: &mut Value, mux: &mut Session) -> Result<Call, Failure> {
    let located = locate(request, mux)?;
    let waiting = if request["action"] == "done" { mux.call("tasks", json!({ "action": "list" })).ok() } else { None };
    let (task, shorts) = located.unzip();
    Ok(Call { task, shorts: shorts.unwrap_or_default(), waiting, ..Call::default() })
}

fn wire(request: &Value) -> &'static str {
    let action = request["action"].as_str().unwrap_or("");
    match action {
        "new" | "edit" | "ready" | "unready" | "start" | "stop" | "setup" | "pick" | "shelve" | "move" | "send" | "messages" | "subscribe" | "unsubscribe" | "hearing" | "done" => "tasks",
        "name" if request.get("task").is_some() => "tasks",
        "name" => "rename-pane",
        "park" | "activate" | "desk" => "desk",
        _ => "task",
    }
}

fn done_line(task: &Value) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let when = task["doneAt"].as_f64().map_or(String::new(), |at| format!(" ({})", ago(now - at)));
    let name = task["name"].as_str().map_or(String::new(), |n| format!(" \"{n}\""));
    let branch = task["branch"].as_str().map_or(String::new(), |b| format!("; its branch {b} is still there"));
    format!("{}{name} is done{when}{branch}", task["short"].as_str().or(task["id"].as_str()).unwrap_or("?"))
}

/// The task ID a query is, when it's only that and nothing listed matches it: a task that's done, or never was.
pub fn lone_task(reply: &Value, query: &str) -> Option<String> {
    let none = ["tasks", "shelf", "done"].iter().all(|list| reply[list].as_array().is_none_or(Vec::is_empty));
    let id = query.trim().to_lowercase();
    id_start(&id).filter(|_| none && !id.contains(char::is_whitespace))
}

pub fn keep_matching(reply: &mut Value, query: &str) {
    let words: Vec<&str> = query.split_whitespace().collect();
    for list in ["tasks", "shelf", "elsewhere", "done"] {
        if let Some(tasks) = reply.get_mut(list).and_then(Value::as_array_mut) {
            tasks.retain(|task| words.iter().all(|word| task_matches(task, word)));
        }
    }
}

fn id_start(word: &str) -> Option<String> {
    let (hex, least) = match word.strip_prefix("task-") {
        Some(hex) => (hex, 1),
        None => (word, 4),
    };
    (hex.len() >= least && hex.len() <= 8 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("task-{hex}"))
}

/// Whether a query word picks out a task: a start of its task ID (`task-1dfb`, or 4 hex digits or more without `task-`), its pane ID exactly, or part of its name, first message or branch, ignoring case.
pub fn task_matches(task: &Value, word: &str) -> bool {
    let word = word.to_lowercase();
    let start = id_start(&word);
    let id = [&task["task"], &task["id"]].iter().filter_map(|v| v.as_str()).any(|id| start.as_ref().is_some_and(|start| id.to_lowercase().starts_with(start.as_str())));
    let pane = [&task["id"], &task["held"]].iter().filter_map(|v| v.as_str()).any(|pane| pane.to_lowercase() == word);
    id || pane || [&task["name"], &task["message"], &task["branch"]].iter().filter_map(|v| v.as_str()).any(|text| text.to_lowercase().contains(&word))
}

pub fn keep_named(reply: &mut Value) {
    if let Some(tasks) = reply["tasks"].as_array_mut() {
        tasks.retain(|task| task["task"].is_string());
    }
}

pub fn keep_recent_done(reply: &mut Value) {
    let Some(done) = reply.get_mut("done").and_then(Value::as_array_mut) else { return };
    let more = done.len().saturating_sub(RECENT_DONE);
    done.truncate(RECENT_DONE);
    if more > 0 {
        reply["moreDone"] = more.into();
    }
}

pub const RECENT_DONE: usize = 5;

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

pub fn place(reply: &Value) -> &'static str {
    if reply["place"] == "shelf" { " on the Shelf" } else { "" }
}

fn freed_lines(reply: &Value) -> String {
    let freed = reply["freed"].as_array().cloned().unwrap_or_default();
    freed.iter().map(|t| format!("\n{} \"{}\" can start now: nothing it waits for is left", t["id"].as_str().unwrap_or("?"), t["name"].as_str().unwrap_or(""))).collect()
}

pub fn freed(listed: &Value, done: &str) -> Vec<Value> {
    let tasks = listed["shelf"].as_array().cloned().unwrap_or_default();
    tasks
        .into_iter()
        .filter(|t| matches!(t["waits"].as_array().map(Vec::as_slice), Some([only]) if only["id"] == done || only["pane"] == done))
        .map(|t| serde_json::json!({ "id": t["id"], "name": t["name"] }))
        .collect()
}

fn ids(list: &Value) -> String {
    list.as_array().map_or(vec![], |l| l.iter().filter_map(|w| w["id"].as_str()).collect::<Vec<_>>()).join(", ")
}

fn names(list: &Value) -> String {
    list.as_array().into_iter().flatten().filter_map(|w| w["name"].as_str().map(|n| format!("\"{n}\"")).or_else(|| w["id"].as_str().map(String::from))).collect::<Vec<_>>().join(", ")
}

pub fn listed(list: &Value) -> bool {
    list.as_array().is_some_and(|l| !l.is_empty())
}

pub fn show(request: &Value, reply: &Value) -> String {
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
        Some("subscribe") | Some("unsubscribe") => {
            let (task, event) = (reply["task"].as_str().unwrap_or("?"), reply["event"].as_str().unwrap_or("?"));
            match (reply["on"] == json!(true), reply["changed"] == json!(true)) {
                (true, true) => format!("{task} hears {event}"),
                (true, false) => format!("{task} already hears {event}"),
                (false, true) => format!("{task} no longer hears {event}"),
                (false, false) => format!("{task} doesn't hear {event}"),
            }
        }
        Some("hearing") => {
            let names = |v: &Value| v.as_array().map_or(String::new(), |l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            let sent = reply["sent"].as_array().cloned().unwrap_or_default();
            let mut lines = vec![format!("hears: {}", names(&reply["hears"]))];
            let rows: Vec<[String; 4]> = sent
                .iter()
                .map(|s| {
                    let text: String = s["text"].as_str().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
                    let text = if text.chars().count() > 60 { format!("{}…", text.chars().take(59).collect::<String>()) } else { text };
                    [ago(now - s["at"].as_f64().unwrap_or(now)), s["event"].as_str().unwrap_or("?").to_string(), if s["delivered"].is_null() { "waiting" } else { "delivered" }.to_string(), text]
                })
                .collect();
            if !rows.is_empty() {
                let head = ["SENT", "EVENT", "STATE", "TEXT"];
                let width = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0).max(head[i].len());
                let line = |r: [&str; 4]| format!("{:w0$}  {:w1$}  {:w2$}  {}", r[0], r[1], r[2], r[3], w0 = width(0), w1 = width(1), w2 = width(2)).trim_end().to_string();
                lines.push(line(head));
                lines.extend(rows.iter().map(|r| line([&r[0], &r[1], &r[2], &r[3]])));
            }
            lines.join("\n")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::parse;

    #[test]
    fn ready_set_and_clear_name_a_task_or_the_one_they_run_in() {
        assert_eq!(parse("kanna task ready set 3f9a").unwrap().1, json!({ "action": "ready", "task": "3f9a" }));
        assert_eq!(parse("kanna task ready clear 3f9a").unwrap().1, json!({ "action": "unready", "task": "3f9a" }));
        assert!(parse("kanna task ready").is_err(), "outside a task's terminal it needs a TASK");
        assert!(parse("kanna task ready 3f9a").is_err(), "it says set or clear");
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
        let text = show(&json!({ "action": "list", "repo": "/r" }), &json!({ "tasks": [], "shelf": shelf }));
        assert_eq!(text, "Desk\n(empty)\n\nShelf\nt1  A  ready\nt2  B  stale    behind \"A\"\nt3  C  ready    merges after \"B\"\nt4  D  blocked  its blockers' branches conflict");
        let ready = show(&json!({ "action": "ready" }), &json!({ "task": { "id": "t1", "ready": mark }, "freed": [{ "id": "t2", "name": "B" }] }));
        assert_eq!(ready, "t1 is ready at 012345678 (screenshot)\nt2 \"B\" can start now: nothing it waits for is left");
        assert_eq!(show(&json!({ "action": "unready" }), &json!({ "task": { "id": "t1" } })), "t1 isn't ready");
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
        let text = show(&json!({ "action": "list", "repo": "/r" }), &json!({ "tasks": [], "shelf": listed["shelf"] }));
        assert!(text.contains("t13  Diff viewer    blocked  waits for t12\n"), "{text}");
        assert!(text.contains("blocked  waits for t8, t12\n"), "{text}");
        assert!(text.ends_with("t15  Free           stopped"), "{text}");
        assert_eq!(freed(&listed, "p40"), vec![json!({ "id": "t13", "name": "Diff viewer" })], "by its pane, and only where it was the last blocker");
        assert_eq!(freed(&listed, "t12").len(), 1);
        let done = show(&json!({ "action": "done", "task": "p40" }), &json!({ "closed": ["p40"], "freed": freed(&listed, "p40") }));
        assert_eq!(done, "p40 is done: closed p40\nt13 \"Diff viewer\" can start now: nothing it waits for is left");
        assert_eq!(show(&json!({ "action": "edit" }), &json!({ "task": { "id": "t14", "waits": [] } })), "t14 waits for nothing: it can start");
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
        let text = show(&json!({ "action": "list", "repo": "/r" }), &reply);
        assert_eq!(
            text,
            "Desk\nt1  Fix login      merged\nt2  Diff viewer    working\n\nShelf\nt3  Release notes  replied\nt4  Port           stopped  codex  /other\nt5  Later          blocked  waits for \"Diff viewer\", t9"
        );
        let empty = show(&json!({ "action": "list" }), &json!({ "tasks": [], "shelf": [reply["shelf"][0].clone()] }));
        assert!(empty.starts_with("Desk\n(empty)\n\nShelf\nt3"), "{empty}");
        let found = show(&json!({ "action": "list", "query": "release" }), &json!({ "tasks": [], "shelf": [reply["shelf"][0].clone()] }));
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
