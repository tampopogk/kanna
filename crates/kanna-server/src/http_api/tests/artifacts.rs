//! Artifact routes end to end: publish from a task workspace, read and
//! annotate by exact tree id, and fetch every relative asset of a multi-file
//! HTML mockup through the isolated preview listener.

use super::*;
use crate::db::{NewPipelineItem, NewRepo};
use crate::http_api::test_support::test_state_with_artifact_home;
use serde_json::{json, Value};

struct ArtifactEnv {
    root: PathBuf,
    state: Arc<AppState>,
    app: axum::Router,
    repo: PathBuf,
    workspace: PathBuf,
    home: PathBuf,
    /// Task ids this home's advance-stage route handed to its stage
    /// advancer, when the home was set up with `stage_gate`.
    advanced: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Drop for ArtifactEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn run_git(directory: &Path, args: &[&str]) {
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
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

const INDEX_HTML: &[u8] = b"<!doctype html><link rel=stylesheet href=css/site.css>\
<script src=js/app.js></script><img src=img/logo.png><a href=pages/about.html>about</a>";
const ABOUT_HTML: &[u8] = b"<!doctype html><link rel=stylesheet href=../css/site.css>about";
const LOGO_PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0xff];

fn write_mockup(workspace: &Path, directory: &str, css: &str) {
    let base = workspace.join(directory);
    write(&base.join("index.html"), INDEX_HTML);
    write(&base.join("css/site.css"), css.as_bytes());
    write(&base.join("js/app.js"), b"document.title = 'mockup';");
    write(&base.join("img/logo.png"), LOGO_PNG);
    write(&base.join("pages/about.html"), ABOUT_HTML);
}

fn setup(label: &str, local_config: Option<Value>) -> ArtifactEnv {
    setup_home(
        label,
        HomeOptions {
            local_config,
            ..HomeOptions::default()
        },
    )
}

/// How one test home is set up beyond the defaults `setup` uses.
struct HomeOptions {
    /// Written to the working tree's `.kanna/config.local.json`.
    local_config: Option<Value>,
    /// Committed as `.kanna/config.json` and published as `origin/main`, the
    /// snapshot repo definitions resolve from.
    committed_config: Option<Value>,
    /// Seed `task-a` and `task-closed`. A home that only receives shared
    /// artifacts has no tasks of its own.
    seed_tasks: bool,
    /// Answer advance-stage through the state's test stage advancer, which
    /// records the call and moves the task to `review` in this home's DB.
    stage_gate: bool,
}

impl Default for HomeOptions {
    fn default() -> Self {
        Self {
            local_config: None,
            committed_config: None,
            seed_tasks: true,
            stage_gate: false,
        }
    }
}

fn setup_home(label: &str, options: HomeOptions) -> ArtifactEnv {
    let HomeOptions {
        local_config,
        committed_config,
        seed_tasks,
        stage_gate,
    } = options;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/artifact-http-tests")
        .join(format!("{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    write(&repo.join("README.md"), b"fixture\n");
    if let Some(committed) = &committed_config {
        write(
            &repo.join(".kanna/config.json"),
            committed.to_string().as_bytes(),
        );
    }
    run_git(&repo, &["add", "."]);
    run_git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "base",
        ],
    );
    if committed_config.is_some() {
        run_git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    }
    if let Some(local) = local_config {
        write(
            &repo.join(".kanna/config.local.json"),
            local.to_string().as_bytes(),
        );
    }
    let workspace = root.join("workspace");
    write_mockup(&workspace, "mock", "body{color:#123}");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let repo_path = repo.to_string_lossy().to_string();
    let workspace_path = workspace.to_string_lossy().to_string();
    let state = test_state_with_artifact_home(&format!("artifacts-{label}"), &home, |db| {
        db.insert_repo(NewRepo {
            id: "repo-a",
            path: &repo_path,
            name: "repo-a",
            default_branch: Some("main"),
        })
        .unwrap();
        if !seed_tasks {
            return;
        }
        for id in ["task-a", "task-closed"] {
            db.insert_pipeline_item(NewPipelineItem {
                id,
                repo_id: "repo-a",
                prompt: "artifact task",
                display_name: None,
                pipeline: "default",
                pipeline_def: None,
                stage: "in progress",
                branch: &format!("task-{id}"),
                agent_type: "pty",
                agent_provider: "claude",
                activity: "idle",
                port_offset: None,
                port_env_json: None,
                agent_spawn_options_json: None,
                base_ref: None,
                notify_task_id: None,
                parent_task_id: None,
            })
            .unwrap();
            db.upsert_worktree(
                &format!("wt-{id}"),
                id,
                &workspace_path,
                &format!("task-{id}"),
            )
            .unwrap();
        }
        db.close_pipeline_item("task-closed").unwrap();
    });
    let advanced = Arc::new(std::sync::Mutex::new(Vec::new()));
    let state = if stage_gate {
        // The real transition forks a worktree and spawns the next stage's
        // agent through the daemon; this seam replaces only that last step,
        // after the route's own access, task resolution and transfer checks.
        let mut state = Arc::try_unwrap(state).unwrap_or_else(|_| unreachable!("fresh test state"));
        let db_path = state.config().db_path.clone();
        let calls = Arc::clone(&advanced);
        state.stage_advancer = Some(Arc::new(move |task_id: String| {
            calls.lock().unwrap().push(task_id.clone());
            Db::open(&db_path)
                .and_then(|db| db.update_pipeline_item_stage(&task_id, "review"))
                .map_err(|error| error.to_string())?;
            Ok(crate::mobile_api::TaskActionResponse {
                task_id,
                follow_task: None,
                revision_budget: None,
                workflow_extended: None,
            })
        }));
        Arc::new(state)
    } else {
        state
    };
    ArtifactEnv {
        root,
        app: router(Arc::clone(&state)),
        state,
        repo,
        workspace,
        home,
        advanced,
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes))),
    )
}

async fn publish(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    call(app, "POST", "/v1/tasks/task-a/artifacts", Some(body)).await
}

