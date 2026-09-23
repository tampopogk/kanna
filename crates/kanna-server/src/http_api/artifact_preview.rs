//! Read-only browser preview of one artifact tree.
//!
//! An ordinary browser cannot navigate the authenticated control API, and
//! artifact HTML must never run with its authority. So opening an artifact
//! binds a separate loopback listener — its own origin — that serves exactly
//! one tree of one repository straight from Git blobs:
//!
//! - The URL carries a 128-bit capability as its first path segment
//!   (`/a/<capability>/<file>`), so relative asset references resolve under
//!   it. A path capability rather than a cookie because the page is served
//!   sandboxed with an opaque origin, which would not send a cookie on its
//!   subresource requests. `Referrer-Policy: no-referrer` keeps it out of
//!   referrers and nothing here logs request paths.
//! - Every response carries a CSP `sandbox allow-scripts` (no same-origin, no
//!   forms, no popups, no top navigation, no service worker) with
//!   `connect-src 'none'`, so mockup scripts and styles run but cannot reach
//!   the control API or the network.
//! - Only GET and HEAD, only this listener's own `Host`, no other trees, no
//!   repository paths, no control endpoints.
//! - Artifact content is never a top-level document. A sandboxed document
//!   may still navigate *itself*, and a top-level one could so reach any
//!   loopback service (the control API included) or carry its capability
//!   away in a URL. So a top-level navigation (`Sec-Fetch-Dest: document`)
//!   is answered with a fixed shell that frames the same URL in an
//!   `<iframe sandbox="allow-scripts">`: the frame cannot navigate the top
//!   (no `allow-top-navigation`) or open popups, and the shell's `frame-src`
//!   confines every navigation of the frame to this listener. Requests with
//!   no `Sec-Fetch-Dest` (non-browser clients) get the content as before.
//!
//! Sessions end on explicit close, after `IDLE_TTL` without a request, or at
//! `HARD_TTL`, and with the server process. Ending stops accepting at once,
//! lets in-flight responses drain for at most the drain deadline, then
//! aborts every connection the listener accepted, so a client that stops
//! reading cannot keep a session's task alive. At most `MAX_SESSIONS`
//! previews are open at a time.

use crate::artifacts::store::ArtifactStore;
use crate::artifacts::{random_hex, ArtifactError};
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{
    ALLOW, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HOST, LOCATION, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::future::{Future, IntoFuture as _};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch, Mutex};

const CAPABILITY_PREFIX: &str = "/a/";
const IDLE_TTL: Duration = Duration::from_secs(15 * 60);
const HARD_TTL: Duration = Duration::from_secs(60 * 60);
const EXPIRY_POLL: Duration = Duration::from_secs(5);
/// How long responses in flight may finish after a session ends.
const DRAIN_DEADLINE: Duration = Duration::from_secs(5);
/// Previews open at once. Each is a listener and a task; opening one more
/// is refused until one is closed or expires.
const MAX_SESSIONS: usize = 16;

type PreviewKey = (String, String);

#[derive(Clone)]
pub(super) struct ArtifactPreviewSessions {
    sessions: Arc<Mutex<HashMap<PreviewKey, PreviewHandle>>>,
    max_sessions: usize,
    drain_deadline: Duration,
    /// Serve tasks still running, closed or not.
    live: Arc<AtomicUsize>,
}

impl Default for ArtifactPreviewSessions {
    fn default() -> Self {
        Self {
            sessions: Arc::default(),
            max_sessions: MAX_SESSIONS,
            drain_deadline: DRAIN_DEADLINE,
            live: Arc::default(),
        }
    }
}

/// Why a preview could not be opened.
#[derive(Debug)]
pub(super) enum PreviewOpenError {
    /// `MAX_SESSIONS` previews are already open.
    Limit(usize),
    Failed(String),
}

impl From<String> for PreviewOpenError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

struct PreviewHandle {
    cancel: oneshot::Sender<()>,
    session: Arc<PreviewSession>,
}

struct PreviewSession {
    repository_path: PathBuf,
    repo_id: String,
    artifact_id: String,
    entrypoint: String,
    capability: String,
    port: u16,
    hard_expires_at: u64,
    last_activity: AtomicU64,
    content_security_policy: HeaderValue,
}

