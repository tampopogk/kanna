//! Locating the workspace binaries this crate's process-level fixtures spawn.
//!
//! A handful of tests here drive a *real* `kanna-daemon` or
//! `kanna-task-transfer` child process rather than a scripted stand-in. Those
//! binaries belong to sibling packages, so Cargo never hands this crate's test
//! binaries a `CARGO_BIN_EXE_*` for them the way it does inside those packages'
//! own `tests/*.rs`; the fixture has to find the artifact on disk.
//!
//! Finding it was where this went wrong. Every fixture had grown its own
//! lookup, and all of them searched only `.build/{debug,release}/<name>` — the
//! layout a plain `cargo build`/`cargo test` writes. But this workspace's own
//! build system does not use that layout: `./kd build sidecars` builds with
//! `--target <host-triple>` and lands its artifacts in
//! `.build/<host-triple>/<profile>/`, which `stageSidecars`
//! (`tools/kd/src/runtime/sidecars.ts`) treats as the *primary* location and
//! the non-triple directory as legacy. So the lookups and the build disagreed
//! about where this workspace puts a binary, and what filled the gap was an
//! accident: `cargo test -p <pkg>` also links that package's `[[bin]]` target
//! into `.build/<profile>/`. Whether a fixture found its binary therefore
//! depended on which unrelated packages had been tested in the tree first, and
//! the suite reported a different failure count on a fresh tree than on a warm
//! one. This module is the single lookup, and it searches the layout the build
//! system actually produces first.
//!
//! The other half of the fix is what happens when the binary genuinely is not
//! there. A missing artifact is a *precondition of the environment*, not a
//! defect in the code under test, and fifteen panics saying "build it first"
//! is what made this suite unreadable. So by default an absent binary skips
//! its test the way
//! `http_api::lan_listener`'s real-address test skips a host with no
//! non-loopback interface: one `eprintln!` and a return.
//!
//! Skipping silently is its own failure, though — it is how real-process
//! coverage disappears while the gate stays green. So the canonical lane
//! (`./kd test rust`, which builds the sidecars before it tests anything) sets
//! `KANNA_REQUIRE_TEST_FIXTURE_BINARIES=1`, and under that flag an absent
//! binary is a failure again. Ad-hoc runs stay readable; the gate stays honest.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A workspace binary a fixture in this crate spawns as a real child process.
pub(crate) struct FixtureBinary {
    /// The `[[bin]]` name, which is also its file name on disk.
    pub name: &'static str,
    /// Env var naming an already-built binary, for a caller that built it
    /// somewhere this module would not look.
    pub override_var: &'static str,
    /// The command that produces it, quoted back to whoever is reading the
    /// skip message.
    pub build_command: &'static str,
}

pub(crate) const KANNA_DAEMON: FixtureBinary = FixtureBinary {
    name: "kanna-daemon",
    override_var: "KANNA_DAEMON_TEST_BIN",
    build_command: "cargo build -p kanna-daemon",
};

pub(crate) const KANNA_TASK_TRANSFER: FixtureBinary = FixtureBinary {
    name: "kanna-task-transfer",
    override_var: "KANNA_TASK_TRANSFER_TEST_BIN",
    build_command: "cargo build -p kanna-task-transfer",
};

/// Resolves `fixture`'s binary, or reports that this tree does not have one.
///
/// Returns `None` after printing why, so the caller can skip. Panics instead
/// when `KANNA_REQUIRE_TEST_FIXTURE_BINARIES` is set, because a lane that
/// promised to build the fixture and then did not must not pass quietly.
pub(crate) fn locate_or_skip(fixture: &FixtureBinary) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(fixture.override_var) {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "{} does not name a file: {path:?}",
            fixture.override_var
        );
        return Some(path);
    }

    let candidates = candidate_paths(fixture.name);
    if let Some(found) = candidates.iter().find(|candidate| candidate.is_file()) {
        return Some(found.clone());
    }

    let looked_in = candidates
        .iter()
        .map(|candidate| candidate.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let message = format!(
        "{} is not built in this tree, so the fixtures that spawn it have nothing to run. \
         Build it with `{}` or `./kd build sidecars`, or set {} to an already-built binary. \
         Looked in: {looked_in}",
        fixture.name, fixture.build_command, fixture.override_var,
    );
    assert!(
        !fixtures_are_required(),
        "{message} (KANNA_REQUIRE_TEST_FIXTURE_BINARIES is set, so this is a failure rather \
         than a skip: the lane that set it was supposed to build this binary first)"
    );
    eprintln!("skipping: {message}");
    None
}

