use crate::pair::{hello_b, pake_bytes, pake_hex, Party, Role};
use crate::{hex, Client, Failure, SecretKey};
use serde_json::{json, Value};

pub const VERSION: u64 = 1;

pub fn name_of(key: &str) -> String {
    format!("iphone-{}", key.chars().take(4).collect::<String>())
}

pub fn instance_of(key: &str) -> Option<String> {
    let bytes = crate::unhex(key)?;
    Some(data_encoding::BASE32_NOPAD.encode(&bytes).to_ascii_lowercase())
}

pub fn descriptor(key: &str) -> Value {
    json!({ "name": name_of(key), "key": key, "role": "phone", "addrs": [], "capabilities": {} })
}

pub async fn join(client: &Client, secret: &SecretKey, offer: &Value) -> Result<Value, Failure> {
    let machine = offer["key"].as_str().unwrap_or_default();
    let code = offer["code"].as_str().ok_or_else(|| Failure::new("bad_request", "that code has no secret in it"))?;
    let addrs: Vec<String> = offer["addrs"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let (mut send, mut recv, ack) = client.lines(machine, &addrs, None, json!({ "kind": "pair", "joining": true })).await?;
    let ha = ack["hello"].clone();
    if ha["device"]["key"] != machine {
        return Err(crate::Refused::new("pake_confirmation_failed").into());
    }
    let key = client.key();
    let mut session = [0u8; 16];
    getrandom::getrandom(&mut session).map_err(|e| Failure::new("failed", e.to_string()))?;
    let hb = hello_b(&descriptor(&key), &hex(&session));
    let mut party = Party::new(Role::B, code, ha.clone(), hb.clone());
    send.write(&json!({ "hello": hb, "pake": pake_hex(&party.outbound) })).await?;
    let reply = recv.read().await?;
    if reply["ok"] != true {
        return Err(Failure::from_reply(&reply));
    }
    let proof = party.finish(secret, &pake_bytes(&reply["pake"])?)?;
    party.confirm(&reply["proof"])?;
    send.write(&json!({ "proof": proof })).await?;
    let reply = recv.read().await?;
    if reply["ok"] != true {
        return Err(Failure::from_reply(&reply));
    }
    let log = party.open(&reply["handoff"], &key)?;
    let sealed = &reply["handoff"]["payload"];
    send.write(&json!({ "ok": true })).await?;
    Ok(json!({ "name": name_of(&key), "machine": ha["device"]["name"], "key": machine, "addrs": addrs, "relay": sealed["relay"], "relay_token": sealed["relay_token"], "log": log.export() }))
}