#[tokio::test]
async fn a_multi_file_mockup_publishes_opens_by_tree_id_and_serves_every_relative_asset() {
    let env = setup("e2e", None);
    let (status, v1) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    assert_eq!(status, StatusCode::CREATED, "{v1}");
    let v1_id = v1["artifactId"].as_str().unwrap().to_string();
    assert_eq!(v1["reference"]["type"], "stored");
    assert_eq!(v1["version"]["retention"], "keep");
    assert_eq!(v1["version"]["producedBy"]["taskId"], "task-a");
    assert!(env
        .home
        .join(".kanna/repos/repo-a/artifacts.git/HEAD")
        .exists());
    assert!(!env.workspace.join(".git").exists());
    assert!(!env.repo.join("artifacts.git").exists());

    // Identical bytes, same identity.
    let (_, again) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    assert_eq!(again["artifactId"], v1_id);
    assert_eq!(again["contentCreated"], false);

    let (status, detail) = call(
        &env.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{v1_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["files"].as_array().unwrap().len(), 5);
    assert_eq!(detail["versions"].as_array().unwrap().len(), 2);

    let (status, opened) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v1_id}/preview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let url = opened["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    assert!(url.ends_with("/index.html"), "{url}");
    assert!(!url.contains(env.root.to_str().unwrap()));
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let index = client.get(&url).send().await.unwrap();
    assert_eq!(index.status(), 200);
    let csp = index.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    for directive in [
        "sandbox allow-scripts",
        "connect-src 'none'",
        "form-action 'none'",
        "worker-src 'none'",
    ] {
        assert!(csp.contains(directive), "{csp}");
    }
    assert!(!csp.contains("allow-same-origin"));
    // The desktop hosts the page in a sandboxed iframe; only its webview
    // origins may frame it.
    assert!(
        csp.contains("frame-ancestors tauri://localhost http://tauri.localhost"),
        "{csp}"
    );
    assert!(!csp.contains("frame-ancestors 'none'"), "{csp}");
    assert!(!csp.contains("frame-ancestors *"), "{csp}");
    assert_eq!(index.headers()["referrer-policy"], "no-referrer");
    assert_eq!(index.headers()["x-content-type-options"], "nosniff");
    // CORP same-origin would block the opaque-origin page's own assets.
    assert!(index
        .headers()
        .get("cross-origin-resource-policy")
        .is_none());
    assert_eq!(index.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(index.bytes().await.unwrap(), INDEX_HTML);

    let page = reqwest::Url::parse(&url).unwrap();
    let about = page.join("pages/about.html").unwrap();
    // Relative references resolve the way a browser resolves them.
    let nested_css = about.join("../css/site.css").unwrap();
    for (asset, media_type, bytes) in [
        (
            page.join("css/site.css").unwrap(),
            "text/css; charset=utf-8",
            b"body{color:#123}".to_vec(),
        ),
        (
            page.join("js/app.js").unwrap(),
            "text/javascript; charset=utf-8",
            b"document.title = 'mockup';".to_vec(),
        ),
        (
            page.join("img/logo.png").unwrap(),
            "image/png",
            LOGO_PNG.to_vec(),
        ),
        (
            about.clone(),
            "text/html; charset=utf-8",
            ABOUT_HTML.to_vec(),
        ),
        (
            nested_css,
            "text/css; charset=utf-8",
            b"body{color:#123}".to_vec(),
        ),
    ] {
        let response = client.get(asset.clone()).send().await.unwrap();
        assert_eq!(response.status(), 200, "{asset}");
        assert_eq!(response.headers()["content-type"], media_type, "{asset}");
        assert_eq!(response.bytes().await.unwrap(), bytes, "{asset}");
    }
    let head = client.head(&url).send().await.unwrap();
    assert_eq!(head.status(), 200);

    // A directory and the bare capability both land on an HTML page.
    let base = page.join("./").unwrap();
    let root = client
        .get(base.as_str().trim_end_matches('/'))
        .send()
        .await
        .unwrap();
    assert_eq!(root.status(), 302);
    assert!(root.headers()["location"]
        .to_str()
        .unwrap()
        .ends_with("/index.html"));

    // A new version with a previous link; annotations on the older id.
    write_mockup(&env.workspace, "mock", "body{color:#456}");
    let (status, v2) = publish(
        &env.app,
        json!({ "path": "mock", "kind": "mockup", "previous": v1_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{v2}");
    let v2_id = v2["artifactId"].as_str().unwrap().to_string();
    assert_ne!(v2_id, v1_id);
    assert_eq!(v2["version"]["previous"], v1_id);
    let (status, comment) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v1_id}/comments"),
        Some(json!({ "author": "designer", "body": "too dark", "anchor": { "path": "css/site.css", "position": "line 1", "excerpt": "color:#123" } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment}");
    assert_eq!(comment["aboutArtifactId"], v1_id);
    let (status, decision) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v1_id}/decisions"),
        Some(json!({ "who": "owner", "what": "superseded by v2" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{decision}");
    let (_, old) = call(
        &env.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{v1_id}"),
        None,
    )
    .await;
    assert_eq!(old["comments"][0]["anchor"]["path"], "css/site.css");
    assert_eq!(old["decisions"][0]["what"], "superseded by v2");
    let (_, new) = call(
        &env.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{v2_id}"),
        None,
    )
    .await;
    assert_eq!(new["comments"], json!([]));
    assert_eq!(new["versions"][0]["previous"], v1_id);

    // The v1 preview still serves v1's bytes and cannot address v2.
    let old_css = client
        .get(page.join("css/site.css").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(
        old_css.bytes().await.unwrap(),
        b"body{color:#123}".as_slice()
    );
    let cross = client
        .get(page.join(&format!("{v2_id}/index.html")).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(cross.status(), 404);

    // Missing asset and refused requests.
    let missing = client
        .get(page.join("css/missing.css").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert!(missing.text().await.unwrap().contains("css/missing.css"));
    let traversal = client
        .get(format!("{}..%2Fsecret", base.as_str()))
        .send()
        .await
        .unwrap();
    assert_eq!(traversal.status(), 400);
    // URL parsers fold `%2e%2e` into `..` before sending; the server must
    // refuse it when a raw client does not.
    let port = page.port().unwrap();
    for raw_path in [
        format!("{}%2e%2e/css/site.css", base.path()),
        format!("{}css/../index.html", base.path()),
        format!("{}css/%2E/site.css", base.path()),
    ] {
        assert_eq!(raw_status(port, &raw_path).await, 400, "{raw_path}");
    }
    let wrong_capability = client
        .get(url.replace(
            &url[url.find("/a/").unwrap() + 3..url.find("/a/").unwrap() + 35],
            &"0".repeat(32),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_capability.status(), 404);
    let rebound = client
        .get(&url)
        .header("host", format!("evil.example:{}", page.port().unwrap()))
        .send()
        .await
        .unwrap();
    assert_eq!(rebound.status(), 404);
    let post = client.post(&url).send().await.unwrap();
    assert_eq!(post.status(), 405);
    let control = client
        .get(format!(
            "http://127.0.0.1:{}/v1/status",
            page.port().unwrap()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        control.status(),
        404,
        "the preview listener exposes no control API"
    );

    // Reopening returns the live session; closing ends the listener.
    let (_, reopened) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v1_id}/preview"),
        None,
    )
    .await;
    assert_eq!(reopened["url"], url);
    let (status, closed) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v1_id}/preview/close"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["closed"], true);
    await_listener_gone(&client, &url).await;

    // Idle expiry makes a session stop serving.
    let (_, expiring) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v2_id}/preview"),
        None,
    )
    .await;
    let expiring_url = expiring["url"].as_str().unwrap().to_string();
    assert_eq!(
        client.get(&expiring_url).send().await.unwrap().status(),
        200
    );
    env.state
        .artifact_previews
        .expire_for_tests("repo-a", &v2_id)
        .await;
    assert_eq!(
        client.get(&expiring_url).send().await.unwrap().status(),
        404
    );
    env.state.artifact_previews.close("repo-a", &v2_id).await;
}

async fn raw_status(port: u16, path: &str) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("no status in {response:?}"))
}

async fn await_listener_gone(client: &reqwest::Client, url: &str) {
    for _ in 0..100 {
        match client.get(url).send().await {
            Err(_) => return,
            Ok(response) if response.status() == 404 => return,
            Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    panic!("closed artifact preview kept serving");
}

#[tokio::test]
async fn missing_and_malformed_identities_are_reported_explicitly() {
    let env = setup("errors", None);
    let unknown = "0123456789abcdef0123456789abcdef01234567";

    // Before anything is published there is no repository and nothing is created.
    let (status, body) = call(
        &env.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{unknown}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "artifact_not_found");
    assert_eq!(body["artifactId"], unknown);
    assert_eq!(body["repoId"], "repo-a");
    assert!(!env.home.join(".kanna").exists());

    let (_, published) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    let id = published["artifactId"].as_str().unwrap().to_string();
    for (method, path, body, status, code) in [
        (
            "GET",
            format!("/v1/repos/repo-a/artifacts/{unknown}"),
            None,
            StatusCode::NOT_FOUND,
            "artifact_not_found",
        ),
        (
            "GET",
            "/v1/repos/repo-a/artifacts/HEAD".to_string(),
            None,
            StatusCode::BAD_REQUEST,
            "invalid_artifact_id",
        ),
        (
            "GET",
            format!("/v1/repos/repo-a/artifacts/{}", &id[..12]),
            None,
            StatusCode::BAD_REQUEST,
            "invalid_artifact_id",
        ),
        (
            "GET",
            format!("/v1/repos/repo-missing/artifacts/{id}"),
            None,
            StatusCode::NOT_FOUND,
            "repo_not_found",
        ),
        (
            "POST",
            format!("/v1/repos/repo-a/artifacts/{unknown}/preview"),
            None,
            StatusCode::NOT_FOUND,
            "artifact_not_found",
        ),
        (
            "POST",
            format!("/v1/repos/repo-a/artifacts/{unknown}/comments"),
            Some(json!({"author": "a", "body": "b"})),
            StatusCode::NOT_FOUND,
            "artifact_not_found",
        ),
        (
            "POST",
            format!("/v1/repos/repo-a/artifacts/{id}/comments"),
            Some(json!({"author": "", "body": "b"})),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            "POST",
            format!("/v1/repos/repo-a/artifacts/{id}/comments"),
            Some(json!({"author": "a", "body": "b", "anchor": {"path": "nope.css"}})),
            StatusCode::NOT_FOUND,
            "artifact_file_not_found",
        ),
        (
            "POST",
            "/v1/tasks/task-a/artifacts".to_string(),
            Some(json!({"path": "mock", "kind": "mockup", "previous": unknown})),
            StatusCode::BAD_REQUEST,
            "unknown_previous_artifact",
        ),
        (
            "POST",
            "/v1/tasks/task-a/artifacts".to_string(),
            Some(json!({"path": "mock", "kind": "diagram"})),
            StatusCode::BAD_REQUEST,
            "invalid_kind",
        ),
        (
            "POST",
            "/v1/tasks/task-a/artifacts".to_string(),
            Some(json!({"path": "mock", "kind": "mockup", "entrypoint": "nope.html"})),
            StatusCode::BAD_REQUEST,
            "invalid_entrypoint",
        ),
        (
            "POST",
            "/v1/tasks/task-a/artifacts".to_string(),
            Some(json!({"path": "../repo", "kind": "document"})),
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            "POST",
            "/v1/tasks/task-a/artifacts".to_string(),
            Some(json!({"path": "absent", "kind": "document"})),
            StatusCode::NOT_FOUND,
            "source_not_found",
        ),
        (
            "POST",
            "/v1/tasks/task-closed/artifacts".to_string(),
            Some(json!({"path": "mock", "kind": "mockup"})),
            StatusCode::CONFLICT,
            "task_closed",
        ),
    ] {
        let (actual, response) = call(&env.app, method, &path, body).await;
        assert_eq!(actual, status, "{method} {path}: {response}");
        assert_eq!(response["error"], code, "{method} {path}: {response}");
    }
    let (status, _) = call(
        &env.app,
        "POST",
        "/v1/tasks/task-unknown/artifacts",
        Some(json!({"path": "mock", "kind": "mockup"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A symlink out of the workspace is refused with nothing published.
    std::os::unix::fs::symlink(&env.repo, env.workspace.join("mock/escape")).unwrap();
    let (status, body) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("mock/escape"),
        "{body}"
    );
}

#[tokio::test]
async fn a_client_without_the_preview_listener_reads_each_file_by_exact_tree_id() {
    use base64::Engine as _;
    let env = setup("files", None);
    let (_, v1) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    let v1_id = v1["artifactId"].as_str().unwrap().to_string();
    write_mockup(&env.workspace, "mock", "body{color:#456}");
    let (_, v2) = publish(
        &env.app,
        json!({ "path": "mock", "kind": "mockup", "previous": v1_id }),
    )
    .await;
    let v2_id = v2["artifactId"].as_str().unwrap().to_string();

    for (id, path, media_type, bytes) in [
        (
            &v1_id,
            "index.html",
            "text/html; charset=utf-8",
            INDEX_HTML.to_vec(),
        ),
        (
            &v1_id,
            "css/site.css",
            "text/css; charset=utf-8",
            b"body{color:#123}".to_vec(),
        ),
        (
            &v2_id,
            "css/site.css",
            "text/css; charset=utf-8",
            b"body{color:#456}".to_vec(),
        ),
        (&v1_id, "img/logo.png", "image/png", LOGO_PNG.to_vec()),
        (
            &v1_id,
            "pages/about.html",
            "text/html; charset=utf-8",
            ABOUT_HTML.to_vec(),
        ),
    ] {
        let (status, file) = call(
            &env.app,
            "GET",
            &format!(
                "/v1/repos/repo-a/artifacts/{id}/files?path={}",
                path.replace('/', "%2F")
            ),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{id} {path}: {file}");
        assert_eq!(file["artifactId"], id.as_str());
        assert_eq!(file["repoId"], "repo-a");
        assert_eq!(file["path"], path);
        assert_eq!(file["mediaType"], media_type);
        assert_eq!(file["size"], bytes.len());
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(file["dataBase64"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, bytes, "{id} {path}");
    }

    let unknown = "0123456789abcdef0123456789abcdef01234567";
    for (path, status, code) in [
        (
            format!("/v1/repos/repo-a/artifacts/{v1_id}/files?path=nope.css"),
            StatusCode::NOT_FOUND,
            "artifact_file_not_found",
        ),
        (
            format!("/v1/repos/repo-a/artifacts/{v1_id}/files?path=..%2Frepo"),
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            format!("/v1/repos/repo-a/artifacts/{unknown}/files?path=index.html"),
            StatusCode::NOT_FOUND,
            "artifact_not_found",
        ),
    ] {
        let (actual, body) = call(&env.app, "GET", &path, None).await;
        assert_eq!(actual, status, "{path}: {body}");
        assert_eq!(body["error"], code, "{path}: {body}");
    }

    // One file above the relay-sized bound is refused, not truncated.
    let large = vec![b'x'; crate::task_files::MAX_TASK_FILE_BYTES as usize + 1];
    write(&env.workspace.join("big/huge.txt"), &large);
    let (_, big) = publish(&env.app, json!({ "path": "big", "kind": "document" })).await;
    let big_id = big["artifactId"].as_str().unwrap();
    let (status, body) = call(
        &env.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{big_id}/files?path=huge.txt"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["error"], "file_too_large");
    assert!(body.get("dataBase64").is_none());
}

#[tokio::test]
async fn artifact_routes_refuse_an_unpaired_lan_peer() {
    let env = setup("lan", None);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/tasks/task-a/artifacts")
        .header("content-type", "application/json")
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [192, 168, 1, 50],
            50000,
        ))))
        .body(Body::from(
            json!({ "path": "mock", "kind": "mockup" }).to_string(),
        ))
        .unwrap();
    let response = env.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!env.home.join(".kanna").exists());
}

#[tokio::test]
async fn a_configured_location_inside_the_working_repository_is_refused() {
    let env = setup(
        "location",
        Some(json!({ "artifacts": { "repositoryPath": ".kanna/artifacts.git" } })),
    );
    let (status, body) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "artifact_repository_location_invalid");
    assert!(!env.repo.join(".kanna/artifacts.git").exists());

    let outside = env.root.join("elsewhere/artifacts.git");
    let env2 = setup(
        "location-ok",
        Some(
            json!({ "artifacts": { "repositoryPath": outside.to_str().unwrap(), "retention": "discard-on-close" } }),
        ),
    );
    let (status, body) = publish(&env2.app, json!({ "path": "mock", "kind": "mockup" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["version"]["retention"], "discard-on-close");
    assert!(outside.join("HEAD").exists());
    assert!(!env2.home.join(".kanna").exists());
}

/// Two servers with separate homes and databases share one artifact through
/// a bare remote named only in each repository's local config.
#[tokio::test]
async fn two_homes_share_an_artifact_and_its_discussion_through_the_configured_remote() {
    let shared = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/artifact-http-tests")
        .join(format!("share-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&shared);
    std::fs::create_dir_all(&shared).unwrap();
    let shared = std::fs::canonicalize(shared).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(shared.clone());
    run_git(&shared, &["init", "--bare", "--quiet", "remote.git"]);
    let remote = shared.join("remote.git");
    let config = json!({ "artifacts": { "remote": remote.to_str().unwrap() } });
    let a = setup("share-a", Some(config.clone()));
    let b = setup("share-b", Some(config));

    let (status, v1) = publish(&a.app, json!({ "path": "mock", "kind": "mockup" })).await;
    assert_eq!(status, StatusCode::CREATED, "{v1}");
    let v1 = v1["artifactId"].as_str().unwrap().to_string();
    write_mockup(&a.workspace, "mock", "body{color:#456}");
    let (_, v2) = publish(
        &a.app,
        json!({ "path": "mock", "kind": "mockup", "previous": v1 }),
    )
    .await;
    let v2 = v2["artifactId"].as_str().unwrap().to_string();

    let (status, pushed) = call(
        &a.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v2}/push"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pushed}");
    assert_eq!(pushed["artifactIds"], json!([v2, v1]));
    assert_eq!(pushed["remote"], remote.to_str().unwrap());

    let (status, fetched) = call(
        &b.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v2}/fetch"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(fetched["detail"]["artifactId"], v2);
    assert_eq!(fetched["detail"]["versions"][0]["previous"], v1);

    // B decides about the older revision and pushes; A fetches it back.
    let (status, _) = call(
        &b.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v1}/decisions"),
        Some(json!({ "who": "stakeholder", "what": "approved" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(
        &b.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v2}/push"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, fetched) = call(
        &a.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{v2}/fetch"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(fetched["recordsImported"], 1);
    let (_, older) = call(
        &a.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{v1}"),
        None,
    )
    .await;
    assert_eq!(older["decisions"][0]["what"], "approved");
    assert_eq!(older["decisions"][0]["aboutArtifactId"], v1);
    // A received "approved" is data: the task it might concern did not move.
    let db = Db::open(&a.state.config().db_path).unwrap();
    let item = db.get_pipeline_item("task-a").unwrap().unwrap();
    assert_eq!(item.stage.as_deref(), Some("in progress"));
    assert!(item.closed_at.is_none());

    let unknown = "0123456789abcdef0123456789abcdef01234567";
    let (status, body) = call(
        &b.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{unknown}/fetch"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "artifact_not_on_remote");
    assert_eq!(body["artifactId"], unknown);
}

#[tokio::test]
async fn sharing_needs_a_usable_configured_remote() {
    let env = setup("no-remote", None);
    let (_, published) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    let id = published["artifactId"].as_str().unwrap().to_string();
    for action in ["push", "fetch"] {
        let (status, body) = call(
            &env.app,
            "POST",
            &format!("/v1/repos/repo-a/artifacts/{id}/{action}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"], "artifact_remote_not_configured");
    }

    let helper = setup(
        "helper-remote",
        Some(json!({ "artifacts": { "remote": "ext::sh -c 'touch pwned'" } })),
    );
    let (_, published) = publish(&helper.app, json!({ "path": "mock", "kind": "mockup" })).await;
    let id = published["artifactId"].as_str().unwrap().to_string();
    let (status, body) = call(
        &helper.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/push"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "artifact_remote_invalid");

    // The remote is configuration, never a request parameter: a body naming
    // one is refused outright (only a fingerprint binding is accepted).
    let (status, _) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/push"),
        Some(json!({ "remote": "/tmp/elsewhere.git" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    for action in ["push", "fetch"] {
        let request = Request::builder()
            .method("POST")
            .uri(format!("/v1/repos/repo-a/artifacts/{id}/{action}"))
            .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [192, 168, 1, 50],
                50000,
            ))))
            .body(Body::empty())
            .unwrap();
        let response = env.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{action}");
    }
}

#[tokio::test]
async fn the_artifact_remote_route_reports_what_push_and_fetch_will_use() {
    let unconfigured = setup("remote-status-none", None);
    let (status, body) = call(
        &unconfigured.app,
        "GET",
        "/v1/repos/repo-a/artifact-remote",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "repoId": "repo-a", "configured": false }));

    let (status, body) = call(
        &unconfigured.app,
        "GET",
        "/v1/repos/missing/artifact-remote",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "repo_not_found");

    let credentialed = setup(
        "remote-status-secret",
        Some(json!({ "artifacts": { "remote": "https://user:pa/ss@host.example/team/a.git" } })),
    );
    let (status, body) = call(
        &credentialed.app,
        "GET",
        "/v1/repos/repo-a/artifact-remote",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fingerprint = body["fingerprint"].as_str().unwrap().to_string();
    assert_eq!(fingerprint.len(), 64, "{body}");
    assert_eq!(
        body,
        json!({
            "repoId": "repo-a",
            "configured": true,
            "remote": "https://***@host.example/team/a.git",
            "source": "machine-local",
            "configFile": ".kanna/config.local.json",
            "fingerprint": fingerprint,
        })
    );
    assert!(!body.to_string().contains("pa/ss"));

    let helper = setup(
        "remote-status-invalid",
        Some(json!({ "artifacts": { "remote": "ext::sh -c 'touch pwned'" } })),
    );
    let (status, body) = call(&helper.app, "GET", "/v1/repos/repo-a/artifact-remote", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["configured"], true);
    assert_eq!(body["source"], "machine-local");
    assert_eq!(body["configFile"], ".kanna/config.local.json");
    assert_eq!(body["error"]["code"], "artifact_remote_invalid");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("remote helpers"),
        "{body}"
    );
    assert!(body.get("remote").is_none(), "{body}");
    assert!(!helper.repo.join("pwned").exists());

    // A committed remote with a machine-local location: the remote is still
    // the committed one.
    let mixed = setup_home(
        "remote-status-mixed",
        HomeOptions {
            committed_config: Some(
                json!({ "artifacts": { "remote": "ssh://git@team.example/a.git" } }),
            ),
            local_config: Some(json!({ "artifacts": { "retention": "30-days" } })),
            ..HomeOptions::default()
        },
    );
    let (status, body) = call(&mixed.app, "GET", "/v1/repos/repo-a/artifact-remote", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["remote"], "ssh://***@team.example/a.git");
    assert_eq!(body["source"], "committed");
    assert_eq!(body["configFile"], ".kanna/config.json");

    // An unresolvable configuration is the same refusal every artifact
    // route gives.
    let broken = setup(
        "remote-status-unresolved",
        Some(json!({ "artifacts": { "origin": "ssh://host/a.git" } })),
    );
    let (status, body) = call(&broken.app, "GET", "/v1/repos/repo-a/artifact-remote", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "artifact_config_unresolved");
}

/// Spec §14 across two accounts: two homes that were never paired (separate
/// servers and databases, no peer or machine trust between them) review one
/// artifact through a single configured remote. The reviewing home can read,
/// comment and decide; only the owning home's stage gate moves its task.
#[tokio::test]
async fn section_14_two_accounts_share_review_through_one_remote_without_pairing() {
    let shared = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/artifact-http-tests")
        .join(format!("s14-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&shared);
    std::fs::create_dir_all(&shared).unwrap();
    let shared = std::fs::canonicalize(shared).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(shared.clone());
    run_git(&shared, &["init", "--bare", "--quiet", "remote.git"]);
    let remote = shared.join("remote.git");
    let remote_path = remote.to_str().unwrap().to_string();
    let config = json!({ "artifacts": { "remote": remote_path } });
    let a = setup_home(
        "s14-a",
        HomeOptions {
            local_config: Some(config.clone()),
            stage_gate: true,
            ..HomeOptions::default()
        },
    );
    let b = setup_home(
        "s14-b",
        HomeOptions {
            local_config: Some(config.clone()),
            seed_tasks: false,
            ..HomeOptions::default()
        },
    );
    let committed = setup_home(
        "s14-committed",
        HomeOptions {
            committed_config: Some(config),
            seed_tasks: false,
            ..HomeOptions::default()
        },
    );

    // Both homes name the same remote, configured on each machine, before
    // anything is pushed; a committed configuration says so.
    for env in [&a, &b] {
        let (status, body) = call(&env.app, "GET", "/v1/repos/repo-a/artifact-remote", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({
                "repoId": "repo-a",
                "configured": true,
                "remote": remote_path,
                "source": "machine-local",
                "configFile": ".kanna/config.local.json",
                "fingerprint": body["fingerprint"],
            })
        );
        assert!(body["fingerprint"].is_string(), "{body}");
    }
    let (status, body) = call(
        &committed.app,
        "GET",
        "/v1/repos/repo-a/artifact-remote",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["remote"], remote_path.as_str());
    assert_eq!(body["source"], "committed");
    assert_eq!(body["configFile"], ".kanna/config.json");
    assert_eq!(
        run_git_output(&remote, &["for-each-ref", "--format=%(refname)"]),
        "",
        "nothing was pushed yet"
    );

    // A publishes a multi-file mockup from task-a's workspace and shares it.
    let (status, published) = publish(&a.app, json!({ "path": "mock", "kind": "mockup" })).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    let hash = published["artifactId"].as_str().unwrap().to_string();
    let (status, pushed) = call(
        &a.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{hash}/push"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pushed}");
    assert!(
        !pushed["createdRefs"].as_array().unwrap().is_empty(),
        "{pushed}"
    );
    let (status, again) = call(
        &a.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{hash}/push"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["createdRefs"], json!([]), "{again}");
    assert!(again["upToDateRefs"].as_u64().unwrap() > 0, "{again}");

    // B has never published; the hash is all it is given.
    assert!(!b.home.join(".kanna/repos/repo-a/artifacts.git").exists());
    let (status, fetched) = call(
        &b.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{hash}/fetch"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(fetched["detail"]["artifactId"], hash);
    let (status, detail) = call(
        &b.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{hash}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let files: Vec<&str> = detail["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert!(
        files.contains(&"index.html") && files.contains(&"css/site.css"),
        "{detail}"
    );
    let (status, page) = call(
        &b.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{hash}/files?path=index.html"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["mediaType"], "text/html; charset=utf-8");
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        page["dataBase64"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(bytes, INDEX_HTML);

    // B reviews that exact version and shares its review.
    let anchor =
        json!({ "path": "css/site.css", "position": "line 1", "excerpt": "body{color:#123}" });
    let (status, comment) = call(
        &b.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{hash}/comments"),
        Some(json!({ "author": "client", "body": "the accent is too dark", "anchor": anchor })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment}");
    let (status, decision) = call(
        &b.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{hash}/decisions"),
        Some(json!({ "who": "client", "what": "approved" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{decision}");
    let (status, pushed) = call(
        &b.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{hash}/push"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pushed}");

    // A receives both records, anchored to the version they were made on.
    let (status, fetched) = call(
        &a.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{hash}/fetch"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(fetched["recordsImported"], 2, "{fetched}");
    let (status, detail) = call(
        &a.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{hash}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let comments = detail["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1, "{detail}");
    assert_eq!(comments[0]["author"], "client");
    assert_eq!(comments[0]["aboutArtifactId"], hash);
    assert_eq!(comments[0]["anchor"], anchor);
    let decisions = detail["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 1, "{detail}");
    assert_eq!(decisions[0]["who"], "client");
    assert_eq!(decisions[0]["what"], "approved");
    assert_eq!(decisions[0]["aboutArtifactId"], hash);

    // The received "approved" is data: task-a did not move, and nothing
    // reached A's stage gate.
    let a_db = Db::open(&a.state.config().db_path).unwrap();
    let item = a_db.get_pipeline_item("task-a").unwrap().unwrap();
    assert_eq!(item.stage.as_deref(), Some("in progress"));
    assert!(item.closed_at.is_none());
    assert!(a.advanced.lock().unwrap().is_empty());

    // B cannot address A's task: it is not in B's database, and B's gate
    // for it answers not found.
    let b_db = Db::open(&b.state.config().db_path).unwrap();
    assert!(b_db.get_pipeline_item("task-a").unwrap().is_none());
    let (status, body) = call(
        &b.app,
        "POST",
        "/v1/tasks/task-a/actions/advance-stage",
        Some(json!({ "source": "operator" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(a.advanced.lock().unwrap().is_empty());

    // A's owner operates A's gate, and that is what moves task-a.
    let (status, body) = call(
        &a.app,
        "POST",
        "/v1/tasks/task-a/actions/advance-stage",
        Some(json!({ "source": "operator" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(*a.advanced.lock().unwrap(), ["task-a"]);
    let item = a_db.get_pipeline_item("task-a").unwrap().unwrap();
    assert_eq!(item.stage.as_deref(), Some("review"));
}

fn run_git_output(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

// ---------------------------------------------------------------------------
// T6b: result references, retention, preview lifecycle and isolation
// ---------------------------------------------------------------------------

const SHA_A: &str = "1111111111111111111111111111111111111111";
const SHA_B: &str = "2222222222222222222222222222222222222222";

fn insert_running_run(env: &ArtifactEnv, run_id: &str) {
    let db = Db::open(&env.state.config().db_path).unwrap();
    db.insert_stage_run(crate::db::NewStageRun {
        id: run_id,
        task_id: "task-a",
        stage: "in progress",
        kind: "main",
        agent: Some("implement"),
        agent_provider: Some("claude"),
        model: None,
        effort: None,
        status: "running",
        result: None,
        feedback: None,
        session_id: Some("task-a"),
        provider_session_id: None,
        cwd: Some(&env.workspace.to_string_lossy()),
        resumed_from_run_id: None,
    })
    .unwrap();
}

fn result_entries(env: &ArtifactEnv) -> Vec<crate::task_store::LedgerFile> {
    let db_path = env.state.config().db_path.clone();
    let db = Db::open(&db_path).unwrap();
    let dir = crate::task_store::task_dir_for(&db, &db_path, "task-a").unwrap();
    crate::task_store::read_ledger(&dir)
        .unwrap()
        .into_iter()
        .filter(|file| file.kind == crate::db::task_store::LedgerEntryKind::Result)
        .collect()
}

async fn complete(env: &ArtifactEnv, body: Value) -> (StatusCode, Value) {
    call(
        &env.app,
        "POST",
        "/v1/tasks/task-a/actions/complete-stage",
        Some(body),
    )
    .await
}

#[tokio::test]
async fn a_result_records_its_named_artifact_references_in_the_ledger_entry() {
    let env = setup("result-refs", None);
    insert_running_run(&env, "run-refs");
    let (status, published) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    let id = published["artifactId"].as_str().unwrap().to_string();

    let (status, body) = complete(
        &env,
        json!({
            "runId": "run-refs",
            "status": "unverified",
            "summary": "mockup ready\n\nNot checked on a phone.",
            "artifacts": {
                "mockup": id,
                "base": { "type": "commit", "repoId": "repo-a", "sha": SHA_A },
                "pull request": { "type": "pr", "url": "https://github.com/o/r/pull/7", "headSha": SHA_B },
            },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The published file on disk carries the references.
    let results = result_entries(&env);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].envelope["artifacts"],
        json!({
            "mockup": { "type": "stored", "repoId": "repo-a", "artifactId": id, "kind": "mockup" },
            "base": { "type": "commit", "repoId": "repo-a", "sha": SHA_A },
            "pull request": { "type": "pr", "url": "https://github.com/o/r/pull/7", "headSha": SHA_B },
        })
    );
    // The stored content knows the result named it.
    let (_, detail) = call(
        &env.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{id}"),
        None,
    )
    .await;
    assert_eq!(detail["bindings"].as_array().unwrap().len(), 1, "{detail}");
    assert_eq!(detail["bindings"][0]["taskId"], "task-a");
    assert_eq!(detail["bindings"][0]["name"], "mockup");
    assert_eq!(detail["bindings"][0]["runId"], "run-refs");
    assert_eq!(detail["expired"], false);
}

#[tokio::test]
async fn an_unresolvable_reference_refuses_the_result_and_records_nothing() {
    let env = setup("result-refused", None);
    insert_running_run(&env, "run-refused");
    let (_, published) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    let id = published["artifactId"].as_str().unwrap().to_string();
    let never = "0123456789abcdef0123456789abcdef01234567";

    for (label, artifacts) in [
        ("never published", json!({ "mockup": id, "ghost": never })),
        ("malformed id", json!({ "mockup": "abc123" })),
        (
            "another repository",
            json!({ "x": { "type": "stored", "repoId": "repo-b", "artifactId": id, "kind": "mockup" } }),
        ),
        (
            "wrong kind",
            json!({ "x": { "type": "stored", "repoId": "repo-a", "artifactId": id, "kind": "report" } }),
        ),
        (
            "short commit",
            json!({ "x": { "type": "commit", "repoId": "repo-a", "sha": "1111" } }),
        ),
        (
            "pr without url",
            json!({ "x": { "type": "pr", "url": "javascript:alert(1)", "headSha": SHA_B } }),
        ),
        ("not a map", json!(["mockup"])),
        ("empty name", json!({ "": id })),
    ] {
        let (status, body) = complete(
            &env,
            json!({
                "runId": "run-refused",
                "status": "unverified",
                "summary": "done",
                "artifacts": artifacts,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {body}");
        assert!(
            body.as_str()
                .unwrap_or_default()
                .contains("nothing was recorded"),
            "{label}: {body}"
        );
    }

    let db = Db::open(&env.state.config().db_path).unwrap();
    let run = db.stage_run("run-refused").unwrap().unwrap();
    assert_eq!(run.status, "running");
    assert_eq!(run.result, None);
    assert!(result_entries(&env).is_empty());
    let (_, detail) = call(
        &env.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{id}"),
        None,
    )
    .await;
    assert_eq!(
        detail["bindings"],
        json!([]),
        "a refused result bound content"
    );

    // The established spelling without references still works and records
    // an empty map.
    let (status, body) = complete(
        &env,
        json!({ "runId": "run-refused", "status": "unverified", "summary": "done" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let results = result_entries(&env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].envelope["artifacts"], json!({}));
    let run = db.stage_run("run-refused").unwrap().unwrap();
    let stored: Value = serde_json::from_str(run.result.as_deref().unwrap()).unwrap();
    assert!(stored.get("artifacts").is_none(), "{stored}");
}

#[tokio::test]
async fn the_retention_sweep_reads_task_lifecycle_from_the_database() {
    let env = setup(
        "retention-sweep",
        Some(json!({ "artifacts": { "retention": "discard-on-close" } })),
    );
    let (_, published) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    let id = published["artifactId"].as_str().unwrap().to_string();
    assert_eq!(published["version"]["retention"], "discard-on-close");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3 * 60 * 60);

    // Open producer: kept.
    let swept = crate::http_api::artifacts::sweep_artifact_retention(&env.state, later);
    assert_eq!(swept.len(), 1);
    assert_eq!(swept[0].0, "repo-a");
    assert!(swept[0].1.as_ref().unwrap().expired.is_empty());

    Db::open(&env.state.config().db_path)
        .unwrap()
        .close_pipeline_item("task-a")
        .unwrap();
    // Just closed: still inside the grace an undone close relies on.
    let swept = crate::http_api::artifacts::sweep_artifact_retention(
        &env.state,
        std::time::SystemTime::now(),
    );
    assert!(swept[0].1.as_ref().unwrap().expired.is_empty());
    let swept = crate::http_api::artifacts::sweep_artifact_retention(&env.state, later);
    assert_eq!(swept[0].1.as_ref().unwrap().expired, vec![id.clone()]);

    let (status, detail) = call(
        &env.app,
        "GET",
        &format!("/v1/repos/repo-a/artifacts/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["retained"], false);
    assert_eq!(detail["expired"], true);
    assert_eq!(detail["versions"].as_array().unwrap().len(), 1);
    assert_eq!(
        detail["expirations"][0]["policies"],
        json!(["discard-on-close"])
    );
    let (status, body) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/preview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "artifact_content_missing");
}

fn repository_path(env: &ArtifactEnv) -> PathBuf {
    env.home.join(".kanna/repos/repo-a/artifacts.git")
}

#[tokio::test]
async fn previews_beyond_the_cap_are_refused_until_one_closes() {
    use crate::http_api::artifact_preview::{ArtifactPreviewSessions, PreviewOpenError};
    let env = setup("preview-cap", None);
    write(&env.workspace.join("one.md"), b"one");
    write(&env.workspace.join("two.md"), b"two");
    let mut ids = Vec::new();
    for path in ["mock", "one.md", "two.md"] {
        let (_, published) = publish(&env.app, json!({ "path": path, "kind": "document" })).await;
        ids.push((
            published["artifactId"].as_str().unwrap().to_string(),
            published["version"]["entrypoint"]
                .as_str()
                .unwrap()
                .to_string(),
        ));
    }
    let sessions = ArtifactPreviewSessions::with_limits(2, std::time::Duration::from_secs(1));
    let open = |index: usize| {
        let (id, entrypoint) = ids[index].clone();
        let sessions = sessions.clone();
        let path = repository_path(&env);
        async move { sessions.open("repo-a".into(), id, path, entrypoint).await }
    };
    open(0).await.unwrap();
    open(1).await.unwrap();
    // Reopening an open one is not a new session.
    open(1).await.unwrap();
    assert!(matches!(open(2).await, Err(PreviewOpenError::Limit(2))));
    assert!(sessions.close("repo-a", &ids[0].0).await);
    open(2).await.unwrap();
    sessions.close("repo-a", &ids[1].0).await;
    sessions.close("repo-a", &ids[2].0).await;
}

/// Send a request for a large file and never read the response.
async fn stall_a_large_download(url: &str) -> (tokio::net::TcpStream, usize) {
    use tokio::io::AsyncWriteExt;
    let url = reqwest::Url::parse(url).unwrap();
    let port = url.port().unwrap();
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream
        .write_all(
            format!(
                "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n",
                url.path()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    // Long enough for the response to fill every buffer and park.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    (stream, BIG_FILE_BYTES)
}

const BIG_FILE_BYTES: usize = 15 * 1024 * 1024;

async fn assert_cut_short(mut stream: tokio::net::TcpStream, expected: usize) {
    use tokio::io::AsyncReadExt;
    let mut received = Vec::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_to_end(&mut received),
    )
    .await
    .expect("the aborted connection never ended");
    // Reset or EOF are both an end; either way it came up short.
    let _ = read;
    assert!(
        received.len() < expected,
        "the stalled response was delivered in full ({} bytes), so the test did not stall it",
        received.len()
    );
}

async fn await_no_live_listeners(
    sessions: &crate::http_api::artifact_preview::ArtifactPreviewSessions,
    within: std::time::Duration,
) {
    let deadline = tokio::time::Instant::now() + within;
    while sessions.live_listeners() > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "a preview listener outlived its drain deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_stalled_reader_cannot_outlive_a_closed_or_expired_preview() {
    use crate::http_api::artifact_preview::ArtifactPreviewSessions;
    let env = setup("preview-drain", None);
    let big = (0..BIG_FILE_BYTES)
        .map(|index| (index * 7919 % 251) as u8)
        .collect::<Vec<_>>();
    write(&env.workspace.join("big.bin"), &big);
    let (status, published) =
        publish(&env.app, json!({ "path": "big.bin", "kind": "media" })).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    let id = published["artifactId"].as_str().unwrap().to_string();
    let drain = std::time::Duration::from_millis(300);
    let sessions = ArtifactPreviewSessions::with_limits(4, drain);

    // Explicit close.
    let opened = sessions
        .open(
            "repo-a".into(),
            id.clone(),
            repository_path(&env),
            "big.bin".into(),
        )
        .await
        .unwrap();
    let url = serde_json::to_value(&opened).unwrap()["url"]
        .as_str()
        .unwrap()
        .to_string();
    let (stream, expected) = stall_a_large_download(&url).await;
    assert_eq!(sessions.live_listeners(), 1);
    assert!(sessions.close("repo-a", &id).await);
    // Still draining right after the close...
    assert_eq!(sessions.live_listeners(), 1);
    // ...and gone within the deadline plus the abort.
    await_no_live_listeners(&sessions, drain * 2 + std::time::Duration::from_secs(2)).await;
    assert_cut_short(stream, expected).await;

    // Idle expiry takes the same path (the expiry poll is five seconds).
    let opened = sessions
        .open(
            "repo-a".into(),
            id.clone(),
            repository_path(&env),
            "big.bin".into(),
        )
        .await
        .unwrap();
    let url = serde_json::to_value(&opened).unwrap()["url"]
        .as_str()
        .unwrap()
        .to_string();
    let (stream, expected) = stall_a_large_download(&url).await;
    sessions.expire_for_tests("repo-a", &id).await;
    await_no_live_listeners(&sessions, drain * 2 + std::time::Duration::from_secs(8)).await;
    assert_cut_short(stream, expected).await;
}

#[tokio::test]
async fn a_top_level_navigation_receives_the_sandboxing_shell_not_the_content() {
    let env = setup("preview-shell", None);
    let (_, published) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    let id = published["artifactId"].as_str().unwrap().to_string();
    let (_, opened) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/preview"),
        None,
    )
    .await;
    let url = opened["url"].as_str().unwrap().to_string();
    let port = reqwest::Url::parse(&url).unwrap().port().unwrap();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let shell = client
        .get(&url)
        .header("Sec-Fetch-Dest", "document")
        .send()
        .await
        .unwrap();
    assert_eq!(shell.status(), 200);
    let policy = shell.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        policy.contains(&format!(
            "frame-src http://127.0.0.1:{port} http://localhost:{port}"
        )),
        "{policy}"
    );
    assert!(policy.contains("script-src 'none'"), "{policy}");
    let path = reqwest::Url::parse(&url).unwrap().path().to_string();
    let html = shell.text().await.unwrap();
    assert!(
        html.contains(&format!(
            "<iframe sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" src=\"{path}\">"
        )),
        "{html}"
    );
    assert!(
        !html.contains("site.css"),
        "the shell leaked content: {html}"
    );

    // A framed request, or one from a client that sends no fetch metadata,
    // gets the content, which only the preview's own origins may frame.
    for dest in [Some("iframe"), None] {
        let mut request = client.get(&url);
        if let Some(dest) = dest {
            request = request.header("Sec-Fetch-Dest", dest);
        }
        let content = request.send().await.unwrap();
        assert_eq!(content.status(), 200);
        let policy = content.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .to_string();
        assert!(policy.starts_with("sandbox allow-scripts;"), "{policy}");
        // The union of the desktop webview origins (T12) and the preview's
        // own shell origins (T6b).
        assert!(
            policy.contains("frame-ancestors tauri://localhost http://tauri.localhost"),
            "{policy}"
        );
        assert!(
            policy.ends_with(&format!(" http://127.0.0.1:{port} http://localhost:{port}")),
            "{policy}"
        );
        assert_eq!(content.bytes().await.unwrap().as_ref(), INDEX_HTML);
    }
    env.state.artifact_previews.close("repo-a", &id).await;
}

// ---------------------------------------------------------------------------
// Browser-level isolation
// ---------------------------------------------------------------------------

/// A Chromium-family browser for the browser-level test: `KANNA_TEST_CHROME`,
/// else Playwright's headless shell, else an installed Chrome or Chromium.
fn find_browser() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("KANNA_TEST_CHROME") {
        return Some(PathBuf::from(path));
    }
    let mut candidates = Vec::new();
    for cache in [
        std::env::var_os("PLAYWRIGHT_BROWSERS_PATH").map(PathBuf::from),
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Caches/ms-playwright")),
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/ms-playwright")),
    ]
    .into_iter()
    .flatten()
    {
        let Ok(entries) = std::fs::read_dir(cache) else {
            continue;
        };
        let mut shells = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("chromium_headless_shell-"))
            })
            .collect::<Vec<_>>();
        shells.sort();
        for shell in shells.into_iter().rev() {
            for platform in [
                "chrome-headless-shell-mac-arm64",
                "chrome-headless-shell-mac-x64",
                "chrome-headless-shell-linux64",
                "chrome-linux",
            ] {
                candidates.push(shell.join(platform).join("chrome-headless-shell"));
                candidates.push(shell.join(platform).join("headless_shell"));
            }
        }
    }
    candidates.extend([
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/chromium-browser"),
    ]);
    candidates.into_iter().find(|path| path.is_file())
}

/// A stand-in for the control port: records the request line of every
/// connection it receives and answers 200.
async fn control_port_canary() -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let line = String::from_utf8_lossy(&buffer[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                recorded.lock().unwrap().push(line);
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
            });
        }
    });
    (port, hits)
}

/// Serve `html` top-level with the preview's pre-T6b posture (a sandboxed
/// document of its own): the positive control proving the probe navigates.
async fn unshielded_page(html: String) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let html = html.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 4096];
                let _ = stream.read(&mut buffer).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Content-Security-Policy: sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline'; connect-src 'none'; frame-ancestors 'none'\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    port
}

/// Load `url` top-level in a headless browser, give its timers five virtual
/// seconds, and return the browser's log. A browser that dumps the page but
/// does not exit (desktop Chrome's updater can keep it up) is killed after
/// a bounded wait; the page had its time either way.
async fn load_in_browser(browser: &Path, profile: &Path, url: &str) -> String {
    let log = profile.with_extension("log");
    std::fs::create_dir_all(profile).unwrap();
    let mut child = tokio::process::Command::new(browser)
        .args([
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--disable-extensions",
            "--disable-background-networking",
            "--use-mock-keychain",
            "--password-store=basic",
            "--enable-logging=stderr",
            "--v=0",
            "--virtual-time-budget=5000",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--dump-dom")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .kill_on_drop(true)
        .spawn()
        .expect("the browser did not start");
    if tokio::time::timeout(std::time::Duration::from_secs(20), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
    std::fs::read_to_string(&log).unwrap_or_default()
}

/// Hostile preview content tries every way a document navigates the top
/// level or itself to the control port. Through the preview none reaches
/// it; the same page served as its own top-level document does, which is
/// what proves the probe would notice.
#[tokio::test]
async fn preview_content_cannot_navigate_to_the_control_port_in_a_real_browser() {
    let Some(browser) = find_browser() else {
        assert!(
            std::env::var_os("KANNA_REQUIRE_BROWSER_TESTS").is_none(),
            "KANNA_REQUIRE_BROWSER_TESTS is set but no browser was found"
        );
        eprintln!("SKIPPED: no Chrome/Chromium found (set KANNA_TEST_CHROME)");
        return;
    };
    let env = setup("preview-browser", None);
    let (control_port, hits) = control_port_canary().await;
    let target = format!("http://127.0.0.1:{control_port}/v1/tasks");
    let hostile = format!(
        "<!doctype html><meta http-equiv=refresh content=\"2;url={target}?via=refresh\">\
         <p>hostile</p><script>\
         const leak = encodeURIComponent(location.href);\
         try {{ top.location.href = '{target}?via=top&leak=' + leak; }} catch (e) {{ console.log('top refused: ' + e); }}\
         setTimeout(() => {{ try {{ window.open('{target}?via=open'); }} catch (e) {{}} }}, 100);\
         setTimeout(() => {{ location.href = '{target}?via=self&leak=' + leak; }}, 300);\
         </script>"
    );
    write(
        &env.workspace.join("hostile/index.html"),
        hostile.as_bytes(),
    );
    let (status, published) =
        publish(&env.app, json!({ "path": "hostile", "kind": "mockup" })).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    let id = published["artifactId"].as_str().unwrap().to_string();
    let (_, opened) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/preview"),
        None,
    )
    .await;
    let url = opened["url"].as_str().unwrap().to_string();
    let profile = env.root.join("browser-profile");

    // Positive control: served as its own top-level document, the page does
    // reach the stand-in control port.
    let control = unshielded_page(hostile.clone()).await;
    let control_log = load_in_browser(
        &browser,
        &profile.join("control"),
        &format!("http://127.0.0.1:{control}/"),
    )
    .await;
    let reached = hits.lock().unwrap().clone();
    eprintln!(
        "browser {}: unshielded page reached the control port with {reached:?}",
        browser.display()
    );
    assert!(
        !reached.is_empty(),
        "the unshielded page never navigated, so this probe proves nothing; browser log:\n{control_log}"
    );
    hits.lock().unwrap().clear();

    let log = load_in_browser(&browser, &profile.join("preview"), &url).await;
    let reached = hits.lock().unwrap().clone();
    assert!(
        reached.is_empty(),
        "preview content navigated to the control port: {reached:?}\nbrowser log:\n{log}"
    );
    eprintln!(
        "through the preview: {:?}",
        log.lines()
            .filter(|line| line.contains("CONSOLE")
                || line.contains("Refused")
                || line.contains("sandbox"))
            .collect::<Vec<_>>()
    );
    // The framed content did run and was refused, rather than never
    // loading: the sandbox refused the top navigation, and the shell's
    // frame-src refused the frame navigating itself.
    assert!(
        log.contains("top refused"),
        "no sign the hostile content ran inside the preview; browser log:\n{log}"
    );
    assert!(
        log.contains("frame-src"),
        "the frame's self-navigation was not refused by the shell's frame-src; browser log:\n{log}"
    );
    env.state.artifact_previews.close("repo-a", &id).await;
}

/// The configuration moves the remote between the client's read of it and its
/// push. A push bound to the remote the reader approved is refused before any
/// remote is contacted; an unbound push keeps today's behaviour.
#[tokio::test]
async fn a_push_bound_to_the_approved_remote_is_refused_when_the_config_moved_it() {
    let env = setup("push-bound-remote", None);
    let approved_remote = env.root.join("approved.git");
    let moved_remote = env.root.join("moved.git");
    run_git(&env.root, &["init", "--bare", "--quiet", "approved.git"]);
    run_git(&env.root, &["init", "--bare", "--quiet", "moved.git"]);
    let local = env.repo.join(".kanna/config.local.json");
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    let configure = |remote: &Path| {
        std::fs::write(
            &local,
            json!({ "artifacts": { "remote": remote.to_str().unwrap() } }).to_string(),
        )
        .unwrap();
        let db = Db::open(&env.state.config().db_path).unwrap();
        let repo = db.get_repo("repo-a").unwrap().unwrap();
        // Stand in for the definitions cache expiring.
        env.state.repo_definitions.invalidate(&repo);
    };
    configure(&approved_remote);
    let (status, published) = publish(&env.app, json!({ "path": "mock", "kind": "mockup" })).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    let id = published["artifactId"].as_str().unwrap().to_string();

    let (_, shown) = call(&env.app, "GET", "/v1/repos/repo-a/artifact-remote", None).await;
    assert_eq!(shown["remote"], approved_remote.to_str().unwrap());
    let approval = json!({ "remoteFingerprint": shown["fingerprint"] });

    // A pull or a local edit moves the remote before the push arrives.
    configure(&moved_remote);
    let (status, body) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/push"),
        Some(approval.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "artifact_remote_changed");
    assert_eq!(body["reason"], "artifact_remote_changed");
    assert_eq!(body["remote"], moved_remote.to_str().unwrap());
    assert_eq!(body["source"], "machine-local");
    assert_ne!(body["fingerprint"], shown["fingerprint"]);
    for remote in [&approved_remote, &moved_remote] {
        assert_eq!(
            run_git_output(remote, &["for-each-ref"]),
            "",
            "{} was contacted",
            remote.display()
        );
    }

    // A fingerprint the server never issued is refused the same way.
    configure(&approved_remote);
    let (status, body) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/push"),
        Some(json!({ "remoteFingerprint": "0".repeat(64) })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "artifact_remote_changed");

    // A malformed body is refused outright, not treated as no binding.
    let (status, body) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/push"),
        Some(json!({ "remote": shown["remote"] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(run_git_output(&approved_remote, &["for-each-ref"]), "");

    // The approved remote, still in force, is pushed to.
    let (status, body) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/push"),
        Some(approval),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!run_git_output(&approved_remote, &["for-each-ref"]).is_empty());

    // `{}` (what kanna_push_artifact sends) binds nothing either.
    let (status, body) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/push"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // No body: pushed to whatever the configuration names now, as before.
    configure(&moved_remote);
    let (status, body) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/push"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["remote"], moved_remote.to_str().unwrap());
}
