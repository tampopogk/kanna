//! Result binding, retention and record order (T6b).

use super::store::{
    is_dot_git, set_test_clock, ArtifactStore, BindingRequest, PublishLimits, PublishRequest,
    TaskLifecycle, DISCARD_ON_CLOSE_GRACE, THIRTY_DAYS,
};
use super::tests::Fixture;
use super::types::{ArtifactContentKind, ArtifactReference, ArtifactRetention};
use super::ArtifactError;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLOSED_AT: u64 = 1_800_000_000;

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn store(fixture: &Fixture) -> ArtifactStore {
    ArtifactStore::open_or_create(&fixture.root.join("artifacts.git"), "repo-test").unwrap()
}

fn publish_as(
    store: &ArtifactStore,
    fixture: &Fixture,
    source: &str,
    task_id: &str,
    retention: ArtifactRetention,
) -> String {
    store
        .publish(PublishRequest {
            task_id,
            workspace_root: &fixture.root.join("workspace"),
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

/// Task lifecycles by id; anything not listed is unknown.
fn lifecycles(entries: &[(&str, TaskLifecycle)]) -> impl Fn(&str) -> TaskLifecycle {
    let map = entries
        .iter()
        .map(|(id, lifecycle)| (id.to_string(), *lifecycle))
        .collect::<HashMap<_, _>>();
    move |id: &str| map.get(id).copied().unwrap_or(TaskLifecycle::Unknown)
}

fn closed() -> TaskLifecycle {
    TaskLifecycle::Closed { at: at(CLOSED_AT) }
}

fn object_exists(repository: &Path, oid: &str) -> bool {
    Command::new("git")
        .arg("--git-dir")
        .arg(repository)
        .args(["cat-file", "-e", oid])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap()
        .success()
}

fn metadata_tip(repository: &Path) -> String {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(repository)
        .args(["rev-parse", "refs/kanna/artifacts/metadata"])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn binding_resolves_every_reference_before_writing_any() {
    let fixture = Fixture::new("binding");
    fixture.write("workspace/plan.md", b"# plan");
    fixture.write("workspace/notes.md", b"# notes");
    let store = store(&fixture);
    let plan = publish_as(
        &store,
        &fixture,
        "plan.md",
        "task-1",
        ArtifactRetention::Keep,
    );
    let repository = fixture.root.join("artifacts.git");
    let before = metadata_tip(&repository);

    // One good reference beside one that was never published: refused
    // whole, nothing written.
    let never = "0123456789abcdef0123456789abcdef01234567";
    let refused = store.bind_to_result(
        "task-2",
        Some("run-1"),
        &[
            BindingRequest {
                name: "plan",
                artifact_id: &plan,
                kind: None,
            },
            BindingRequest {
                name: "ghost",
                artifact_id: never,
                kind: None,
            },
        ],
    );
    assert!(
        matches!(refused, Err(ArtifactError::NotFound { .. })),
        "{refused:?}"
    );
    // A declared kind no version has is refused too.
    let wrong_kind = store.bind_to_result(
        "task-2",
        None,
        &[BindingRequest {
            name: "plan",
            artifact_id: &plan,
            kind: Some(ArtifactContentKind::Mockup),
        }],
    );
    assert!(matches!(wrong_kind, Err(ArtifactError::InvalidRequest(_))));
    assert_eq!(
        metadata_tip(&repository),
        before,
        "a refusal wrote metadata"
    );
    assert!(store.detail(&plan).unwrap().bindings.is_empty());

    let bound = store
        .bind_to_result(
            "task-2",
            Some("run-1"),
            &[BindingRequest {
                name: "plan",
                artifact_id: &plan,
                kind: Some(ArtifactContentKind::Document),
            }],
        )
        .unwrap();
    assert_eq!(
        bound,
        vec![ArtifactReference::Stored {
            repo_id: "repo-test".into(),
            artifact_id: plan.clone(),
            kind: ArtifactContentKind::Document,
        }]
    );
    // Binding the same name again (a retried or corrected result) adds
    // nothing.
    store
        .bind_to_result(
            "task-2",
            Some("run-2"),
            &[BindingRequest {
                name: "plan",
                artifact_id: &plan,
                kind: None,
            }],
        )
        .unwrap();
    let bindings = store.detail(&plan).unwrap().bindings;
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].task_id, "task-2");
    assert_eq!(bindings[0].name, "plan");
    assert_eq!(bindings[0].run_id.as_deref(), Some("run-1"));
}

#[test]
fn retention_keeps_or_collects_by_policy_and_injected_clock() {
    let fixture = Fixture::new("retention-policy");
    fixture.write("workspace/keep.md", b"keep");
    fixture.write("workspace/month.md", b"month");
    fixture.write("workspace/discard.md", b"discard");
    let store = store(&fixture);
    let keep = publish_as(
        &store,
        &fixture,
        "keep.md",
        "task-k",
        ArtifactRetention::Keep,
    );
    let month = publish_as(
        &store,
        &fixture,
        "month.md",
        "task-m",
        ArtifactRetention::ThirtyDays,
    );
    let discard = publish_as(
        &store,
        &fixture,
        "discard.md",
        "task-d",
        ArtifactRetention::DiscardOnClose,
    );
    let retained = |id: &str| store.detail(id).unwrap().retained;

    // Every producer still open: nothing goes, however late it is.
    let open = lifecycles(&[
        ("task-k", TaskLifecycle::Open),
        ("task-m", TaskLifecycle::Open),
        ("task-d", TaskLifecycle::Open),
    ]);
    let sweep = store
        .sweep_retention(
            at(CLOSED_AT) + THIRTY_DAYS * 10,
            ArtifactRetention::Keep,
            &open,
        )
        .unwrap();
    assert!(sweep.expired.is_empty());

    let all_closed = lifecycles(&[
        ("task-k", closed()),
        ("task-m", closed()),
        ("task-d", closed()),
    ]);
    // Inside the discard grace: nothing yet.
    let sweep = store
        .sweep_retention(
            at(CLOSED_AT) + DISCARD_ON_CLOSE_GRACE - Duration::from_secs(1),
            ArtifactRetention::Keep,
            &all_closed,
        )
        .unwrap();
    assert!(sweep.expired.is_empty(), "{sweep:?}");

    // Past the grace: discard-on-close goes, 30-days waits.
    let sweep = store
        .sweep_retention(
            at(CLOSED_AT) + DISCARD_ON_CLOSE_GRACE,
            ArtifactRetention::Keep,
            &all_closed,
        )
        .unwrap();
    assert_eq!(sweep.expired, vec![discard.clone()]);
    assert_eq!(sweep.prune_error, None);
    assert!(!retained(&discard) && retained(&month) && retained(&keep));

    // A day short of 30 days after the close: 30-days still waits.
    let sweep = store
        .sweep_retention(
            at(CLOSED_AT) + THIRTY_DAYS - Duration::from_secs(86_400),
            ArtifactRetention::Keep,
            &all_closed,
        )
        .unwrap();
    assert!(sweep.expired.is_empty());
    let sweep = store
        .sweep_retention(
            at(CLOSED_AT) + THIRTY_DAYS,
            ArtifactRetention::Keep,
            &all_closed,
        )
        .unwrap();
    assert_eq!(sweep.expired, vec![month.clone()]);

    // Keep never goes.
    let sweep = store
        .sweep_retention(
            at(CLOSED_AT) + THIRTY_DAYS * 100,
            ArtifactRetention::Keep,
            &all_closed,
        )
        .unwrap();
    assert!(sweep.expired.is_empty());
    assert!(retained(&keep));

    // Git pruned the collected trees; the kept one is intact.
    let repository = fixture.root.join("artifacts.git");
    assert!(!object_exists(&repository, &discard));
    assert!(!object_exists(&repository, &month));
    assert!(object_exists(&repository, &keep));
}

#[test]
fn a_task_that_is_unknown_here_keeps_its_content() {
    let fixture = Fixture::new("retention-unknown");
    fixture.write("workspace/a.md", b"a");
    let store = store(&fixture);
    let id = publish_as(
        &store,
        &fixture,
        "a.md",
        "task-elsewhere",
        ArtifactRetention::DiscardOnClose,
    );
    let sweep = store
        .sweep_retention(
            at(CLOSED_AT) + THIRTY_DAYS * 10,
            ArtifactRetention::Keep,
            &lifecycles(&[]),
        )
        .unwrap();
    assert!(sweep.expired.is_empty());
    assert!(store.detail(&id).unwrap().retained);
}

#[test]
fn shared_content_stays_while_another_open_task_references_it() {
    let fixture = Fixture::new("retention-shared");
    fixture.write("workspace/shared.md", b"shared bytes");
    fixture.write("workspace/copy/shared.md", b"shared bytes");
    let store = store(&fixture);
    let shared = publish_as(
        &store,
        &fixture,
        "shared.md",
        "task-a",
        ArtifactRetention::DiscardOnClose,
    );
    let late = at(CLOSED_AT) + THIRTY_DAYS * 2;

    // A result of another, open task named it.
    store
        .bind_to_result(
            "task-b",
            Some("run-b"),
            &[BindingRequest {
                name: "reviewed",
                artifact_id: &shared,
                kind: None,
            }],
        )
        .unwrap();
    let sweep = store
        .sweep_retention(
            late,
            ArtifactRetention::Keep,
            &lifecycles(&[("task-a", closed()), ("task-b", TaskLifecycle::Open)]),
        )
        .unwrap();
    assert!(
        sweep.expired.is_empty(),
        "collected content an open task named"
    );

    // Another open task published the identical tree itself.
    let same = publish_as(
        &store,
        &fixture,
        "copy/shared.md",
        "task-c",
        ArtifactRetention::DiscardOnClose,
    );
    assert_eq!(same, shared);
    let sweep = store
        .sweep_retention(
            late,
            ArtifactRetention::Keep,
            &lifecycles(&[
                ("task-a", closed()),
                ("task-b", closed()),
                ("task-c", TaskLifecycle::Open),
            ]),
        )
        .unwrap();
    assert!(
        sweep.expired.is_empty(),
        "collected content an open task produced"
    );

    // Once nobody open holds it, it goes.
    let sweep = store
        .sweep_retention(
            late,
            ArtifactRetention::Keep,
            &lifecycles(&[
                ("task-a", closed()),
                ("task-b", closed()),
                ("task-c", closed()),
            ]),
        )
        .unwrap();
    assert_eq!(sweep.expired, vec![shared]);
}

#[test]
fn an_expired_artifact_keeps_its_records_and_reads_as_expired() {
    let fixture = Fixture::new("retention-expired");
    fixture.write("workspace/mock/index.html", b"<p>mock</p>");
    let store = store(&fixture);
    let published = store
        .publish(PublishRequest {
            task_id: "task-1",
            workspace_root: &fixture.root.join("workspace"),
            source_path: "mock",
            kind: ArtifactContentKind::Mockup,
            entrypoint: None,
            previous: None,
            retention: ArtifactRetention::DiscardOnClose,
            limits: PublishLimits::default(),
        })
        .unwrap();
    let id = published.artifact_id;
    store
        .record_comment(
            &id,
            super::store::CommentRequest {
                author: "reviewer",
                body: "tighten the header",
                anchor: None,
            },
        )
        .unwrap();
    let before = store.detail(&id).unwrap();
    assert!(before.retained && !before.expired && before.expirations.is_empty());

    let sweep = store
        .sweep_retention(
            at(CLOSED_AT) + DISCARD_ON_CLOSE_GRACE,
            ArtifactRetention::Keep,
            &lifecycles(&[("task-1", closed())]),
        )
        .unwrap();
    assert_eq!(sweep.expired, vec![id.clone()]);

    // Reopened from disk, as a later reader would.
    let reopened = ArtifactStore::open_existing(&fixture.root.join("artifacts.git"), "repo-test")
        .unwrap()
        .unwrap();
    let detail = reopened.detail(&id).unwrap();
    assert!(!detail.retained);
    assert!(detail.expired, "an expired descriptor must read as expired");
    assert!(detail.files.is_empty());
    assert_eq!(detail.versions, before.versions);
    assert_eq!(detail.comments, before.comments);
    assert_eq!(detail.expirations.len(), 1);
    let expiry = &detail.expirations[0];
    assert_eq!(expiry.policies, vec![ArtifactRetention::DiscardOnClose]);
    assert_eq!(expiry.storage, before.versions[0].storage);
    assert!(matches!(
        reopened.entrypoint(&id),
        Err(ArtifactError::ContentMissing { .. })
    ));
    assert!(matches!(
        reopened.read_file(&id, "index.html"),
        Err(ArtifactError::ContentMissing { .. })
    ));
    // A result can no longer name it.
    assert!(matches!(
        reopened.bind_to_result(
            "task-2",
            None,
            &[BindingRequest {
                name: "mock",
                artifact_id: &id,
                kind: None,
            }],
        ),
        Err(ArtifactError::ContentMissing { .. })
    ));
    // A second sweep finds nothing further to do.
    let sweep = reopened
        .sweep_retention(
            at(CLOSED_AT) + THIRTY_DAYS,
            ArtifactRetention::Keep,
            &lifecycles(&[("task-1", closed())]),
        )
        .unwrap();
    assert!(sweep.expired.is_empty());
    assert_eq!(reopened.detail(&id).unwrap().expirations.len(), 1);
}

#[test]
fn version_order_follows_the_persisted_sequence_not_the_clock() {
    let fixture = Fixture::new("sequence-order");
    fixture.write("workspace/site/index.html", b"<p>one</p>");
    fixture.write("workspace/site/other.html", b"<p>two</p>");
    let store = store(&fixture);
    let publish_with_entry = |entrypoint: &str| {
        store
            .publish(PublishRequest {
                task_id: "task-1",
                workspace_root: &fixture.root.join("workspace"),
                source_path: "site",
                kind: ArtifactContentKind::Mockup,
                entrypoint: Some(entrypoint),
                previous: None,
                retention: ArtifactRetention::Keep,
                limits: PublishLimits::default(),
            })
            .unwrap()
    };

    // The clock runs backwards between publications, then stands still.
    set_test_clock(Some(at(2_000_000_000)));
    let first = publish_with_entry("index.html");
    set_test_clock(Some(at(1_000_000_000)));
    let second = publish_with_entry("other.html");
    let third = publish_with_entry("index.html");
    let fourth = publish_with_entry("other.html");
    set_test_clock(None);

    let id = first.artifact_id.clone();
    let order = store
        .detail(&id)
        .unwrap()
        .versions
        .into_iter()
        .map(|version| version.record_id)
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        vec![
            first.version.record_id,
            second.version.record_id,
            third.version.record_id,
            fourth.version.record_id.clone(),
        ]
    );
    assert!(fourth.version.record_id.starts_with("s000000000004-"));
    // The newest publication decides what opening it shows.
    assert_eq!(
        store.entrypoint(&id).unwrap().as_deref(),
        Some("other.html")
    );
}

#[test]
fn records_from_before_the_sequence_read_first_in_timestamp_order() {
    // Written by T6 increment 1 (`<millis>-<random>`), then sequenced ones.
    let mut names = [
        "s000000000002-aa.json",
        "1790000000500-ff.json",
        "s000000000001-bb.json",
        "1790000000100-00.json",
    ];
    names.sort_by_key(|name| super::store::record_order_for_tests(name));
    assert_eq!(
        names,
        [
            "1790000000100-00.json",
            "1790000000500-ff.json",
            "s000000000001-bb.json",
            "s000000000002-aa.json",
        ]
    );
}

#[test]
fn every_spelling_of_git_a_file_system_may_resolve_is_refused() {
    for name in [
        ".git",
        ".GIT",
        ".Git",
        ".gIt",
        ".g\u{200c}it",
        "\u{feff}.git",
    ] {
        assert!(is_dot_git(name), "{name:?}");
    }
    for name in [".github", "git", ".gitignore", "..git", ".git.bak"] {
        assert!(!is_dot_git(name), "{name:?}");
    }
}

/// On a case-insensitive volume (the macOS default) `.GIT` opens the real
/// `.git`. Both the requested path and a recursive entry are refused, and on
/// such a volume the test proves the alias really resolves.
#[test]
fn case_aliases_of_git_are_refused_on_a_case_insensitive_file_system() {
    let fixture = Fixture::new("dot-git-case");
    fixture.write("workspace/repo/index.html", b"x");
    fixture.write("workspace/repo/.git/config", b"[core]\n\tsecret = yes\n");
    let workspace = fixture.root.join("workspace");
    let case_insensitive = workspace.join("repo/.GIT/config").exists();
    eprintln!("workspace volume is case-insensitive: {case_insensitive}");
    let store = store(&fixture);
    let refused = |source: &str| {
        let result = store.publish(PublishRequest {
            task_id: "task-1",
            workspace_root: &workspace,
            source_path: source,
            kind: ArtifactContentKind::Document,
            entrypoint: None,
            previous: None,
            retention: ArtifactRetention::Keep,
            limits: PublishLimits::default(),
        });
        assert!(
            matches!(result, Err(ArtifactError::InvalidPath(_))),
            "{source}: {result:?}"
        );
    };
    refused("repo/.GIT");
    refused("repo/.GIT/config");
    refused("repo/.Git/config");
    refused("repo");

    // A directory whose own entry is spelled `.GIT` is refused while
    // walking it, whatever the volume.
    fixture.write("workspace/upper/index.html", b"x");
    fixture.write("workspace/upper/.GIT/HEAD", b"ref: refs/heads/main");
    refused("upper");
    fixture.write("workspace/nested/a/b/.Git/HEAD", b"ref: refs/heads/main");
    fixture.write("workspace/nested/a/index.html", b"x");
    refused("nested");
}
