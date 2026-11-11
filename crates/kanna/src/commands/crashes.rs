use super::{absolute, usage, Command, Flow};
use crate::args::Argv;
use crate::output::home_relative;
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "crashes",
    group: "Information",
    summary: "List Kanna's crash records, newest first, with their stacks and the macOS crash report that matches each.",
    usage: "kanna crashes [--all]\n       kanna crashes dismiss [RECORD]",
    options: &[("--all", "Also list the records already dismissed (from the crash banner, or with dismiss).")],
    notes: "Kanna.app, the daemon and the keeper each write a record in the state folder's crashes/ before they die of a bug: the reason, the crashing thread's stack, the build and the panes open. A record's report is the .ips in ~/Library/Logs/DiagnosticReports of the same process and pid, when macOS wrote one. dismiss takes away the crash banner: for RECORD and every older record, or all of them.",
    examples: &[("kanna crashes", "The crashes not yet dismissed."), ("kanna crashes --all --json", "Every record, as the daemon has it.")],
    parse,
    show,
    flow: Flow::PLAIN,
};

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let all = args.flag("--all");
    match args.positional().as_deref() {
        None | Some("list") => Ok(json!({ "action": "list", "all": all })),
        Some("dismiss") => {
            let mut out = json!({ "action": "dismiss" });
            if let Some(file) = args.positional() {
                out["file"] = json!(absolute(&file));
            }
            Ok(out)
        }
        Some(other) => Err(usage("crashes", &format!("unknown action \"{other}\" (list or dismiss)"))),
    }
}

pub fn show(args: &Value, reply: &Value) -> String {
    if args["action"] == "dismiss" {
        return match reply["dismissed"].as_u64().unwrap_or(0) {
            1 => "dismissed 1 crash record".into(),
            n => format!("dismissed {n} crash records"),
        };
    }
    let list = reply["crashes"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return if args["all"] == true { "no crash records".into() } else { "no crashes (kanna crashes --all lists the dismissed ones)".into() };
    }
    let mut out = vec![];
    for record in &list {
        let text = |key: &str| record[key].as_str().unwrap_or("?").to_string();
        let time = text("time").replace('T', " ");
        let time = time.split('.').next().unwrap_or(&time).to_string();
        let build = &record["build"];
        let commit: String = build["commit"].as_str().unwrap_or("?").chars().take(8).collect();
        let dismissed = if record["dismissed"] == true { "  (dismissed)" } else { "" };
        out.push(format!("{time}  {} (pid {}, {} {commit})  {}{dismissed}", text("process"), record["pid"], build["channel"].as_str().unwrap_or("?"), text("reason")));
        out.push(format!("  record  {}", home_relative(&text("file"))));
        out.push(format!("  report  {}", record["ips"].as_str().map_or_else(|| "none".to_string(), home_relative)));
        let stack = record["stack"].as_array().cloned().unwrap_or_default();
        for (i, frame) in kanna_crash::site(&stack).iter().take(20).enumerate() {
            let image = frame["image"].as_str().unwrap_or("?");
            let symbol = frame["symbol"].as_str().unwrap_or("?");
            out.push(match (frame["offset"].as_u64(), frame["at"].as_str()) {
                (Some(offset), _) => format!("  {i:<2} {image}  {symbol} + {offset}"),
                (None, Some(at)) => format!("  {i:<2} {image}  {symbol} ({at})"),
                (None, None) => format!("  {i:<2} {image}  {symbol}"),
            });
        }
        out.push(String::new());
    }
    out.pop();
    out.join("\n")
}
