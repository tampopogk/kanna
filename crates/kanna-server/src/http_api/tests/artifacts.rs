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
    ArtifactEnv {
        root,
        app: router(Arc::clone(&state)),
        state,
        repo,
        workspace,
        home,
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

    // The remote is configuration, never a request parameter.
    let (status, _) = call(
        &env.app,
        "POST",
        &format!("/v1/repos/repo-a/artifacts/{id}/push"),
        Some(json!({ "remote": "/tmp/elsewhere.git" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

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
