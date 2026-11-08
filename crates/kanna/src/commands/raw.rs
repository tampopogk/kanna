use super::Invocation;
use crate::args::Argv;
use crate::target::Session;
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};

pub const HELP: &str = "usage: kanna raw REQUEST\n\nSends one control-protocol request as JSON and prints the reply, e.g.\n  kanna raw '{\"cmd\":\"list\"}'\n  kanna raw '{\"cmd\":\"open\",\"args\":{\"type\":\"term\",\"name\":\"a\"}}'";

pub fn run(args: &mut Argv, invocation: &Invocation, wants_help: bool) -> Result<(), Failure> {
    if wants_help {
        println!("{HELP}");
        return Ok(());
    }
    let text = args.positional().ok_or_else(|| fail(exit::USAGE, "raw: missing REQUEST, e.g. kanna raw '{\"cmd\":\"list\"}'"))?;
    let request: Value = serde_json::from_str(&text).map_err(|e| fail(exit::USAGE, format!("raw: REQUEST is not valid JSON: {e}")))?;
    let cmd = request["cmd"].as_str().ok_or_else(|| fail(exit::USAGE, "raw: REQUEST needs a \"cmd\""))?;
    let mut session = Session::connect(invocation.target, invocation.env)?;
    session.host = invocation.host.clone();
    let reply = session.call(cmd, request.get("args").cloned().unwrap_or(json!({})))?;
    println!("{reply}");
    Ok(())
}