impl PreviewSession {
    fn is_current(&self) -> bool {
        let now = unix_seconds();
        now < self.hard_expires_at
            && now.saturating_sub(self.last_activity.load(Ordering::Acquire)) < IDLE_TTL.as_secs()
    }

    fn url(&self) -> String {
        format!(
            "http://127.0.0.1:{}{CAPABILITY_PREFIX}{}/{}",
            self.port,
            self.capability,
            encode_path(&self.entrypoint)
        )
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OpenedArtifactPreview {
    repo_id: String,
    artifact_id: String,
    entrypoint: String,
    /// Open this in a browser. It is a bearer capability for this one tree.
    url: String,
    /// Milliseconds since the epoch; the session may end earlier when idle.
    expires_at: u64,
    idle_timeout_secs: u64,
}

impl ArtifactPreviewSessions {
    #[cfg(test)]
    pub(super) fn with_limits(max_sessions: usize, drain_deadline: Duration) -> Self {
        Self {
            max_sessions,
            drain_deadline,
            ..Self::default()
        }
    }

    /// Serve tasks still running, including ones draining after a close.
    #[cfg(test)]
    pub(super) fn live_listeners(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }

    pub(super) async fn open(
        &self,
        repo_id: String,
        artifact_id: String,
        repository_path: PathBuf,
        entrypoint: String,
    ) -> Result<OpenedArtifactPreview, PreviewOpenError> {
        let key = (repo_id.clone(), artifact_id.clone());
        let mut sessions = self.sessions.lock().await;
        if let Some(handle) = sessions.get(&key) {
            if handle.session.is_current() && handle.session.repository_path == repository_path {
                handle
                    .session
                    .last_activity
                    .store(unix_seconds(), Ordering::Release);
                return Ok(opened(&handle.session));
            }
        }
        // An expired session still in the map is on its way out and does
        // not count; the one this open replaces does not either.
        let open = sessions
            .iter()
            .filter(|(other, handle)| **other != key && handle.session.is_current())
            .count();
        if open >= self.max_sessions {
            return Err(PreviewOpenError::Limit(self.max_sessions));
        }

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| format!("failed to bind artifact preview listener: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("failed to inspect artifact preview listener: {error}"))?
            .port();
        let capability = random_hex(16)?;
        let now = unix_seconds();
        let session = Arc::new(PreviewSession {
            repository_path,
            repo_id: repo_id.clone(),
            artifact_id: artifact_id.clone(),
            entrypoint,
            capability,
            port,
            hard_expires_at: now.saturating_add(HARD_TTL.as_secs()),
            last_activity: AtomicU64::new(now),
            content_security_policy: content_security_policy(port)?,
        });
        let (cancel, cancelled) = oneshot::channel();
        if let Some(previous) = sessions.insert(
            key.clone(),
            PreviewHandle {
                cancel,
                session: Arc::clone(&session),
            },
        ) {
            let _ = previous.cancel.send(());
        }
        drop(sessions);

        let registry = self.clone();
        let served = Arc::clone(&session);
        let drain_deadline = self.drain_deadline;
        self.live.fetch_add(1, Ordering::AcqRel);
        tokio::spawn(async move {
            let app = Router::new()
                .route("/", any(serve_artifact_request))
                .route("/{*path}", any(serve_artifact_request))
                .with_state(Arc::clone(&served));
            let (abort, aborted) = watch::channel(false);
            let (stop, stopped) = oneshot::channel::<()>();
            let server = axum::serve(
                TrackedListener {
                    inner: listener,
                    aborted,
                },
                app,
            )
            .with_graceful_shutdown(async move {
                let _ = stopped.await;
            })
            .into_future();
            tokio::pin!(server);
            let expiring = Arc::clone(&served);
            let ended = tokio::select! {
                result = &mut server => Some(result),
                _ = cancelled => None,
                _ = wait_for_expiry(expiring) => None,
            };
            let result = match ended {
                Some(result) => result,
                None => {
                    // Stop accepting and let responses in flight finish, but
                    // only until the deadline: then every accepted connection
                    // is aborted, which ends the server whatever its clients
                    // are (or are not) doing.
                    let _ = stop.send(());
                    match tokio::time::timeout(drain_deadline, &mut server).await {
                        Ok(result) => result,
                        Err(_) => {
                            let _ = abort.send(true);
                            match tokio::time::timeout(drain_deadline, &mut server).await {
                                Ok(result) => result,
                                Err(_) => {
                                    log::warn!(
                                        "artifact preview listener did not stop after aborting its connections"
                                    );
                                    Ok(())
                                }
                            }
                        }
                    }
                }
            };
            if let Err(error) = result {
                log::warn!("artifact preview listener stopped with an error: {error}");
            }
            registry.remove_if_current(&key, &served).await;
            registry.live.fetch_sub(1, Ordering::AcqRel);
        });
        Ok(opened(&session))
    }

    /// End the preview of one artifact. Returns whether one was open.
    pub(super) async fn close(&self, repo_id: &str, artifact_id: &str) -> bool {
        let removed = self
            .sessions
            .lock()
            .await
            .remove(&(repo_id.to_string(), artifact_id.to_string()));
        match removed {
            Some(handle) => {
                let _ = handle.cancel.send(());
                true
            }
            None => false,
        }
    }

    async fn remove_if_current(&self, key: &PreviewKey, session: &Arc<PreviewSession>) {
        let mut sessions = self.sessions.lock().await;
        if sessions
            .get(key)
            .is_some_and(|handle| Arc::ptr_eq(&handle.session, session))
        {
            sessions.remove(key);
        }
    }

    #[cfg(test)]
    pub(super) async fn expire_for_tests(&self, repo_id: &str, artifact_id: &str) {
        if let Some(handle) = self
            .sessions
            .lock()
            .await
            .get(&(repo_id.to_string(), artifact_id.to_string()))
        {
            handle.session.last_activity.store(0, Ordering::Release);
        }
    }
}

fn opened(session: &PreviewSession) -> OpenedArtifactPreview {
    OpenedArtifactPreview {
        repo_id: session.repo_id.clone(),
        artifact_id: session.artifact_id.clone(),
        entrypoint: session.entrypoint.clone(),
        url: session.url(),
        expires_at: session.hard_expires_at.saturating_mul(1000),
        idle_timeout_secs: IDLE_TTL.as_secs(),
    }
}

fn content_security_policy(port: u16) -> Result<HeaderValue, String> {
    let origins = format!("'self' http://127.0.0.1:{port} http://localhost:{port}");
    HeaderValue::from_str(&format!(
        "sandbox allow-scripts; default-src 'none'; \
         script-src {origins} 'unsafe-inline'; style-src {origins} 'unsafe-inline'; \
         img-src {origins} data: blob:; font-src {origins} data:; media-src {origins} data: blob:; \
         frame-src {origins}; child-src {origins}; connect-src 'none'; worker-src 'none'; \
         manifest-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'; \
         frame-ancestors {}",
        shell_origins(port)
    ))
    .map_err(|error| format!("invalid artifact preview policy: {error}"))
}

/// This listener's own origins: the only place the top-level shell lives.
fn shell_origins(port: u16) -> String {
    format!("http://127.0.0.1:{port} http://localhost:{port}")
}

/// The top-level document a browser navigation receives instead of the
/// content. Kanna-authored and script-free; it frames `path` (this request's
/// own path, so the frame's request carries the capability) sandboxed.
fn shell_response(session: &PreviewSession, path_and_query: &str, head: bool) -> Response {
    let escaped = path_and_query
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Kanna artifact preview</title>\
         <style>html,body,iframe{{margin:0;border:0;width:100%;height:100%;display:block}}</style>\
         <iframe sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" src=\"{escaped}\"></iframe>"
    );
    let origins = shell_origins(session.port);
    let policy = format!(
        "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; \
         frame-src {origins}; child-src {origins}; form-action 'none'; base-uri 'none'; \
         frame-ancestors 'none'"
    );
    let mut response = if head {
        Response::new(Body::empty())
    } else {
        Response::new(Body::from(html))
    };
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    if let Ok(policy) = HeaderValue::from_str(&policy) {
        headers.insert(CONTENT_SECURITY_POLICY, policy);
    }
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    response
}

async fn serve_artifact_request(
    State(session): State<Arc<PreviewSession>>,
    request: Request<Body>,
) -> Response {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(ALLOW, HeaderValue::from_static("GET, HEAD"))],
        )
            .into_response();
    }
    // A DNS-rebound name resolves here too; only the literal loopback
    // authority this listener handed out is served.
    let host_ok = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| {
            host == format!("127.0.0.1:{}", session.port)
                || host == format!("localhost:{}", session.port)
        });
    if !host_ok
        || request.uri().scheme().is_some()
        || request.uri().authority().is_some()
        || !session.is_current()
    {
        return not_found("not found");
    }
    let Some(rest) = request.uri().path().strip_prefix(CAPABILITY_PREFIX) else {
        return not_found("not found");
    };
    let (presented, file_path) = match rest.split_once('/') {
        Some((capability, file_path)) => (capability, Some(file_path)),
        None => (rest, None),
    };
    if !secret_matches(presented, &session.capability) {
        return not_found("not found");
    }
    session
        .last_activity
        .store(unix_seconds(), Ordering::Release);
    let top_level = request
        .headers()
        .get("sec-fetch-dest")
        .is_some_and(|dest| dest.as_bytes().eq_ignore_ascii_case(b"document"));
    if top_level {
        let path_and_query = request
            .uri()
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/");
        return shell_response(&session, path_and_query, request.method() == Method::HEAD);
    }
    let base = format!("{CAPABILITY_PREFIX}{}/", session.capability);
    let Some(raw_path) = file_path.filter(|path| !path.is_empty()) else {
        return redirect(&format!("{base}{}", encode_path(&session.entrypoint)));
    };
    let Some(mut decoded) = decode_path(raw_path) else {
        return (StatusCode::BAD_REQUEST, "malformed artifact path").into_response();
    };
    if decoded.ends_with('/') {
        decoded.push_str("index.html");
    }
    let head = request.method() == Method::HEAD;
    let reader = Arc::clone(&session);
    let requested = decoded.clone();
    let result = tokio::task::spawn_blocking(move || {
        let store = ArtifactStore::open_existing(&reader.repository_path, &reader.repo_id)?
            .ok_or_else(|| ArtifactError::NotFound {
                repo_id: reader.repo_id.clone(),
                artifact_id: reader.artifact_id.clone(),
            })?;
        match store.read_file(&reader.artifact_id, &requested) {
            Err(ArtifactError::FileNotFound { .. })
                if store.is_directory(&reader.artifact_id, &requested) =>
            {
                Ok(None)
            }
            other => other.map(Some),
        }
    })
    .await;
    let blob = match result {
        Ok(Ok(Some(blob))) => blob,
        Ok(Ok(None)) => return redirect(&format!("{base}{}/", encode_path(&decoded))),
        Ok(Err(error @ ArtifactError::InvalidPath(_))) => {
            return (StatusCode::BAD_REQUEST, error.to_string()).into_response()
        }
        Ok(Err(error)) => {
            return (super::artifacts::artifact_status(&error), error.to_string()).into_response()
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let media_type = media_type(&blob.path);
    let mut response = if head {
        Response::new(Body::empty())
    } else {
        Response::new(Body::from(blob.bytes))
    };
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&media_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    apply_isolation_headers(headers, &session);
    response
}

fn apply_isolation_headers(headers: &mut axum::http::HeaderMap, session: &PreviewSession) {
    headers.insert(
        CONTENT_SECURITY_POLICY,
        session.content_security_policy.clone(),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // No `Cross-Origin-Resource-Policy: same-origin`: the sandboxed page has
    // an opaque origin, so that header would block its own stylesheets,
    // scripts and images. The CSP above already confines loads to this
    // listener.
    headers.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
}

fn redirect(location: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(LOCATION, value);
    }
    response
        .headers_mut()
        .insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

fn not_found(message: &'static str) -> Response {
    (StatusCode::NOT_FOUND, message).into_response()
}

fn media_type(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, extension)| extension);
    let base = match extension {
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("ico") => "image/x-icon",
        Some("avif") => "image/avif",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("xml") => "application/xml",
        Some("wasm") => "application/wasm",
        _ => crate::task_files::task_file_media_type(path),
    };
    if base.starts_with("text/") || base == "image/svg+xml" || base == "application/json" {
        format!("{base}; charset=utf-8")
    } else {
        base.to_string()
    }
}

