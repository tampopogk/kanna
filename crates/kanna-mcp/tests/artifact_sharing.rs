//! Two independent Kanna homes share one artifact through a local bare Git
//! remote: home A over MCP, home B over `kanna-cli tool call`, each a real
//! `kanna-server` with its own `HOME`, database and artifact store. Nothing
//! pairs the two servers; the artifact id is all that passes between them.
//! All fixture files live under this worktree's `.tmp/`.
//!
//! See `common/mod.rs` for the harness.

mod common;

use common::{execute_sql, start_bare_chain_with_env, RunningServer};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const SENTINEL: &str = "KANNA-SHARING-SENTINEL-7f3e";

struct Fixture {
    root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Like the server, the CLI is a sibling binary the workspace lane builds.
fn kanna_cli_binary() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_kanna-mcp"))
        .parent()
        .expect("kanna-mcp binary directory")
        .join("kanna-cli");
    assert!(
        path.exists(),
        "kanna-cli binary is missing at {}; run `cargo test --workspace` or \
         `cargo build -p kanna-cli` first",
        path.display()
    );
    path
}

/// `kanna-cli tool call <tool> --arg k=v ...` against one server.
fn cli(
    server: &RunningServer,
    cwd: &Path,
    tool: &str,
    args: &[(&str, &str)],
) -> Result<Value, String> {
    let mut command = Command::new(kanna_cli_binary());
    command
        .args(["tool", "call", tool, "--server-url", &server.base_url])
        .current_dir(cwd)
        .env_remove("KANNA_TASK_ID");
    for (key, value) in args {
        command.args(["--arg", &format!("{key}={value}")]);
    }
    let output = command.output().unwrap();
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout).unwrap())
    } else {
        Err(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn init_repo(path: &Path, remote: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "--quiet", "--initial-branch", "main"]);
    git(
        path,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "base",
        ],
    );
    // Only the machine-local layer names the remote; the store location is
    // each home's default under its own HOME.
    write(
        &path.join(".kanna/config.local.json"),
        json!({ "artifacts": { "remote": remote } })
            .to_string()
            .as_bytes(),
    );
}

async fn seed(server: &RunningServer, repo_id: &str, repo: &Path, task_id: &str, workspace: &Path) {
    execute_sql(
        server,
        "INSERT INTO repo (id, path, name, default_branch, hidden, sort_order, created_at, last_opened_at)
         VALUES (?, ?, 'Artifacts', 'main', 0, 0, datetime('now'), datetime('now'))",
        json!([repo_id, repo.to_string_lossy()]),
    )
    .await;
    execute_sql(
        server,
        "INSERT INTO pipeline_item (
            id, repo_id, prompt, stage, branch, agent_type, activity, pinned, pin_order,
            display_name, created_at, updated_at, pipeline, initial_pipeline, agent_provider
         ) VALUES (?, ?, 'mockup', 'in progress', ?, 'pty', 'idle',
                   0, NULL, 'Mockup', datetime('now'), datetime('now'), 'default', 'default', 'claude')",
        json!([task_id, repo_id, task_id]),
    )
    .await;
    execute_sql(
        server,
        "INSERT INTO worktree (id, pipeline_item_id, path, branch) VALUES (?, ?, ?, ?)",
        json!([
            format!("wt-{task_id}"),
            task_id,
            workspace.to_string_lossy(),
            task_id
        ]),
    )
    .await;
}

