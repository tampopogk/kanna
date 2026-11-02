use serde_json::Value;

pub fn protocols(value: &Value) -> Option<[u32; 2]> {
    let pair = value.as_array()?;
    let lo = u32::try_from(pair.first()?.as_u64()?).ok()?;
    let hi = u32::try_from(pair.get(1)?.as_u64()?).ok()?;
    (pair.len() == 2 && lo <= hi).then_some([lo, hi])
}

pub fn common(mine: [u32; 2], theirs: [u32; 2]) -> Option<u32> {
    let top = mine[1].min(theirs[1]);
    (top >= mine[0].max(theirs[0])).then_some(top)
}

pub fn agreed(ack: &Value, mine: [u32; 2]) -> bool {
    match ack["protocol"].as_u64() {
        Some(protocol) => u32::try_from(protocol).is_ok_and(|p| mine[0] <= p && p <= mine[1]),
        None => common(mine, [1, 1]).is_some(),
    }
}

pub fn short(commit: &str) -> String {
    let dirty = if commit.ends_with("-dirty") { "-dirty" } else { "" };
    format!("{}{dirty}", commit.chars().take(8).collect::<String>())
}

fn span(p: [u32; 2]) -> String {
    if p[0] == p[1] { p[0].to_string() } else { format!("{}–{}", p[0], p[1]) }
}

pub fn other_protocol(name: &str, theirs: &Value, mine: [u32; 2]) -> String {
    let their = protocols(&theirs["protocols"]).unwrap_or([1, 1]);
    let runs = match (theirs["version"].as_str(), theirs["commit"].as_str()) {
        (Some(version), Some(commit)) => format!("Kanna {version} ({})", short(commit)),
        (Some(version), None) => format!("Kanna {version}"),
        _ => "an older Kanna".into(),
    };
    let fix = if their[1] < mine[0] { format!("it's older: update Kanna on {name}") } else { "this machine is older: update it".to_string() };
    format!("{name} runs {runs}, protocol {}, and this machine speaks {}; {fix}", span(their), span(mine))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn protocols_meet_at_the_highest_both_speak() {
        assert_eq!(common([1, 3], [2, 5]), Some(3));
        assert_eq!(common([1, 1], [1, 1]), Some(1));
        assert_eq!(common([1, 2], [3, 4]), None);
        assert_eq!(protocols(&json!([2, 1])), None);
        assert_eq!(protocols(&json!([1, 2])), Some([1, 2]));
    }

    #[test]
    fn an_ack_must_name_a_protocol_this_side_speaks_and_none_means_1() {
        assert!(agreed(&json!({ "ok": true, "protocol": 2 }), [1, 3]));
        assert!(!agreed(&json!({ "ok": true, "protocol": 4 }), [1, 3]));
        assert!(agreed(&json!({ "ok": true }), [1, 2]));
        assert!(!agreed(&json!({ "ok": true }), [2, 3]));
    }

    #[test]
    fn the_message_says_which_side_is_older() {
        let old = json!({ "protocols": [1, 1], "version": "4.0.1", "commit": "d7cb0a1e".repeat(5) });
        assert_eq!(other_protocol("r720-1", &old, [2, 3]), "r720-1 runs Kanna 4.0.1 (d7cb0a1e), protocol 1, and this machine speaks 2–3; it's older: update Kanna on r720-1");
        let new = json!({ "protocols": [4, 5], "version": "4.2.0" });
        assert_eq!(other_protocol("vm", &new, [2, 3]), "vm runs Kanna 4.2.0, protocol 4–5, and this machine speaks 2–3; this machine is older: update it");
    }
}
