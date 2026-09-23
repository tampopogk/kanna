//! Retention against artifact sharing (T6b × T7): a sweep never prunes what
//! a fetch or push is holding without a ref, never lets an expired tree
//! reach the remote, and treats received content by this home's rules only.

use super::remote::{fetch, pause, push, ArtifactRemote};
use super::store::{
    set_test_clock, ArtifactStore, BindingRequest, CommentRequest, PublishLimits, PublishRequest,
    TaskLifecycle, DISCARD_ON_CLOSE_GRACE, THIRTY_DAYS,
};
use super::tests::{git, Fixture};
use super::types::{ArtifactContentKind, ArtifactRetention};
use super::ArtifactError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const T0: u64 = 1_800_000_000;

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

struct Sharing {
    fixture: Fixture,
    remote: ArtifactRemote,
}

impl Sharing {
    fn new(label: &str) -> Self {
        let fixture = Fixture::new(label);
        git(&fixture.root, &["init", "--bare", "--quiet", "remote.git"]);
        let remote = ArtifactRemote::parse(
            fixture.root.join("remote.git").to_str().unwrap(),
            &fixture.root,
            &fixture.root.join("a/artifacts.git"),
        )
        .unwrap();
        Self { fixture, remote }
    }

    fn path(&self, home: &str) -> PathBuf {
        self.fixture.root.join(home).join("artifacts.git")
    }

    fn home(&self, home: &str) -> ArtifactStore {
        ArtifactStore::open_or_create(&self.path(home), &format!("repo-{home}")).unwrap()
    }

    fn publish(
        &self,
        store: &ArtifactStore,
        source: &str,
        task_id: &str,
        retention: ArtifactRetention,
    ) -> String {
        store
            .publish(PublishRequest {
                task_id,
                workspace_root: &self.fixture.root.join("workspace"),
                source_path: source,
                kind: ArtifactContentKind::Document,
                entrypoint: None,
                previous: None,
                retention,
                limits: PublishLimits::default(),
            })
            .unwrap()
            .artifact_id
    }

