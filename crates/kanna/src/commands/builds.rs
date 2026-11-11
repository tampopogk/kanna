use crate::args::Argv;
use crate::target::{Choice, Env, Session, Target};
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};

pub const HELP: &str = "kanna builds [--json]
kanna builds cancel JOB

Your build Macs' queues, in the order their jobs run, and each Mac's last finished jobs. cancel takes a queued job out
of its queue, or stops a running one.";

pub fn run(args: &[String], env: Env) -> Result<(), Failure> {
    let mut args = Argv::new(args.to_vec());
    let json_output = args.flag("--json");
    if args.flag("--help") || args.flag("-h") {
        println!("{HELP}");
        return Ok(());
    }
    let action = args.positional().unwrap_or_else(|| "list".into());
    let mut request = json!({});
    match action.as_str() {
        "list" => {}
        "cancel" => request["job"] = json!(args.positional().ok_or_else(|| fail(exit::USAGE, format!("builds cancel needs a JOB\n\n{HELP}")))?),
        other => return Err(fail(exit::USAGE, format!("builds: unknown action \"{other}\"\n\n{HELP}"))),
    }
    let target = Target::resolve(&kanna_build::Build::current(), Choice { mux: None, instance: None, background: false, fresh: false }, env)?;
    let reply = Session::connect(&target, env)?.call("build.pool", request)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&reply).unwrap_or_default());
    } else if action == "cancel" {
        println!("{}", match reply["done"].as_str() {
            Some("dequeued") => "taken out of the queue",
            Some("stopping") => "stopping",
            Some("finished") => "already finished",
            _ => "asked; it stops as soon as it reaches its guest",
        });
    } else {
        println!("{}", render(&reply));
    }
    Ok(())
}

fn age(from: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let seconds = now.saturating_sub(from) / 1000;
    if seconds < 60 { format!("{seconds}s") } else if seconds < 3600 { format!("{}m {}s", seconds / 60, seconds % 60) } else { format!("{}h {}m", seconds / 3600, seconds / 60 % 60) }
}

fn result(status: &Value) -> String {
    match status["phase"].as_str() {
        Some("exited") if status["code"] == 0 => "passed".into(),
        Some("exited") => format!("exit {}", status["code"]),
        Some("signalled") => format!("signal {}", status["signal"]),
        Some("cancelled") => "cancelled".into(),
        _ => format!("failed: {}", status["error"].as_str().unwrap_or("unknown")),
    }
}

fn render(reply: &Value) -> String {
    let macs = reply["macs"].as_array().cloned().unwrap_or_default();
    if macs.is_empty() {
        return "no build Macs: pair one (kanna machine pair) and give it a guest (kanna guests create)".into();
    }
    let mut out = String::new();
    for mac in &macs {
        let name = mac["mac"].as_str().unwrap_or("?");
        if let Some(why) = mac["unreachable"].as_str() {
            out += &format!("{name}: unreachable ({why})\n");
            continue;
        }
        let guests: Vec<String> = mac["guests"].as_array().into_iter().flatten().map(|g| format!("{} {} Xcode {}", g["name"].as_str().unwrap_or("?"), g["arch"].as_str().unwrap_or("?"), g["xcode"].as_str().unwrap_or("?"))).collect();
        if guests.is_empty() && mac["jobs"].as_array().is_none_or(Vec::is_empty) {
            continue;
        }
        out += &format!("{name}  {}\n", guests.join(", "));
        for job in mac["jobs"].as_array().into_iter().flatten() {
            let id = job["job"].as_str().unwrap_or("?");
            let label = job["label"].as_str().unwrap_or("?");
            let from = job["machine"].as_str().unwrap_or("?");
            let (mark, state) = match job["stage"].as_str() {
                Some("queued") => ((job["ahead"].as_u64().unwrap_or(0) + 1).to_string(), format!("queued {}", age(job["submitted"].as_u64().unwrap_or(0)))),
                stage => ("▶".to_string(), format!("{} {} in {}", stage.unwrap_or("running"), age(job["started"].as_u64().or(job["submitted"].as_u64()).unwrap_or(0)), job["guest"].as_str().unwrap_or("?"))),
            };
            out += &format!("  {mark} {label}  {from}  {state}  {id}\n");
        }
        for job in mac["recent"].as_array().into_iter().flatten().take(5) {
            out += &format!("  · {}  {}  {}  {} ago  {}\n", job["label"].as_str().unwrap_or("?"), job["machine"].as_str().unwrap_or("?"), result(&job["status"]), age(job["finished"].as_u64().unwrap_or(0)), job["job"].as_str().unwrap_or("?"));
        }
    }
    if out.is_empty() { "no build Macs with guests".into() } else { out.trim_end().into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_reads_in_run_order_with_finished_jobs_after() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let reply = json!({ "macs": [
            { "mac": "studio", "guests": [{ "name": "vm-a", "arch": "aarch64", "xcode": "26.1" }], "jobs": [
                { "job": "b-1", "label": "Login screen", "machine": "linux", "stage": "running", "started": now, "guest": "vm-a" },
                { "job": "b-2", "label": "Settings page", "machine": "r720", "stage": "queued", "submitted": now, "ahead": 0 },
            ], "recent": [{ "job": "b-0", "label": "Widgets", "machine": "linux", "status": { "phase": "exited", "code": 65 }, "finished": now }] },
            { "mac": "mini", "unreachable": "mini didn't answer in time" },
        ] });
        let text = render(&reply);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "studio  vm-a aarch64 Xcode 26.1");
        assert!(lines[1].contains("▶ Login screen  linux  running 0s in vm-a  b-1"), "{text}");
        assert!(lines[2].contains("1 Settings page  r720  queued 0s  b-2"), "{text}");
        assert!(lines[3].contains("· Widgets  linux  exit 65"), "{text}");
        assert!(lines[4].contains("mini: unreachable"), "{text}");
        assert!(render(&json!({ "macs": [] })).starts_with("no build Macs"));
    }
}
