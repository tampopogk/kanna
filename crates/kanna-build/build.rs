#[path = "src/tags.rs"]
mod tags;

use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-env-changed=KANNA_CHANNEL");
    println!("cargo::rerun-if-env-changed=KANNA_VERSION");
    println!("cargo::rerun-if-changed=build.rs");
    let channel = std::env::var("KANNA_CHANNEL").ok().filter(|c| !c.is_empty()).unwrap_or_else(|| "dev".into());
    if !["production", "staging", "dev"].contains(&channel.as_str()) {
        panic!("KANNA_CHANNEL is \"{channel}\": use production, staging or dev");
    }
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let checkout = manifest.join("../..").canonicalize().unwrap_or_else(|_| manifest.join("../.."));
    let version = std::env::var("KANNA_VERSION").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| tag_version(&checkout));
    if !valid_version(&version) {
        panic!("KANNA_VERSION is \"{version}\": use X.Y.Z or X.Y.Z-PRERELEASE");
    }
    let dot_git = checkout.join(".git");
    println!("cargo::rerun-if-changed={}", dot_git.display());
    let worktree = if dot_git.is_file() { checkout.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { String::new() };
    println!("cargo::rustc-env=KANNA_BUILD_CHANNEL={channel}");
    println!("cargo::rustc-env=KANNA_BUILD_VERSION={version}");
    println!("cargo::rustc-env=KANNA_BUILD_CHECKOUT={}", checkout.display());
    println!("cargo::rustc-env=KANNA_BUILD_WORKTREE={worktree}");
    println!("cargo::rerun-if-env-changed=KANNA_COMMIT");
    let commit = std::env::var("KANNA_COMMIT").ok().filter(|c| !c.is_empty()).or_else(|| head(&checkout)).unwrap_or_else(|| "unknown".into());
    println!("cargo::rustc-env=KANNA_BUILD_COMMIT={commit}");
}

fn tag_version(checkout: &std::path::Path) -> String {
    if let Some(common) = tags::git(checkout, &["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
        let common = PathBuf::from(common);
        for path in [common.join("refs/tags"), common.join("packed-refs")] {
            if path.exists() {
                println!("cargo::rerun-if-changed={}", path.display());
            }
        }
    }
    tags::dev_version(checkout).unwrap_or_else(|| panic!("no vX.Y.Z tag to read the version from in {}: run git fetch --tags, or set KANNA_VERSION", checkout.display()))
}

fn head(checkout: &std::path::Path) -> Option<String> {
    let git = |args: &[&str]| tags::git(checkout, args);
    let dir = git(&["rev-parse", "--absolute-git-dir"])?;
    println!("cargo::rerun-if-changed={dir}/HEAD");
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]).unwrap_or(dir);
        println!("cargo::rerun-if-changed={common}/{reference}");
    }
    git(&["rev-parse", "--short=12", "HEAD"])
}

fn valid_version(version: &str) -> bool {
    let (core, pre) = version.split_once('-').unwrap_or((version, ""));
    let numbers: Vec<&str> = core.split('.').collect();
    numbers.len() == 3
        && numbers.iter().all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        && (!version.contains('-') || (!pre.is_empty() && pre.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))))
}
