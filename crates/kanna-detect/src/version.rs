use std::cmp::Ordering;

pub fn numbers(text: &str) -> Option<Vec<u64>> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric() || (bytes[i - 1] == b'v' && (i == 1 || !bytes[i - 2].is_ascii_alphanumeric()));
        if boundary && bytes[i].is_ascii_digit() {
            let mut end = i;
            while end < bytes.len() && (bytes[end].is_ascii_digit() || (bytes[end] == b'.' && bytes.get(end + 1).is_some_and(u8::is_ascii_digit))) {
                end += 1;
            }
            let found = &text[i..end];
            if found.contains('.') {
                return found.split('.').map(|n| n.parse().ok()).collect();
            }
            i = end;
        } else {
            i += 1;
        }
    }
    None
}

pub fn compare(a: &[u64], b: &[u64]) -> Ordering {
    (0..a.len().max(b.len())).map(|i| a.get(i).copied().unwrap_or(0).cmp(&b.get(i).copied().unwrap_or(0))).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
}

pub fn compare_text(a: &str, b: &str) -> Ordering {
    compare(&numbers(a).unwrap_or_default(), &numbers(b).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_found_in_probe_output() {
        assert_eq!(numbers("2.1.296 (Claude Code)"), Some(vec![2, 1, 296]));
        assert_eq!(numbers("codex-cli 0.162.0"), Some(vec![0, 162, 0]));
        assert_eq!(numbers("mock-agent v1.4"), Some(vec![1, 4]));
        assert_eq!(numbers("no version"), None);
        assert_eq!(compare_text("2026.10.10.1", "2026.10.5.3"), Ordering::Greater);
    }
}
