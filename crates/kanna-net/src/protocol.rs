use serde_json::Value;

pub fn of(value: &Value) -> Option<u32> {
    u32::try_from(value["protocol"].as_u64()?).ok()
}

pub fn short(commit: &str) -> String {
    let dirty = if commit.ends_with("-dirty") { "-dirty" } else { "" };
    format!("{}{dirty}", commit.chars().take(8).collect::<String>())
}

pub fn other_protocol(name: &str, theirs: &Value, mine: u32) -> String {
    let their = of(theirs);
    let runs = match (theirs["version"].as_str(), theirs["commit"].as_str()) {
        (Some(version), Some(commit)) => format!("Kanna {version} ({})", short(commit)),
        (Some(version), None) => format!("Kanna {version}"),
        _ => "an older Kanna".into(),
    };
    let speaks = their.map_or("an older protocol".to_string(), |p| format!("protocol {p}"));
    let fix = if their.is_none_or(|p| p < mine) { format!("it's older: update Kanna on {name}") } else { "this machine is older: update it".to_string() };
    format!("{name} runs {runs}, {speaks}, and this machine speaks {mine}; {fix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_message_says_which_side_is_older() {
        let old = json!({ "protocol": 3, "version": "4.0.1", "commit": "d7cb0a1e".repeat(5) });
        assert_eq!(other_protocol("r720-1", &old, 4), "r720-1 runs Kanna 4.0.1 (d7cb0a1e), protocol 3, and this machine speaks 4; it's older: update Kanna on r720-1");
        let new = json!({ "protocol": 5, "version": "4.2.0" });
        assert_eq!(other_protocol("vm", &new, 4), "vm runs Kanna 4.2.0, protocol 5, and this machine speaks 4; this machine is older: update it");
        let before = json!({ "protocols": [2, 3], "version": "4.0.0" });
        assert_eq!(other_protocol("vm", &before, 4), "vm runs Kanna 4.0.0, an older protocol, and this machine speaks 4; it's older: update Kanna on vm");
    }
}
