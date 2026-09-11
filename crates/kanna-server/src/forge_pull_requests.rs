//! Confirming pull-request outcomes with the forge.
//!
//! Kanna owns this path. A signed desktop cannot depend on a developer's
//! separately installed `gh`, and a repository-wide listing cannot prove the
//! state of an old known PR that fell off a page. The adapter therefore asks
//! GitHub's REST API for each unresolved canonical identity and persists only
//! responses that name that exact PR.

use crate::db::{Db, ForgePullRequestObservation, UnresolvedPullRequest};
use serde::Deserialize;
use std::time::Duration;

const RECHECK_AFTER: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeAvailability {
    Confirmed,
    Unavailable,
}

/// Server-owned, bundled GitHub transport. The optional base URL exists so
/// route tests can exercise the real HTTP adapter deterministically.
#[derive(Clone)]
pub struct ForgeClient {
    token: Option<String>,
    api_base_override: Option<String>,
    connect_timeout: Duration,
    request_timeout: Duration,
}

impl ForgeClient {
    pub fn from_environment() -> Self {
        Self::new(
            std::env::var("KANNA_GITHUB_TOKEN")
                .ok()
                .filter(|token| !token.trim().is_empty()),
            None,
            CONNECT_TIMEOUT,
            REQUEST_TIMEOUT,
        )
    }