    fn remote_refs(&self) -> Vec<String> {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(self.fixture.root.join("remote.git"))
            .args(["for-each-ref", "--format=%(refname)"])
            .output()
            .unwrap();
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn comment(store: &ArtifactStore, id: &str) {
    store
        .record_comment(
            id,
            CommentRequest {
                author: "reviewer",
                body: "noted",
                anchor: None,
            },
        )
        .unwrap();
}

fn lifecycles(entries: &[(&str, TaskLifecycle)]) -> impl Fn(&str) -> TaskLifecycle + Send {
    let map = entries
        .iter()
        .map(|(id, lifecycle)| (id.to_string(), *lifecycle))
        .collect::<HashMap<_, _>>();
    move |id: &str| map.get(id).copied().unwrap_or(TaskLifecycle::Unknown)
}

fn git_ok(repository: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("--git-dir")
        .arg(repository)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
        .status
        .success()
}

/// Every object every ref reaches is present.
fn assert_no_object_loss(repository: &Path) {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(repository)
        .args(["fsck", "--connectivity-only", "--no-dangling"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} lost objects: {}",
        repository.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn incoming_refs(repository: &Path) -> String {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(repository)
        .args([
            "for-each-ref",
            "--format=%(refname)",
            "refs/kanna/artifacts/incoming/",
        ])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap()
}

/// A fetch parked between validation and import (its objects protected by
/// nothing but the repository lock) holds a concurrent sweep off until the
/// import has given them refs. The sweep then collects only what it should.
#[test]
fn a_sweep_waits_for_a_fetch_and_its_import_and_loses_nothing() {
    let sharing = Sharing::new("sweep-vs-fetch");
    sharing
        .fixture
        .write("workspace/shared/index.html", b"<p>shared</p>");
    sharing.fixture.write("workspace/shared/a.css", b"p{}");
    sharing.fixture.write("workspace/old.md", b"old local work");
    let a = sharing.home("a");
    let b = sharing.home("b");
    let shared = sharing.publish(&a, "shared", "task-a", ArtifactRetention::Keep);
    comment(&a, &shared);
    push(&a, &sharing.remote, &shared).unwrap();
    let old = sharing.publish(&b, "old.md", "task-old", ArtifactRetention::DiscardOnClose);

    let b_path = sharing.path("b");
    let (reached, release) = pause::arm(&b_path, "fetch-before-import");
    let fetcher = {
        let path = b_path.clone();
        let remote = sharing.remote.clone();
        let shared = shared.clone();
        std::thread::spawn(move || {
            let b = ArtifactStore::open_existing(&path, "repo-b")
                .unwrap()
                .unwrap();
            fetch(&b, &remote, &shared)
        })
    };
    reached
        .recv_timeout(Duration::from_secs(60))
        .expect("the fetch never reached its import");
    // The fetched objects are in B with no ref naming them.
    assert!(incoming_refs(&b_path).is_empty());
    assert!(git_ok(&b_path, &["cat-file", "-e", &shared]));

    let sweeper = {
        let path = b_path.clone();
        std::thread::spawn(move || {
            let b = ArtifactStore::open_existing(&path, "repo-b")
                .unwrap()
                .unwrap();
            b.sweep_retention(
                SystemTime::now() + Duration::from_secs(3 * 60 * 60),
                ArtifactRetention::Keep,
                &lifecycles(&[(
                    "task-old",
                    TaskLifecycle::Closed {
                        at: SystemTime::now(),
                    },
                )]),
            )
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        !sweeper.is_finished(),
        "the sweep ran while a fetch held unreferenced objects"
    );
    release.send(()).unwrap();

    let fetched = fetcher.join().unwrap().unwrap();
    assert_eq!(fetched.content_retained, vec![shared.clone()]);
    let sweep = sweeper.join().unwrap().unwrap();
    assert_eq!(sweep.expired, vec![old.clone()]);
    assert_eq!(sweep.prune_error, None);

    // Nothing the fetch imported was lost to the prune...
    assert_no_object_loss(&b_path);
    let detail = b.detail(&shared).unwrap();
    assert!(detail.retained && !detail.expired);
    assert_eq!(detail.files.len(), 2);
    assert_eq!(detail.comments.len(), 1);
    assert_eq!(
        b.read_file(&shared, "a.css").unwrap().bytes,
        b"p{}".to_vec()
    );
    // ...and the expired tree is gone, with no private refs left over.
    assert!(!git_ok(&b_path, &["cat-file", "-e", &old]));
    assert!(b.detail(&old).unwrap().expired);
    assert!(incoming_refs(&b_path).is_empty());
}

/// A push parked with its record commits built but not yet sent holds a
/// concurrent sweep off, so the push neither loses those objects nor sends
/// a tree that expired under it; once a tree has expired, no push sends it.
#[test]
fn a_sweep_waits_for_a_push_and_an_expired_tree_never_reaches_the_remote() {
    let sharing = Sharing::new("sweep-vs-push");
    sharing
        .fixture
        .write("workspace/pushed.md", b"pushed while retained");
    sharing
        .fixture
        .write("workspace/expired.md", b"expired before any push");
    let a = sharing.home("a");
    let pushed = sharing.publish(&a, "pushed.md", "task-p", ArtifactRetention::DiscardOnClose);
    comment(&a, &pushed);
    let expired = sharing.publish(
        &a,
        "expired.md",
        "task-e",
        ArtifactRetention::DiscardOnClose,
    );
    let closed = lifecycles(&[
        (
            "task-p",
            TaskLifecycle::Closed {
                at: SystemTime::now(),
            },
        ),
        (
            "task-e",
            TaskLifecycle::Closed {
                at: SystemTime::now(),
            },
        ),
    ]);
    let later = SystemTime::now() + DISCARD_ON_CLOSE_GRACE + Duration::from_secs(60);

    // A tree retention already collected cannot be pushed.
    let sweep = a
        .sweep_retention(later, ArtifactRetention::Keep, &|id| {
            if id == "task-e" {
                closed(id)
            } else {
                TaskLifecycle::Open
            }
        })
        .unwrap();
    assert_eq!(sweep.expired, vec![expired.clone()]);
    assert!(matches!(
        push(&a, &sharing.remote, &expired),
        Err(ArtifactError::ContentMissing { .. })
    ));
    assert!(
        sharing.remote_refs().is_empty(),
        "an expired tree's push wrote to the remote: {:?}",
        sharing.remote_refs()
    );

    let a_path = sharing.path("a");
    let (reached, release) = pause::arm(&a_path, "push-before-send");
    let pusher = {
        let path = a_path.clone();
        let remote = sharing.remote.clone();
        let pushed = pushed.clone();
        std::thread::spawn(move || {
            let a = ArtifactStore::open_existing(&path, "repo-a")
                .unwrap()
                .unwrap();
            push(&a, &remote, &pushed)
        })
    };
    reached
        .recv_timeout(Duration::from_secs(60))
        .expect("the push never reached its send");
    let sweeper = {
        let path = a_path.clone();
        std::thread::spawn(move || {
            let a = ArtifactStore::open_existing(&path, "repo-a")
                .unwrap()
                .unwrap();
            a.sweep_retention(later, ArtifactRetention::Keep, &closed)
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        !sweeper.is_finished(),
        "the sweep ran while a push held unreferenced record commits"
    );
    release.send(()).unwrap();

    let outcome = pusher.join().unwrap().expect("the push lost its objects");
    // Content, version and comment.
    assert_eq!(outcome.created_refs.len(), 3, "{outcome:?}");
    let sweep = sweeper.join().unwrap().unwrap();
    assert_eq!(sweep.expired, vec![pushed.clone()]);

    // The remote holds exactly what was pushed while it was retained, whole.
    assert_no_object_loss(&sharing.fixture.root.join("remote.git"));
    let refs = sharing.remote_refs();
    assert_eq!(refs.len(), 3, "{refs:?}");
    assert!(refs.iter().all(|name| !name.contains(&expired)), "{refs:?}");
    assert_no_object_loss(&a_path);
    // Now expired here, it cannot be pushed again.
    assert!(matches!(
        push(&a, &sharing.remote, &pushed),
        Err(ArtifactError::ContentMissing { .. })
    ));
}

/// Fetch `id` into a new home at `received`, on this thread's clock.
fn receive(sharing: &Sharing, home: &str, id: &str, received: SystemTime) -> ArtifactStore {
    let store = sharing.home(home);
    set_test_clock(Some(received));
    let fetched = fetch(&store, &sharing.remote, id);
    set_test_clock(None);
    fetched.unwrap();
    store
}

/// Received content follows this repository's policy from when it arrived.
/// The task id its producer had on the other home is never looked up here:
/// a local task that happens to carry the same id, open or closed, neither
/// keeps it nor releases it.
#[test]
fn received_content_follows_local_rules_and_never_a_remote_producer() {
    let sharing = Sharing::new("received-retention");
    sharing.fixture.write("workspace/mock.md", b"their mockup");
    let a = sharing.home("a");
    // On A it is kept forever; that policy is A's, not ours.
    let id = sharing.publish(&a, "mock.md", "task-a", ArtifactRetention::Keep);
    push(&a, &sharing.remote, &id).unwrap();

    // A local task named like the remote producer is open: it must not keep
    // the received content.
    let b = receive(&sharing, "b", &id, at(T0));
    let namesake_open = lifecycles(&[("task-a", TaskLifecycle::Open)]);
    let sweep = b
        .sweep_retention(
            at(T0) + DISCARD_ON_CLOSE_GRACE - Duration::from_secs(1),
            ArtifactRetention::DiscardOnClose,
            &namesake_open,
        )
        .unwrap();
    assert!(sweep.expired.is_empty());
    let sweep = b
        .sweep_retention(
            at(T0) + DISCARD_ON_CLOSE_GRACE,
            ArtifactRetention::DiscardOnClose,
            &namesake_open,
        )
        .unwrap();
    assert_eq!(sweep.expired, vec![id.clone()]);
    let detail = b.detail(&id).unwrap();
    assert!(detail.expired);
    assert_eq!(
        detail.expirations[0].policies,
        vec![ArtifactRetention::DiscardOnClose]
    );
    assert_eq!(detail.versions.len(), 1, "the received version was kept");

    // A namesake closed long ago must not release it either: under `keep`
    // received content stays.
    let c = receive(&sharing, "c", &id, at(T0));
    let namesake_closed = lifecycles(&[(
        "task-a",
        TaskLifecycle::Closed {
            at: at(T0 - 400 * 86_400),
        },
    )]);
    let sweep = c
        .sweep_retention(
            at(T0) + THIRTY_DAYS * 20,
            ArtifactRetention::Keep,
            &namesake_closed,
        )
        .unwrap();
    assert!(sweep.expired.is_empty());
    // Under `30-days` it goes 30 days after it arrived.
    let sweep = c
        .sweep_retention(
            at(T0) + THIRTY_DAYS - Duration::from_secs(1),
            ArtifactRetention::ThirtyDays,
            &namesake_closed,
        )
        .unwrap();
    assert!(sweep.expired.is_empty());
    let sweep = c
        .sweep_retention(
            at(T0) + THIRTY_DAYS,
            ArtifactRetention::ThirtyDays,
            &namesake_closed,
        )
        .unwrap();
    assert_eq!(sweep.expired, vec![id.clone()]);

    // A local result that named it keeps it while that task is open, and
    // the clock then runs from that task's close.
    let d = receive(&sharing, "d", &id, at(T0));
    d.bind_to_result(
        "task-review",
        Some("run-review"),
        &[BindingRequest {
            name: "their mockup",
            artifact_id: &id,
            kind: None,
        }],
    )
    .unwrap();
    let sweep = d
        .sweep_retention(
            at(T0) + THIRTY_DAYS * 20,
            ArtifactRetention::DiscardOnClose,
            &lifecycles(&[("task-review", TaskLifecycle::Open)]),
        )
        .unwrap();
    assert!(
        sweep.expired.is_empty(),
        "collected content an open task named"
    );
    let closed_at = at(T0) + THIRTY_DAYS;
    let reviewer_closed = lifecycles(&[("task-review", TaskLifecycle::Closed { at: closed_at })]);
    let sweep = d
        .sweep_retention(
            closed_at + DISCARD_ON_CLOSE_GRACE - Duration::from_secs(1),
            ArtifactRetention::DiscardOnClose,
            &reviewer_closed,
        )
        .unwrap();
    assert!(sweep.expired.is_empty());
    let sweep = d
        .sweep_retention(
            closed_at + DISCARD_ON_CLOSE_GRACE,
            ArtifactRetention::DiscardOnClose,
            &reviewer_closed,
        )
        .unwrap();
    assert_eq!(sweep.expired, vec![id]);
}

fn sequence_of(record_id: &str) -> u64 {
    record_id
        .strip_prefix('s')
        .and_then(|rest| rest.split_once('-'))
        .and_then(|(number, _)| number.parse().ok())
        .unwrap_or_else(|| panic!("{record_id} is not a sequence-form record id"))
}

/// A home that imports a version whose id carries a high sequence number
/// and then publishes the same artifact itself: its own publication is the
/// latest version, and every id it mints next sorts after the imported one.
#[test]
fn a_local_publication_after_an_import_is_the_latest_version() {
    let sharing = Sharing::new("sequence-after-import");
    sharing.fixture.write("workspace/filler.md", b"filler");
    sharing
        .fixture
        .write("workspace/site/index.html", b"<p>index</p>");
    sharing
        .fixture
        .write("workspace/site/other.html", b"<p>other</p>");
    let publish_site = |store: &ArtifactStore, task_id: &str, entrypoint: &str| {
        store
            .publish(PublishRequest {
                task_id,
                workspace_root: &sharing.fixture.root.join("workspace"),
                source_path: "site",
                kind: ArtifactContentKind::Mockup,
                entrypoint: Some(entrypoint),
                previous: None,
                retention: ArtifactRetention::Keep,
                limits: PublishLimits::default(),
            })
            .unwrap()
    };

    // A's sequence runs well ahead of a fresh home's.
    let a = sharing.home("a");
    let filler = sharing.publish(&a, "filler.md", "task-a", ArtifactRetention::Keep);
    for _ in 0..20 {
        comment(&a, &filler);
    }
    let theirs = publish_site(&a, "task-a", "index.html");
    let imported_sequence = sequence_of(&theirs.version.record_id);
    assert!(imported_sequence > 20, "{}", theirs.version.record_id);
    push(&a, &sharing.remote, &theirs.artifact_id).unwrap();

    let b = sharing.home("b");
    fetch(&b, &sharing.remote, &theirs.artifact_id).unwrap();
    let ours = publish_site(&b, "task-b", "other.html");
    assert_eq!(ours.artifact_id, theirs.artifact_id);
    assert!(
        sequence_of(&ours.version.record_id) > imported_sequence,
        "local {} minted at or before imported {}",
        ours.version.record_id,
        theirs.version.record_id
    );

    let id = theirs.artifact_id.clone();
    assert_eq!(b.entrypoint(&id).unwrap().as_deref(), Some("other.html"));
    let versions = b.detail(&id).unwrap().versions;
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].record_id, theirs.version.record_id);
    assert_eq!(versions[1].record_id, ours.version.record_id);
    assert_eq!(versions[1].produced_by.task_id, "task-b");

    // Records minted after that keep sorting after the import too.
    b.record_comment(
        &id,
        CommentRequest {
            author: "bob",
            body: "after the import",
            anchor: None,
        },
    )
    .unwrap();
    let comments = b.detail(&id).unwrap().comments;
    assert!(sequence_of(&comments.last().unwrap().record_id) > imported_sequence);
}

/// Received content that expired and is fetched again gets a new clock: it
/// is kept for the policy's full period from the refetch, not collected at
/// once because its version records still carry the first receipt.
#[test]
fn refetched_content_restarts_its_retention_clock() {
    let sharing = Sharing::new("refetch-restarts-clock");
    sharing
        .fixture
        .write("workspace/theirs.md", b"their report");
    let a = sharing.home("a");
    let id = sharing.publish(&a, "theirs.md", "task-a", ArtifactRetention::Keep);
    push(&a, &sharing.remote, &id).unwrap();
    let nobody = lifecycles(&[]);
    let policy = ArtifactRetention::ThirtyDays;

    let b = receive(&sharing, "b", &id, at(T0));
    let sweep = b
        .sweep_retention(at(T0) + THIRTY_DAYS, policy, &nobody)
        .unwrap();
    assert_eq!(sweep.expired, vec![id.clone()]);
    let detail = b.detail(&id).unwrap();
    assert!(!detail.retained && detail.expired, "{detail:?}");
    assert_eq!(detail.versions.len(), 1);

    // Fetched again a day later.
    let refetched = at(T0) + THIRTY_DAYS + Duration::from_secs(86_400);
    set_test_clock(Some(refetched));
    let fetched = fetch(&b, &sharing.remote, &id);
    set_test_clock(None);
    assert_eq!(fetched.unwrap().content_retained, vec![id.clone()]);
    let sweep = b
        .sweep_retention(refetched + Duration::from_secs(1), policy, &nobody)
        .unwrap();
    assert!(
        sweep.expired.is_empty(),
        "refetched content was collected at once"
    );
    let sweep = b
        .sweep_retention(
            refetched + THIRTY_DAYS - Duration::from_secs(1),
            policy,
            &nobody,
        )
        .unwrap();
    assert!(sweep.expired.is_empty());
    assert!(b.detail(&id).unwrap().retained);

    let sweep = b
        .sweep_retention(refetched + THIRTY_DAYS, policy, &nobody)
        .unwrap();
    assert_eq!(sweep.expired, vec![id.clone()]);
    let detail = b.detail(&id).unwrap();
    assert!(!detail.retained && detail.expired);
}
