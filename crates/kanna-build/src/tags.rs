use std::path::Path;
use std::process::Command;

pub fn git(checkout: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(checkout).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn dev_version(checkout: &Path) -> Option<String> {
    last_release(&git(checkout, &["tag", "-l", "v*"])?).map(|release| format!("{release}-dev"))
}

pub fn last_release(tags: &str) -> Option<String> {
    let [major, minor, patch] = tags.lines().filter_map(release).max()?;
    Some(format!("{major}.{minor}.{patch}"))
}

fn release(tag: &str) -> Option<[u64; 3]> {
    let mut numbers = tag.trim().strip_prefix('v')?.split('.');
    let mut number = || numbers.next().filter(|n| n.bytes().all(|b| b.is_ascii_digit()))?.parse().ok();
    let found = [number()?, number()?, number()?];
    numbers.next().is_none().then_some(found)
}

#[cfg(test)]
mod tests {
    use super::last_release;

    #[test]
    fn the_last_release_is_the_highest_and_skips_staging_tags() {
        assert_eq!(last_release("v4.0.2\nv4.10.0\nv4.9.1\nv4.11.0-staging.3\nvnext\nv4.12\nv4.12.0.1\n").as_deref(), Some("4.10.0"));
    }

    #[test]
    fn no_release_tag_is_no_version() {
        assert_eq!(last_release(""), None);
        assert_eq!(last_release("v4.1.0-staging.1\n"), None);
    }
}
