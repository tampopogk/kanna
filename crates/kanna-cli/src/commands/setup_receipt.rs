//! Capture what a task's startup shell leaves behind, for the agent that runs
//! after it.
//!
//! Setup used to run inside the agent's own login shell, so anything it
//! exported — a PATH entry for a workspace-local toolchain, a token, a
//! version-manager shim — simply applied to the agent. Now setup runs in its
//! own terminal, and that inheritance has to be carried across deliberately.
//! This is the bundled helper the setup shell invokes as its last successful
//! step: it writes its own environment and working directory, which are the
//! shell's, to a private file the server reads before spawning the agent.
//!
//! It is a *readiness* receipt, not a completion hook: it says setup finished
//! and what it left behind, and nothing about the task's outcome. The file
//! holds the workspace's exported environment, so it is written `0600` into a
//! server-owned directory and is never echoed to the terminal or returned on
//! any public surface.

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupReceipt {
    version: u32,
    cwd: String,
    env: BTreeMap<String, String>,
}

pub(crate) fn run(output: &str) {
    let cwd = match std::env::current_dir() {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(error) => {
            eprintln!("kanna-cli: cannot read the setup working directory: {error}");
            std::process::exit(1);
        }
    };
    let receipt = SetupReceipt {
        version: 1,
        cwd,
        env: std::env::vars().collect(),
    };
    let rendered = match serde_json::to_vec(&receipt) {
        Ok(rendered) => rendered,
        Err(error) => {
            eprintln!("kanna-cli: cannot render the setup receipt: {error}");
            std::process::exit(1);
        }
    };
    if let Some(parent) = std::path::Path::new(output).parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("kanna-cli: cannot create the setup receipt directory: {error}");
            std::process::exit(1);
        }
    }
    if let Err(error) = write_private(output, &rendered) {
        eprintln!("kanna-cli: cannot write the setup receipt: {error}");
        std::process::exit(1);
    }
}

#[cfg(unix)]
fn write_private(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}