/// Whether an absent fixture binary is a failure rather than a skip.
fn fixtures_are_required() -> bool {
    required_from(
        std::env::var("KANNA_REQUIRE_TEST_FIXTURE_BINARIES")
            .ok()
            .as_deref(),
    )
}

/// The flag's meaning, separated from the process environment so it can be
/// asserted on without a concurrent fixture test reading a mutation of it.
fn required_from(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true"))
}

/// Where `name` could be, most canonical first.
///
/// Both `.build` layouts are searched for both profiles, plus the plain
/// sibling-of-this-test-binary directory in case `build-dir` is ever unset.
/// The profile this test binary was itself built under comes first, so a tree
/// holding both a debug and a release artifact uses the matching one.
fn candidate_paths(name: &str) -> Vec<PathBuf> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/kanna-server sits two path segments under the workspace root");
    let build_root = workspace_root.join(".build");

    let own_profile = own_profile_dir();
    let profiles = match own_profile
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|profile| profile.to_str())
    {
        Some("release") => ["release", "debug"],
        _ => ["debug", "release"],
    };

    let mut candidates = Vec::new();
    for profile in profiles {
        // What `./kd build sidecars` produces: `cargo build --target <triple>`.
        if let Some(triple) = host_target_triple() {
            candidates.push(build_root.join(triple).join(profile).join(name));
        }
        // What a plain `cargo build`/`cargo test` in this workspace produces.
        candidates.push(build_root.join(profile).join(name));
    }
    if let Some(profile_dir) = own_profile {
        candidates.push(profile_dir.join(name));
    }
    candidates
}

/// The profile directory holding this test binary's own `deps/`.
///
/// This workspace splits `build-dir` (`.build/cargo-build`) from `target-dir`
/// (`.build`), so this is *not* a sibling of where named `[[bin]]` artifacts
/// land — the profile name is the only thing the two layouts share.
fn own_profile_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.parent().and_then(Path::parent).map(Path::to_path_buf)
}

/// This host's Rust target triple, asked of `rustc` exactly as
/// `tools/kd/src/runtime/sidecars.ts` asks it, and cached for the process.
///
/// `None` only if `rustc` cannot be run at all, in which case the triple
/// layout is simply not searched.
fn host_target_triple() -> Option<&'static str> {
    static TRIPLE: OnceLock<Option<String>> = OnceLock::new();
    TRIPLE
        .get_or_init(|| {
            let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
            let output = std::process::Command::new(rustc).arg("-vV").output().ok()?;
            String::from_utf8(output.stdout)
                .ok()?
                .lines()
                .find_map(|line| line.trim().strip_prefix("host:"))
                .map(|host| host.trim().to_string())
        })
        .as_deref()
}

/// Resolves a [`FixtureBinary`] or returns from the calling test.
///
/// Only valid directly inside a `#[test]`/`#[tokio::test]` body: the skip has
/// to leave the test, not a helper it called.
macro_rules! fixture_binary_or_skip {
    ($fixture:expr) => {
        match $crate::test_fixture_binaries::locate_or_skip(&$fixture) {
            Some(path) => path,
            None => return,
        }
    };
}

pub(crate) use fixture_binary_or_skip;

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression this module exists for: the lookup must know the layout
    /// `./kd build sidecars` actually writes, not only the one a bare
    /// `cargo build` happens to leave behind — and must prefer it, because
    /// that is the artifact this workspace's build system calls primary.
    #[test]
    fn the_host_triple_layout_is_searched_and_preferred_over_the_plain_one() {
        let triple = host_target_triple().expect("rustc must be runnable to compile this test");
        let build_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
            .join(".build");
        let candidates = candidate_paths("kanna-daemon");

        let triple_index = candidates
            .iter()
            .position(|candidate| {
                *candidate == build_root.join(triple).join("debug").join("kanna-daemon")
            })
            .expect("the `cargo build --target <triple>` layout must be searched");
        let plain_index = candidates
            .iter()
            .position(|candidate| *candidate == build_root.join("debug").join("kanna-daemon"))
            .expect("the plain `cargo build` layout must still be searched");

        assert!(
            triple_index < plain_index,
            "the build system's own primary layout must be preferred: {candidates:?}"
        );
    }

    /// Absent-binary handling is a *skip* by default and a *failure* only
    /// under the canonical lane's flag; nothing else may reintroduce the
    /// fifteen "build it first" panics that made this suite unreadable.
    #[test]
    fn only_the_required_flag_turns_a_skip_back_into_a_failure() {
        assert!(!required_from(None), "an ad-hoc run skips");
        assert!(!required_from(Some("0")));
        assert!(!required_from(Some("")));
        assert!(required_from(Some("1")), "`./kd test rust` fails instead");
        assert!(required_from(Some("true")));
    }
}
