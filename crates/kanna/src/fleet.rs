use crate::output;
use crate::target::Session;
use kanna_client::Failure;
use serde_json::{json, Value};

pub fn offer(mux: &mut Session, json_output: bool) -> Result<(), Failure> {
    let offer = mux.call("fleet", json!({ "action": "offer" }))?;
    if json_output {
        println!("{offer}");
    } else {
        println!("{}", output::offer(&offer));
    }
    let paired = mux.call("fleet", json!({ "action": "wait" }))?;
    if json_output {
        println!("{paired}");
    } else {
        println!("paired: {} is in the fleet of {}", offer["name"].as_str().unwrap_or("this machine"), paired["name"].as_str().unwrap_or("?"));
    }
    Ok(())
}

pub fn reachable(addrs: &Value) -> Option<String> {
    let all: Vec<&str> = addrs.as_array()?.iter().filter_map(Value::as_str).collect();
    let lan = |a: &&&str| a.starts_with("192.168.") || a.starts_with("10.") || a.starts_with("172.");
    all.iter().find(lan).or_else(|| all.iter().find(|a| !a.starts_with("127.") && !a.starts_with('['))).map(|a| a.to_string())
}