#[tokio::test]
async fn two_homes_share_an_artifact_and_its_discussion_over_mcp_and_cli() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/mcp-artifact-sharing")
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let fixture = Fixture {
        root: std::fs::canonicalize(&root).unwrap(),
    };
    let remote = fixture.root.join("remote.git");
    git(&fixture.root, &["init", "--bare", "--quiet", "remote.git"]);

    let (home_a, home_b) = (fixture.root.join("home-a"), fixture.root.join("home-b"));
    let (repo_a, repo_b) = (fixture.root.join("repo-a"), fixture.root.join("repo-b"));
    let (workspace_a, workspace_b) = (fixture.root.join("ws-a"), fixture.root.join("ws-b"));
    init_repo(&repo_a, &remote);
    init_repo(&repo_b, &remote);
    std::fs::create_dir_all(&workspace_b).unwrap();

    const INDEX: &[u8] = b"<link rel=stylesheet href=css/site.css><img src=img/logo.png>";
    const LOGO: &[u8] = &[0x89, b'P', b'N', b'G', 0, 255];
    write(&workspace_a.join("mock/index.html"), INDEX);
    write(&workspace_a.join("mock/css/site.css"), b"body{color:#123}");
    write(&workspace_a.join("mock/img/logo.png"), LOGO);
    // Things around the published directory that must never be shared.
    write(
        &workspace_a.join(".env"),
        format!("TOKEN={SENTINEL}").as_bytes(),
    );
    write(
        &home_a.join(".kanna/tasks/task-a/transcript.jsonl"),
        format!("{{\"text\":\"{SENTINEL}\"}}").as_bytes(),
    );
    write(
        &home_a.join(".config/gh/hosts.yml"),
        format!("oauth_token: {SENTINEL}").as_bytes(),
    );

    let sentinel_env = Path::new(SENTINEL);
    let (server_a, _daemon_a, mut mcp_a) = start_bare_chain_with_env(
        "artifact-share-a",
        &[
            ("HOME", &home_a),
            ("KANNA_SHARING_TEST_SECRET", sentinel_env),
        ],
    )
    .await;
    let (server_b, _daemon_b, mut mcp_b) =
        start_bare_chain_with_env("artifact-share-b", &[("HOME", &home_b)]).await;
    seed(&server_a, "repo-a", &repo_a, "task-a", &workspace_a).await;
    seed(&server_b, "repo-bee", &repo_b, "task-b", &workspace_b).await;

    // A publishes two versions and pushes the newer one over MCP.
    mcp_a.call_tool(
        2,
        "kanna_publish_artifact",
        json!({ "task_id": "task-a", "path": "mock", "kind": "mockup" }),
    );
    let v1 = mcp_a.recv_task()["artifactId"]
        .as_str()
        .unwrap()
        .to_string();
    write(&workspace_a.join("mock/css/site.css"), b"body{color:#456}");
    mcp_a.call_tool(
        3,
        "kanna_publish_artifact",
        json!({ "task_id": "task-a", "path": "mock", "kind": "mockup", "previous": v1 }),
    );
    let v2 = mcp_a.recv_task()["artifactId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(home_a
        .join(".kanna/repos/repo-a/artifacts.git/HEAD")
        .exists());
    mcp_a.call_tool(
        4,
        "kanna_push_artifact",
        json!({ "repo_id": "repo-a", "artifact_id": v2 }),
    );
    let pushed = mcp_a.recv_task();
    assert_eq!(pushed["artifactIds"], json!([v2, v1]));

    // B is told only the id, and fetches it through the CLI.
    let cwd = &fixture.root;
    let fetched = cli(
        &server_b,
        cwd,
        "kanna_fetch_artifact",
        &[("repo_id", "repo-bee"), ("artifact_id", &v2)],
    )
    .unwrap();
    assert_eq!(fetched["detail"]["artifactId"], v2, "same tree id at B");
    assert_eq!(fetched["detail"]["versions"][0]["previous"], v1);
    assert_eq!(fetched["fetched"], json!([v2, v1]));
    assert!(home_b
        .join(".kanna/repos/repo-bee/artifacts.git/HEAD")
        .exists());

    // B opens identical bytes, relative assets included, at the same id.
    mcp_b.call_tool(
        2,
        "kanna_open_artifact",
        json!({ "repo_id": "repo-bee", "artifact_id": v2 }),
    );
    let url = reqwest::Url::parse(mcp_b.recv_task()["url"].as_str().unwrap()).unwrap();
    let client = reqwest::Client::new();
    for (path, expected) in [
        ("index.html", INDEX),
        ("css/site.css", b"body{color:#456}".as_slice()),
        ("img/logo.png", LOGO),
    ] {
        let response = client.get(url.join(path).unwrap()).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(response.bytes().await.unwrap(), expected, "{path}");
    }

    // Both homes annotate concurrently; B's decision is about the older id.
    mcp_a.call_tool(
        5,
        "kanna_record_artifact_comment",
        json!({ "repo_id": "repo-a", "artifact_id": v2, "author": "alice", "body": "from A" }),
    );
    let a_comment = mcp_a.recv_task()["recordId"].as_str().unwrap().to_string();
    let b_comment = cli(
        &server_b,
        cwd,
        "kanna_record_artifact_comment",
        &[
            ("repo_id", "repo-bee"),
            ("artifact_id", &v2),
            ("author", "bob"),
            ("body", "from B"),
        ],
    )
    .unwrap()["recordId"]
        .as_str()
        .unwrap()
        .to_string();
    let b_decision = cli(
        &server_b,
        cwd,
        "kanna_record_artifact_decision",
        &[
            ("repo_id", "repo-bee"),
            ("artifact_id", &v1),
            ("who", "bob"),
            ("what", "approved"),
        ],
    )
    .unwrap()["recordId"]
        .as_str()
        .unwrap()
        .to_string();

    // B pushes first, then A; then each fetches.
    cli(
        &server_b,
        cwd,
        "kanna_push_artifact",
        &[("repo_id", "repo-bee"), ("artifact_id", &v2)],
    )
    .unwrap();
    mcp_a.call_tool(
        6,
        "kanna_push_artifact",
        json!({ "repo_id": "repo-a", "artifact_id": v2 }),
    );
    mcp_a.recv_task();
    mcp_a.call_tool(
        7,
        "kanna_fetch_artifact",
        json!({ "repo_id": "repo-a", "artifact_id": v2 }),
    );
    let at_a = mcp_a.recv_task();
    let at_b = cli(
        &server_b,
        cwd,
        "kanna_fetch_artifact",
        &[("repo_id", "repo-bee"), ("artifact_id", &v2)],
    )
    .unwrap();
    for fetched in [&at_a, &at_b] {
        let comments = fetched["detail"]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|comment| comment["recordId"].as_str().unwrap().to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            comments,
            BTreeSet::from([a_comment.clone(), b_comment.clone()])
        );
        assert_eq!(fetched["detail"]["decisions"], json!([]));
    }
    mcp_a.call_tool(
        8,
        "kanna_get_artifact",
        json!({ "repo_id": "repo-a", "artifact_id": v1 }),
    );
    let older = mcp_a.recv_task();
    assert_eq!(older["decisions"][0]["recordId"], b_decision.as_str());
    assert_eq!(older["decisions"][0]["aboutArtifactId"], v1.as_str());

    // A received "approved" moved nothing at A.
    mcp_a.call_get_task(9, "task-a");
    let task = mcp_a.recv_task();
    assert_eq!(task["stage"], "in progress", "{task}");

    // An id the remote does not hold is reported missing, over both paths.
    let unknown = "0123456789abcdef0123456789abcdef01234567";
    let error = cli(
        &server_b,
        cwd,
        "kanna_fetch_artifact",
        &[("repo_id", "repo-bee"), ("artifact_id", unknown)],
    )
    .unwrap_err();
    assert!(error.contains("artifact_not_on_remote"), "{error}");
    assert!(error.contains("missing"), "{error}");
    mcp_a.call_tool(
        10,
        "kanna_fetch_artifact",
        json!({ "repo_id": "repo-a", "artifact_id": unknown }),
    );
    let response = mcp_a.recv();
    assert_eq!(response["result"]["isError"], json!(true), "{response}");
    assert!(response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("artifact_not_on_remote"));

    // The remote's whole inventory: artifact refs only, nothing unreachable,
    // every blob either published payload or a record, no secret anywhere.
    let refs = git(&remote, &["for-each-ref", "--format=%(refname)"]);
    for name in refs.lines() {
        assert!(
            name.starts_with("refs/kanna/artifacts/shared/content/")
                || name.starts_with("refs/kanna/artifacts/shared/records/"),
            "unexpected ref {name}"
        );
    }
    // 2 content, 2 versions, 2 comments, 1 decision.
    assert_eq!(refs.lines().count(), 7, "{refs}");
    let all = git(
        &remote,
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname) %(objecttype)",
        ],
    );
    let reachable = git(&remote, &["rev-list", "--objects", "--all"])
        .lines()
        .map(|line| line.split(' ').next().unwrap().to_string())
        .collect::<BTreeSet<_>>();
    let payload: [&[u8]; 4] = [INDEX, b"body{color:#123}", b"body{color:#456}", LOGO];
    for line in all.lines() {
        let (id, kind) = line.split_once(' ').unwrap();
        assert!(
            reachable.contains(id),
            "unreachable object {id} on the remote"
        );
        if kind != "blob" {
            continue;
        }
        let bytes = Command::new("git")
            .args(["cat-file", "blob", id])
            .current_dir(&remote)
            .output()
            .unwrap()
            .stdout;
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SENTINEL),
            "a secret reached the remote in {id}"
        );
        if !payload.contains(&bytes.as_slice()) {
            let record: Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| panic!("blob {id} is neither payload nor a record"));
            assert_eq!(record["schemaVersion"], 1, "{record}");
        }
    }
}
