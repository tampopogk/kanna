use crate::args::Argv;
use crate::target::{Choice, Env, Session, Target};
use kanna_client::{exit, fail, Failure};
use serde_json::json;

const GUESTS_HELP: &str = "kanna guests [list]
kanna guests create NAME --from MASTER_VM_DIR [--cpus N] [--memory GIB] [--data GIB]
kanna guests data NAME [--data GIB]
kanna guests start|stop|retire NAME

A build Mac's guests: macOS VMs that run builds for one tenant, each an APFS clone of a master VM folder with its own
machine identity and a data disk (default 64 GiB, the volume KannaData) for its builds. `data` adds one to a guest made
without it, attached when the guest next starts. The daemon keeps every guest that isn't stopped running; retire destroys
its disks.";

pub fn run(args: &[String], env: Env) -> Result<(), Failure> {
    let mut args = Argv::new(args.to_vec());
    let json_output = args.flag("--json");
    if args.flag("--help") || args.flag("-h") {
        println!("{GUESTS_HELP}");
        return Ok(());
    }
    let action = args.positional().unwrap_or_else(|| "list".into());
    let mut request = json!({ "action": action });
    if action != "list" {
        request["name"] = json!(args.positional().ok_or_else(|| fail(exit::USAGE, format!("guests {action} needs a NAME\n\n{GUESTS_HELP}")))?);
    }
    if let Some(from) = args.option("--from")? {
        let from = std::fs::canonicalize(&from).map_err(|e| fail(exit::USAGE, format!("guests: {from}: {e}")))?;
        request["from"] = json!(from);
    }
    for (flag, key) in [("--cpus", "cpus"), ("--memory", "memory"), ("--data", "data")] {
        if let Some(value) = args.option(flag)? {
            request[key] = json!(value.parse::<u64>().map_err(|_| fail(exit::USAGE, format!("guests: {flag} takes a number")))?);
        }
    }
    let target = Target::resolve(&kanna_build::Build::current(), Choice { mux: None, instance: None, background: false, fresh: false }, env)?;
    let reply = Session::connect(&target, env)?.call("build", request)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&reply).unwrap_or_default());
    } else if let Some(list) = reply["guests"].as_array() {
        if list.is_empty() {
            println!("no guests: kanna guests create NAME --from MASTER_VM_DIR");
        }
        for guest in list {
            let state = if guest["stopped"] == true { "stopped" } else if guest["socket"] == true { "running" } else { "starting" };
            println!("{:<16} {state}", guest["name"].as_str().unwrap_or_default());
        }
    } else {
        println!("{}", reply["guest"].as_str().unwrap_or("done"));
    }
    Ok(())
}