    fn new(
        token: Option<String>,
        api_base_override: Option<String>,
        connect_timeout: Duration,
        request_timeout: Duration,
    ) -> Self {
        Self {
            token,
            api_base_override,
            connect_timeout,
            request_timeout,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(base_url: String, token: Option<&str>, timeout: Duration) -> Self {
        Self::new(token.map(str::to_string), Some(base_url), timeout, timeout)
    }

    fn query_pull_request(
        &self,
        http: &reqwest::blocking::Client,
        identity: &UnresolvedPullRequest,
    ) -> Result<ForgePullRequestObservation, String> {
        let token = self
            .token
            .as_deref()
            .ok_or_else(|| "KANNA_GITHUB_TOKEN is not configured".to_string())?;
        let parsed = GithubPullRequestIdentity::parse(&identity.pr_url, identity.pr_number)
            .ok_or_else(|| format!("unrecognized pull request URL: {}", identity.pr_url))?;
        let base = self
            .api_base_override
            .clone()
            .unwrap_or_else(|| parsed.api_base());
        let response = http
            .get(format!(
                "{}/repos/{}/{}/pulls/{}",
                base.trim_end_matches('/'),
                parsed.owner,
                parsed.repo,
                parsed.number
            ))
            .header(reqwest::header::USER_AGENT, "Kanna")
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .bearer_auth(token)
            .send()
            .map_err(|error| format!("request failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("forge returned HTTP {}", response.status()));
        }
        let answer: GithubPullRequest = response
            .json()
            .map_err(|error| format!("invalid forge response: {error}"))?;
        if answer.number != Some(parsed.number) {
            return Err("forge response did not identify the requested pull request".to_string());
        }
        let state = answer
            .state
            .ok_or_else(|| "forge response omitted pull request state".to_string())?;
        let state = if answer.merged_at.is_some() {
            "MERGED".to_string()
        } else {
            state.to_ascii_uppercase()
        };
        if !matches!(state.as_str(), "OPEN" | "CLOSED" | "MERGED") {
            return Err(format!(
                "forge returned unknown pull request state `{state}`"
            ));
        }
        Ok(ForgePullRequestObservation {
            pr_number: parsed.number,
            // Use the identity Kanna already trusts, not an optional response
            // URL, so URL-only legacy rows are updated by their existing key.
            url: Some(identity.pr_url.clone()),
            created_at: answer.created_at,
            merged_at: answer.merged_at,
            state: Some(state),
        })
    }

    fn http_client(&self) -> Result<reqwest::blocking::Client, String> {
        reqwest::blocking::Client::builder()
            .connect_timeout(self.connect_timeout)
            .timeout(self.request_timeout)
            .build()
            .map_err(|error| format!("could not build forge HTTP client: {error}"))
    }
}

#[derive(Debug)]
struct GithubPullRequestIdentity {
    host: String,
    owner: String,
    repo: String,
    number: i64,
}

impl GithubPullRequestIdentity {
    fn parse(url: &str, recorded_number: Option<i64>) -> Option<Self> {
        let trimmed = url.trim();
        let without_scheme = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))?;
        let without_www = without_scheme
            .strip_prefix("www.")
            .unwrap_or(without_scheme);
        let mut parts = without_www.split('/');
        let host = parts.next()?.to_string();
        let owner = parts.next()?.to_string();
        let repo = parts.next()?.to_string();
        if parts.next()? != "pull" {
            return None;
        }
        let number = parts.next()?.parse().ok()?;
        if recorded_number.is_some_and(|recorded| recorded != number) {
            return None;
        }
        Some(Self {
            host,
            owner,
            repo,
            number,
        })
    }

    fn api_base(&self) -> String {
        if self.host.eq_ignore_ascii_case("github.com") {
            "https://api.github.com".to_string()
        } else {
            format!("https://{}/api/v3", self.host)
        }
    }
}

#[derive(Debug, Deserialize)]
struct GithubPullRequest {
    number: Option<i64>,
    created_at: Option<String>,
    merged_at: Option<String>,
    state: Option<String>,
}

/// Refresh every unresolved identity that is not already fresh. A partial
/// answer may preserve the facts it did confirm, but availability stays false
/// until every nonterminal known PR has a fresh individual confirmation.
pub fn reconcile_repo_pull_requests(
    db: &Db,
    repo_id: &str,
    client: &ForgeClient,
) -> ForgeAvailability {
    let unresolved = match db.unresolved_repo_pull_requests(repo_id) {
        Ok(unresolved) => unresolved,
        Err(error) => {
            log::warn!("analytics: reading unresolved pull requests failed: {error}");
            return ForgeAvailability::Unavailable;
        }
    };
    if unresolved.is_empty() {
        return ForgeAvailability::Confirmed;
    }

    let now = now_epoch_seconds();
    let stale: Vec<_> = unresolved
        .into_iter()
        .filter(|pull_request| !recently_reconciled(pull_request.forge_checked_at, now))
        .collect();
    if stale.is_empty() {
        return ForgeAvailability::Confirmed;
    }

    let http = match client.http_client() {
        Ok(http) => http,
        Err(error) => {
            log::info!("analytics: {error}");
            return ForgeAvailability::Unavailable;
        }
    };

    let mut observations = Vec::with_capacity(stale.len());
    let mut complete = true;
    for identity in &stale {
        match client.query_pull_request(&http, identity) {
            Ok(observation) => observations.push(observation),
            Err(error) => {
                complete = false;
                log::info!(
                    "analytics: could not confirm pull request {}: {error}",
                    identity.pr_key
                );
            }
        }
    }
    let recorded = match db.record_forge_pull_requests(repo_id, &observations) {
        Ok(recorded) => recorded,
        Err(error) => {
            log::warn!("analytics: recording pull request facts failed: {error}");
            return ForgeAvailability::Unavailable;
        }
    };
    if complete && recorded == stale.len() {
        ForgeAvailability::Confirmed
    } else {
        ForgeAvailability::Unavailable
    }
}

fn now_epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn recently_reconciled(last_checked_epoch_seconds: Option<i64>, now_epoch_seconds: i64) -> bool {
    let Some(last) = last_checked_epoch_seconds else {
        return false;
    };
    now_epoch_seconds.saturating_sub(last) < RECHECK_AFTER.as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::{
        recently_reconciled, reconcile_repo_pull_requests, ForgeAvailability, ForgeClient,
        GithubPullRequestIdentity,
    };
    use crate::db::{AnalyticsRange, Db, ForgePullRequestObservation};
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    struct MockResponse {
        status: u16,
        body: String,
        delay: Duration,
    }

    fn spawn_forge(
        responses: HashMap<String, MockResponse>,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind forge fixture");
        let address = listener.local_addr().expect("fixture address");
        let handle = std::thread::spawn(move || {
            for _ in 0..responses.len() {
                let (mut socket, _) = listener.accept().expect("accept forge request");
                let mut request = [0_u8; 4096];
                let bytes = socket.read(&mut request).expect("read request");
                let request = String::from_utf8_lossy(&request[..bytes]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .expect("request path");
                assert!(request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-token"));
                let response = responses.get(path).expect("expected request path");
                std::thread::sleep(response.delay);
                let reason = if response.status == 200 {
                    "OK"
                } else {
                    "Error"
                };
                let encoded = format!(
                    "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response.status,
                    reason,
                    response.body.len(),
                    response.body
                );
                let _ = socket.write_all(encoded.as_bytes());
            }
        });
        (format!("http://{address}"), handle)
    }

    fn response(number: i64, state: &str, merged_at: Option<&str>) -> String {
        serde_json::json!({
            "number": number,
            "created_at": "2026-04-17T08:00:00Z",
            "merged_at": merged_at,
            "state": state,
        })
        .to_string()
    }

    fn db(label: &str) -> Db {
        let db = Db::open_for_tests(&Db::test_db_path(label)).expect("open db");
        db.insert_test_repo("repo-1", "Repo One").expect("repo");
        db
    }

    #[test]
    fn a_recent_confirmation_is_reused_and_an_old_one_is_not() {
        assert!(recently_reconciled(Some(1_000), 1_100));
        assert!(!recently_reconciled(Some(1_000), 1_400));
        assert!(!recently_reconciled(None, 1_000));
    }

    #[test]
    fn url_only_pull_request_identity_supplies_its_number() {
        let parsed = GithubPullRequestIdentity::parse(
            "https://github.example/acme/widgets/pull/314/files",
            None,
        )
        .expect("identity");
        assert_eq!(parsed.number, 314);
        assert_eq!(parsed.api_base(), "https://github.example/api/v3");
    }

    #[test]
    fn missing_credentials_and_network_leave_state_unconfirmed() {
        let db = db("forge-unavailable");
        let url = "https://github.com/acme/widgets/pull/1";
        db.insert_test_unresolved_pull_request("repo-1", Some(1), url, None)
            .expect("pr");
        let no_credentials = ForgeClient::for_tests(
            "http://127.0.0.1:1".to_string(),
            None,
            Duration::from_millis(20),
        );
        assert_eq!(
            reconcile_repo_pull_requests(&db, "repo-1", &no_credentials),
            ForgeAvailability::Unavailable
        );
        let no_network = ForgeClient::for_tests(
            "http://127.0.0.1:1".to_string(),
            Some("test-token"),
            Duration::from_millis(20),
        );
        assert_eq!(
            reconcile_repo_pull_requests(&db, "repo-1", &no_network),
            ForgeAvailability::Unavailable
        );
    }

    #[test]
    fn partial_response_does_not_claim_the_repository_is_confirmed() {
        let db = db("forge-partial");
        for number in [1, 2] {
            db.insert_test_unresolved_pull_request(
                "repo-1",
                Some(number),
                &format!("https://github.com/acme/widgets/pull/{number}"),
                None,
            )
            .expect("pr");
        }
        let (base, server) = spawn_forge(HashMap::from([
            (
                "/repos/acme/widgets/pulls/1".to_string(),
                MockResponse {
                    status: 200,
                    body: response(1, "open", None),
                    delay: Duration::ZERO,
                },
            ),
            (
                "/repos/acme/widgets/pulls/2".to_string(),
                MockResponse {
                    status: 503,
                    body: "{}".to_string(),
                    delay: Duration::ZERO,
                },
            ),
        ]));
        let client = ForgeClient::for_tests(base, Some("test-token"), Duration::from_secs(1));
        assert_eq!(
            reconcile_repo_pull_requests(&db, "repo-1", &client),
            ForgeAvailability::Unavailable
        );
        server.join().expect("forge server");
        let unresolved = db
            .unresolved_repo_pull_requests("repo-1")
            .expect("unresolved");
        assert!(unresolved
            .iter()
            .any(|pr| pr.pr_number == Some(1) && pr.forge_checked_at.is_some()));
        assert!(unresolved
            .iter()
            .any(|pr| pr.pr_number == Some(2) && pr.forge_checked_at.is_none()));
    }

    #[test]
    fn url_only_and_old_known_pull_requests_are_queried_directly() {
        let db = db("forge-url-only-old");
        let url = "https://github.com/acme/widgets/pull/1";
        db.insert_test_unresolved_pull_request("repo-1", None, url, None)
            .expect("url-only pr");
        let (base, server) = spawn_forge(HashMap::from([(
            "/repos/acme/widgets/pulls/1".to_string(),
            MockResponse {
                status: 200,
                body: response(1, "open", None),
                delay: Duration::ZERO,
            },
        )]));
        let client = ForgeClient::for_tests(base, Some("test-token"), Duration::from_secs(1));
        assert_eq!(
            reconcile_repo_pull_requests(&db, "repo-1", &client),
            ForgeAvailability::Confirmed,
            "a known old PR is fetched by identity even if more than 200 unrelated PRs are newer"
        );
        server.join().expect("forge server");
        let unresolved = db
            .unresolved_repo_pull_requests("repo-1")
            .expect("unresolved");
        assert_eq!(unresolved[0].pr_number, Some(1));
        assert!(unresolved[0].forge_checked_at.is_some());
    }

    #[test]
    fn merged_and_closed_are_distinct_terminal_outcomes() {
        let db = db("forge-terminal-states");
        for number in [7, 8] {
            db.insert_test_unresolved_pull_request(
                "repo-1",
                Some(number),
                &format!("https://github.com/acme/widgets/pull/{number}"),
                None,
            )
            .expect("pr");
        }
        let (base, server) = spawn_forge(HashMap::from([
            (
                "/repos/acme/widgets/pulls/7".to_string(),
                MockResponse {
                    status: 200,
                    body: response(7, "closed", Some("2026-04-18T08:00:00Z")),
                    delay: Duration::ZERO,
                },
            ),
            (
                "/repos/acme/widgets/pulls/8".to_string(),
                MockResponse {
                    status: 200,
                    body: response(8, "closed", None),
                    delay: Duration::ZERO,
                },
            ),
        ]));
        let client = ForgeClient::for_tests(base, Some("test-token"), Duration::from_secs(1));
        assert_eq!(
            reconcile_repo_pull_requests(&db, "repo-1", &client),
            ForgeAvailability::Confirmed
        );
        server.join().expect("forge server");
        assert_eq!(
            db.test_pull_request_state("repo-1", "https://github.com/acme/widgets/pull/7")
                .expect("merged state")
                .as_deref(),
            Some("MERGED")
        );
        assert_eq!(
            db.test_pull_request_state("repo-1", "https://github.com/acme/widgets/pull/8")
                .expect("closed state")
                .as_deref(),
            Some("CLOSED")
        );
    }

    #[test]
    fn a_stale_closed_pull_request_can_reopen_and_change_analytics() {
        let db = db("forge-closed-reopened");
        let url = "https://github.com/acme/widgets/pull/8";
        db.insert_test_unresolved_pull_request("repo-1", Some(8), url, None)
            .expect("pr");
        db.record_forge_pull_requests(
            "repo-1",
            &[ForgePullRequestObservation {
                pr_number: 8,
                url: Some(url.to_string()),
                created_at: Some("2026-04-17T08:00:00Z".to_string()),
                merged_at: None,
                state: Some("CLOSED".to_string()),
            }],
        )
        .expect("record closed state");

        let range = AnalyticsRange {
            from: "2026-04-16".to_string(),
            to: "2026-04-20".to_string(),
        };
        let before = db
            .repo_analytics("repo-1", &range, true, Vec::new())
            .expect("closed analytics");
        assert_eq!(before.pull_requests.open_now, Some(0));

        db.set_test_pull_request_forge_timestamps(
            "repo-1",
            url,
            "2000-01-01 00:00:00",
            "2000-01-01 00:00:00",
        )
        .expect("age closed confirmation past freshness");
        let (base, server) = spawn_forge(HashMap::from([(
            "/repos/acme/widgets/pulls/8".to_string(),
            MockResponse {
                status: 200,
                body: response(8, "open", None),
                delay: Duration::ZERO,
            },
        )]));
        let client = ForgeClient::for_tests(base, Some("test-token"), Duration::from_secs(1));
        assert_eq!(
            reconcile_repo_pull_requests(&db, "repo-1", &client),
            ForgeAvailability::Confirmed
        );
        server.join().expect("forge server");

        assert_eq!(
            db.test_pull_request_state("repo-1", url)
                .expect("reopened state")
                .as_deref(),
            Some("OPEN")
        );
        let after = db
            .repo_analytics("repo-1", &range, true, Vec::new())
            .expect("reopened analytics");
        assert_eq!(after.pull_requests.open_now, Some(1));
    }

    #[test]
    fn one_fresh_row_does_not_mask_an_unchecked_row() {
        let db = db("forge-per-row-freshness");
        db.insert_test_unresolved_pull_request(
            "repo-1",
            Some(1),
            "https://github.com/acme/widgets/pull/1",
            Some("2099-01-01 00:00:00"),
        )
        .expect("fresh pr");
        db.insert_test_unresolved_pull_request(
            "repo-1",
            Some(2),
            "https://github.com/acme/widgets/pull/2",
            None,
        )
        .expect("unchecked pr");
        let client = ForgeClient::for_tests(
            "http://127.0.0.1:1".to_string(),
            None,
            Duration::from_millis(20),
        );
        assert_eq!(
            reconcile_repo_pull_requests(&db, "repo-1", &client),
            ForgeAvailability::Unavailable
        );
    }

    #[test]
    fn forge_requests_have_a_bounded_timeout() {
        let db = db("forge-timeout");
        db.insert_test_unresolved_pull_request(
            "repo-1",
            Some(9),
            "https://github.com/acme/widgets/pull/9",
            None,
        )
        .expect("pr");
        let (base, server) = spawn_forge(HashMap::from([(
            "/repos/acme/widgets/pulls/9".to_string(),
            MockResponse {
                status: 200,
                body: response(9, "open", None),
                delay: Duration::from_millis(100),
            },
        )]));
        let client = ForgeClient::for_tests(base, Some("test-token"), Duration::from_millis(20));
        assert_eq!(
            reconcile_repo_pull_requests(&db, "repo-1", &client),
            ForgeAvailability::Unavailable
        );
        server.join().expect("forge server");
    }
}
