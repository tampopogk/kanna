//! The artifact tools, over MCP, against a real `kanna-server`.
//!
//! `crates/kanna-tool-catalog` pins what the tools declare and the CLI tests
//! pin its `tool call` wire requests. This pins what they do: JSON-RPC in,
//! `kanna-mcp`'s own request resolution, the real routes, a real bare
//! artifact repository on disk, and the bytes a browser gets back from the
//! preview listener. All fixture files live under this worktree's `.tmp/`.
//!
//! See `common/mod.rs` for the harness.

mod common;

use common::{execute_sql, start_bare_chain};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn git(directory: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn tool_error(response: &Value) -> String {
    assert_eq!(response["result"]["isError"], json!(true), "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_mockup_publishes_opens_and_is_annotated_by_exact_id_over_mcp() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/mcp-artifact-tools")
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let fixture = Fixture {
        root: std::fs::canonicalize(&root).unwrap(),
    };
    let repo = fixture.root.join("repo");
    let workspace = fixture.root.join("workspace");
    let store = fixture.root.join("store/artifacts.git");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    git(
        &repo,
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
    // The machine-local layer names where this machine keeps artifacts.
    write(
        &repo.join(".kanna/config.local.json"),
        json!({ "artifacts": { "repositoryPath": store } })
            .to_string()
            .as_bytes(),
    );
    write(
        &workspace.join("mock/index.html"),
        b"<link rel=stylesheet href=css/site.css><img src=img/logo.png>",
    );
    write(&workspace.join("mock/css/site.css"), b"body{color:#123}");
    write(
        &workspace.join("mock/img/logo.png"),
        &[0x89, b'P', b'N', b'G', 0, 255],
    );

    let (server, _daemon, mut mcp) = start_bare_chain("artifact-tools").await;
    execute_sql(
        &server,
        "INSERT INTO repo (id, path, name, default_branch, hidden, sort_order, created_at, last_opened_at)
         VALUES ('repo-art', ?, 'Artifacts', 'main', 0, 0, datetime('now'), datetime('now'))",
        json!([repo.to_string_lossy()]),
    )
    .await;
    execute_sql(
        &server,
        "INSERT INTO pipeline_item (
            id, repo_id, prompt, stage, branch, agent_type, activity, pinned, pin_order,
            display_name, created_at, updated_at, pipeline, initial_pipeline, agent_provider
         ) VALUES ('task-art', 'repo-art', 'mockup', 'in progress', 'task-art', 'pty', 'idle',
                   0, NULL, 'Mockup', datetime('now'), datetime('now'), 'default', 'default', 'claude')",
        json!([]),
    )
    .await;
    execute_sql(
        &server,
        "INSERT INTO worktree (id, pipeline_item_id, path, branch) VALUES ('wt-art', 'task-art', ?, 'task-art')",
        json!([workspace.to_string_lossy()]),
    )
    .await;

    mcp.call_tool(
        2,
        "kanna_publish_artifact",
        json!({ "task_id": "task-art", "path": "mock", "kind": "mockup" }),
    );
    let v1 = mcp.recv_task();
    let v1_id = v1["artifactId"].as_str().unwrap().to_string();
    assert_eq!(v1_id.len(), 40);
    assert!(store.join("HEAD").exists(), "the configured store was used");

    write(&workspace.join("mock/css/site.css"), b"body{color:#456}");
    mcp.call_tool(
        3,
        "kanna_publish_artifact",
        json!({ "task_id": "task-art", "path": "mock", "kind": "mockup", "previous": v1_id }),
    );
    let v2 = mcp.recv_task();
    let v2_id = v2["artifactId"].as_str().unwrap().to_string();
    assert_ne!(v2_id, v1_id);
    assert_eq!(v2["version"]["previous"], v1_id);

    mcp.call_tool(
        4,
        "kanna_record_artifact_comment",
        json!({ "repo_id": "repo-art", "artifact_id": v1_id, "author": "designer", "body": "too dark",
                "anchor": { "path": "css/site.css", "excerpt": "#123" } }),
    );
    assert_eq!(mcp.recv_task()["aboutArtifactId"], v1_id);
    mcp.call_tool(
        5,
        "kanna_record_artifact_decision",
        json!({ "repo_id": "repo-art", "artifact_id": v1_id, "who": "owner", "what": "superseded" }),
    );
    assert_eq!(mcp.recv_task()["aboutArtifactId"], v1_id);

    mcp.call_tool(
        6,
        "kanna_get_artifact",
        json!({ "repo_id": "repo-art", "artifact_id": v1_id }),
    );
    let old = mcp.recv_task();
    assert_eq!(old["comments"][0]["anchor"]["path"], "css/site.css");
    assert_eq!(old["decisions"][0]["who"], "owner");
    mcp.call_tool(
        7,
        "kanna_get_artifact",
        json!({ "repo_id": "repo-art", "artifact_id": v2_id }),
    );
    assert_eq!(mcp.recv_task()["comments"], json!([]));

    // Open the older version and fetch its page and relative assets.
    mcp.call_tool(
        8,
        "kanna_open_artifact",
        json!({ "repo_id": "repo-art", "artifact_id": v1_id }),
    );
    let opened = mcp.recv_task();
    let url = reqwest::Url::parse(opened["url"].as_str().unwrap()).unwrap();
    let client = reqwest::Client::new();
    let page = client.get(url.clone()).send().await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("sandbox allow-scripts"));
    let css = client
        .get(url.join("css/site.css").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(css.bytes().await.unwrap(), b"body{color:#123}".as_slice());
    let logo = client
        .get(url.join("img/logo.png").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(logo.headers()["content-type"], "image/png");
    assert_eq!(
        logo.bytes().await.unwrap(),
        [0x89, b'P', b'N', b'G', 0, 255].as_slice()
    );

    mcp.call_tool(
        9,
        "kanna_close_artifact",
        json!({ "repo_id": "repo-art", "artifact_id": v1_id }),
    );
    assert_eq!(mcp.recv_task()["closed"], true);

    // A missing object is an explicit tool error naming it.
    let unknown = "0123456789abcdef0123456789abcdef01234567";
    mcp.call_tool(
        10,
        "kanna_get_artifact",
        json!({ "repo_id": "repo-art", "artifact_id": unknown }),
    );
    let error = tool_error(&mcp.recv());
    assert!(error.contains("artifact_not_found"), "{error}");
    assert!(error.contains(unknown), "{error}");

    // Nothing reached the working repository.
    assert!(!repo.join("artifacts.git").exists());
    let status = Command::new("git")
        .args(["status", "--porcelain", "--ignored"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&status.stdout).trim(),
        "?? .kanna/",
        "only the fixture's own local config is untracked"
    );
}
