use crate::target::Session;
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Remote {
    pub host: String,
    pub machine: String,
    pub task: String,
}

impl Remote {
    pub fn explain(&self, mut failure: Failure) -> Failure {
        if let Some(why) = failure.message.strip_suffix(" [unreachable]") {
            return fail(exit::UNREACHABLE, format!("{} runs on {}, which is unreachable: {why}", self.task, self.machine));
        }
        if failure.code == exit::UNREACHABLE {
            failure.message = format!("{} runs on {}, which is unreachable: {}", self.task, self.machine, failure.message);
        }
        failure
    }
}

pub fn task_like(reference: &str) -> bool {
    let lower = reference.to_lowercase();
    let (hex, least) = match lower.strip_prefix("task-") {
        Some(hex) => (hex, 1),
        None => (lower.as_str(), 4),
    };
    (least..=8).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn locate(mux: &mut Session, reference: &str) -> Result<Value, Failure> {
    match mux.call("tasks", json!({ "action": "locate", "task": reference })) {
        Ok(found) => Ok(found),
        Err(failure) if failure.code == exit::USAGE || failure.code == exit::UNSUPPORTED => Ok(Value::Null),
        Err(failure) => Err(failure),
    }
}

pub fn label(found: &Value) -> String {
    let task = &found["task"];
    let short = task["short"].as_str().or(task["id"].as_str()).unwrap_or("?");
    match task["name"].as_str() {
        Some(name) => format!("{short} \"{name}\""),
        None => short.to_string(),
    }
}

pub fn elsewhere(found: &Value) -> Result<Option<Remote>, Failure> {
    let Some(machine) = found["machine"].as_str() else { return Ok(None) };
    let task = label(found);
    match found["host"].as_str() {
        Some(host) => Ok(Some(Remote { host: host.to_string(), machine: machine.to_string(), task })),
        None => Err(fail(exit::UNREACHABLE, format!("{task} runs on {machine}, which is unreachable: it isn't one of your machines any more (see `kanna machine`)"))),
    }
}

fn agent_pane(found: &Value) -> Result<String, Failure> {
    if found["done"] == true {
        return Err(fail(exit::NOT_FOUND, format!("{} is done, so it has no agent pane", label(found))));
    }
    let short = found["task"]["short"].as_str().unwrap_or("TASK");
    found["pane"].as_str().map(String::from).ok_or_else(|| fail(exit::NOT_FOUND, format!("{} has no agent pane: it hasn't started (`kanna task start {short}`)", label(found))))
}

pub fn panes(fields: &[&str], request: &mut Value, mux: &mut Session) -> Result<Option<Remote>, Failure> {
    let chosen = mux.host.clone();
    let mut routed: Option<Remote> = None;
    let mut plain = false;
    for field in fields {
        let Some(reference) = request[*field].as_str().map(String::from) else { continue };
        if !task_like(&reference) {
            plain = true;
            continue;
        }
        let found = locate(mux, &reference).map_err(|f| explained(&routed, f))?;
        if found["task"].is_null() {
            plain = true;
            continue;
        }
        let pane = match elsewhere(&found)? {
            None => agent_pane(&found).map_err(|f| explained(&routed, f))?,
            Some(there) => {
                if let Some(host) = &chosen {
                    return Err(fail(exit::NOT_FOUND, format!("{} runs on {}, not {host}", there.task, there.machine)));
                }
                if plain || routed.is_some() {
                    return Err(fail(exit::USAGE, format!("{} runs on {}: name panes and tasks of one machine", there.task, there.machine)));
                }
                mux.host = Some(there.host.clone());
                let id = found["task"]["id"].as_str().unwrap_or(&reference).to_string();
                let pane = locate(mux, &id).and_then(|found| agent_pane(&found)).map_err(|f| there.explain(f));
                routed = Some(there);
                pane?
            }
        };
        request[*field] = json!(pane);
    }
    Ok(routed)
}

fn explained(routed: &Option<Remote>, failure: Failure) -> Failure {
    match routed {
        Some(remote) => remote.explain(failure),
        None => failure,
    }
}

pub fn task(request: &mut Value, mux: &mut Session) -> Result<Option<Remote>, Failure> {
    if mux.host.is_some() {
        return Ok(None);
    }
    let action = request["action"].as_str().unwrap_or("").to_string();
    let field = match action.as_str() {
        "send" => "to",
        "new" | "list" => return Ok(None),
        _ => "task",
    };
    let Some(reference) = request[field].as_str().map(String::from).filter(|r| task_like(r)) else { return Ok(None) };
    let found = locate(mux, &reference)?;
    let Some(there) = elsewhere(&found)? else { return Ok(None) };
    let id = found["task"]["id"].as_str().unwrap_or(&reference).to_string();
    let by = request.as_object_mut().and_then(|r| r.remove("by"));
    if matches!(action.as_str(), "activate" | "park") {
        mux.host = Some(there.host.clone());
        let pane = locate(mux, &id).map_err(|f| there.explain(f)).and_then(|found| agent_pane(&found));
        mux.host = None;
        request["task"] = json!(format!("{}@{}", pane?, there.machine));
        if let Some(by) = by {
            request["by"] = by;
        }
        return Ok(None);
    }
    if action == "send" {
        if let Ok(me) = mux.call("tasks", json!({ "action": "sender", "by": by })) {
            if me["from"] != "user" {
                request["sender"] = me["from"].clone();
                if me["repo"].is_string() {
                    request["repo"] = me["repo"].clone();
                }
            }
        }
    }
    if action == "pick" {
        if let Some(me) = found["me"].as_str() {
            request["desk"] = json!(me);
        }
    }
    request[field] = json!(id);
    mux.host = Some(there.host.clone());
    Ok(Some(there))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_could_be_a_task_id_is_looked_up() {
        for task in ["task-1", "task-1dfb", "1dfb", "1DFB", "task-1dfb0c12", "1dfb0c12"] {
            assert!(task_like(task), "{task}");
        }
        for pane in ["term-21", "p1", "agent", "1df", "task-", "task-1dfb0c12a", "server", "term-1@vm", "planner"] {
            assert!(!task_like(pane), "{pane}");
        }
    }

    #[test]
    fn an_unreachable_machine_is_named_with_the_task() {
        let remote = Remote { host: "vm".into(), machine: "vm".into(), task: "task-1dfb \"Fix\"".into() };
        let failure = remote.explain(fail(exit::FAILED, "can't reach vm within 10s [unreachable]"));
        assert_eq!((failure.code, failure.message.as_str()), (exit::UNREACHABLE, "task-1dfb \"Fix\" runs on vm, which is unreachable: can't reach vm within 10s"));
        let other = remote.explain(fail(exit::NOT_FOUND, "vm: no pane \"x\" [not_found]"));
        assert_eq!(other.message, "vm: no pane \"x\" [not_found]");
    }
}
