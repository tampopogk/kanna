use super::{usage, Command, Flow};
use crate::args::Argv;
use crate::output::{place_of, tagged, whose};
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "window",
    group: "Tabs and windows",
    summary: "Make a window one of a task's, so it shows and hides with the task, or free it.",
    usage: "kanna window assign WINDOW TASK\n       kanna window free WINDOW",
    options: &[],
    notes: "A task's windows are on screen while a Desk shows the task; otherwise they close, their panes still running, and they open again at their frames when it's shown. A window opened from a task's pane (`kanna open --window new`) or torn off its tabs is the task's already; one made anywhere else is free. TASK is a task's ID (task-1dfb) or its agent's pane. A Desk, a window with a lead (the Quiz window) and a task that hasn't started are refused. In the app: Window → Assign Window to Task and Free Window.",
    examples: &[("kanna window assign win-4 task-1dfb", "Make win-4 one of task-1dfb's windows."), ("kanna window free win-4", "Take win-4 out of its task; it stays open.")],
    parse,
    show,
    flow: Flow::PLAIN,
};

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let action = args.positional().ok_or_else(|| usage("window", "say assign or free"))?;
    let window = args.positional().ok_or_else(|| usage("window", "missing WINDOW (an ID like win-4; see `kanna workspace list --all`)"))?;
    match action.as_str() {
        "assign" => {
            let task = args.positional().ok_or_else(|| usage("window", "missing TASK (an ID like task-1dfb; see `kanna task list`)"))?;
            Ok(json!({ "action": "assign", "window": window, "workspace": task }))
        }
        "free" => Ok(json!({ "action": "free", "window": window })),
        other => Err(usage("window", &format!("unknown action \"{other}\": assign or free"))),
    }
}

pub fn show(args: &Value, reply: &Value) -> String {
    let window = reply["window"].as_str().or(args["window"].as_str()).unwrap_or("?");
    let named = |ws: &Value| tagged(ws["id"].as_str().unwrap_or("?"), &ws["task"].as_str().map(String::from).into_iter().collect::<Vec<_>>());
    match (args["action"].as_str(), whose(reply)) {
        (Some("assign"), Some(_)) => {
            let ws = &reply["workspace"];
            let tags: Vec<String> = ws["task"].as_str().map(String::from).into_iter().chain([place_of(&ws["where"]).to_string()]).collect();
            format!("assigned {window} to {}", tagged(ws["id"].as_str().unwrap_or("?"), &tags))
        }
        _ => match reply["was"]["id"].as_str() {
            Some(_) => format!("{window} is free (out of {})", named(&reply["was"])),
            None => format!("{window} is free"),
        },
    }
}