/// Percent-decode a request path into `/`-separated names. An encoded `/`,
/// NUL, backslash or invalid UTF-8 is refused rather than reinterpreted.
fn decode_path(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = raw.get(index + 1..index + 3)?;
            let byte = u8::from_str_radix(hex, 16).ok()?;
            if byte == b'/' || byte == 0 || byte == b'\\' {
                return None;
            }
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    (!decoded.contains('\0') && !decoded.contains('\\')).then_some(decoded)
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            segment
                .bytes()
                .map(|byte| {
                    if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                        (byte as char).to_string()
                    } else {
                        format!("%{byte:02X}")
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn secret_matches(presented: &str, expected: &str) -> bool {
    let presented = Sha256::digest(presented.as_bytes());
    let expected = Sha256::digest(expected.as_bytes());
    presented
        .iter()
        .zip(expected.iter())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

/// The preview's listener, handing out connections that can all be aborted
/// at once when the drain deadline passes.
struct TrackedListener {
    inner: TcpListener,
    aborted: watch::Receiver<bool>,
}

impl axum::serve::Listener for TrackedListener {
    type Io = AbortableStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (stream, address) = axum::serve::Listener::accept(&mut self.inner).await;
        (AbortableStream::new(stream, self.aborted.clone()), address)
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// A connection that fails every read and write once its listener is
/// aborted. The abort signal is polled alongside the socket, so a response
/// parked on a full send buffer (a client that stopped reading) is woken and
/// ends too.
struct AbortableStream {
    inner: TcpStream,
    aborted: Pin<Box<dyn Future<Output = ()> + Send>>,
    dead: bool,
}

impl AbortableStream {
    fn new(inner: TcpStream, mut aborted: watch::Receiver<bool>) -> Self {
        Self {
            inner,
            aborted: Box::pin(async move {
                if aborted.wait_for(|aborted| *aborted).await.is_err() {
                    // The session is gone without aborting: never fire.
                    std::future::pending::<()>().await;
                }
            }),
            dead: false,
        }
    }

    fn check(&mut self, context: &mut Context<'_>) -> std::io::Result<()> {
        if !self.dead && self.aborted.as_mut().poll(context).is_ready() {
            self.dead = true;
        }
        if self.dead {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "artifact preview session ended",
            ));
        }
        Ok(())
    }
}

impl AsyncRead for AbortableStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Err(error) = self.check(context) {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl AsyncWrite for AbortableStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if let Err(error) = self.check(context) {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.inner).poll_write(context, bytes)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Err(error) = self.check(context) {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.dead {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

async fn wait_for_expiry(session: Arc<PreviewSession>) {
    while session.is_current() {
        tokio::time::sleep(EXPIRY_POLL).await;
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_decoding_refuses_encoded_separators_and_invalid_utf8() {
        assert_eq!(
            decode_path("css/site%20main.css").as_deref(),
            Some("css/site main.css")
        );
        assert_eq!(decode_path("a%2Fb"), None);
        assert_eq!(decode_path("a%2fb"), None);
        assert_eq!(decode_path("a%00b"), None);
        assert_eq!(decode_path("a%5Cb"), None);
        assert_eq!(decode_path("a%ff"), None);
        assert_eq!(decode_path("a%2"), None);
        assert_eq!(encode_path("css/site main.css"), "css/site%20main.css");
    }

    #[test]
    fn text_types_declare_utf8_and_fonts_are_typed() {
        assert_eq!(media_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(media_type("a/b.JS"), "text/javascript; charset=utf-8");
        assert_eq!(media_type("logo.png"), "image/png");
        assert_eq!(media_type("f.woff2"), "font/woff2");
        assert_eq!(media_type("blob.bin"), "application/octet-stream");
    }
}
