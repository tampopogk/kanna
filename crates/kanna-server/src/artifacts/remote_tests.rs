//! Two independent artifact repositories sharing through one local bare
//! remote. Everything lives under this worktree's `.tmp/`.

use super::remote::{fetch, push, run_git_bounded, ArtifactRemote};
use super::store::{ArtifactStore, CommentRequest, DecisionRequest};
use super::tests::{git, publish, write_mockup, Fixture};
use super::types::{
    ArtifactComment, ArtifactContentKind, ArtifactProducer, ArtifactRetention, ArtifactStorage,
    ArtifactVersion, ARTIFACT_RECORD_SCHEMA_VERSION,
};
use super::ArtifactError;
use git2::{Oid, Repository, Signature, Time};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SENTINEL: &str = "KANNA-SHARING-SENTINEL-d41c";

struct Homes {
    fixture: Fixture,
    remote_path: PathBuf,
    remote: ArtifactRemote,
    a: ArtifactStore,
    b: ArtifactStore,
}

impl Homes {
    fn new(label: &str) -> Self {
        let fixture = Fixture::new(label);
        git(&fixture.root, &["init", "--bare", "--quiet", "remote.git"]);
        let remote_path = fixture.root.join("remote.git");
        let a =
            ArtifactStore::open_or_create(&fixture.root.join("a/artifacts.git"), "repo-a").unwrap();
        let b =
            ArtifactStore::open_or_create(&fixture.root.join("b/artifacts.git"), "repo-b").unwrap();
        let remote = ArtifactRemote::parse(
            remote_path.to_str().unwrap(),
            &fixture.root,
            &fixture.root.join("a/artifacts.git"),
        )
        .unwrap();
        // Things that sit next to the published directories in A's workspace
        // and must never reach the remote.
        fixture.write(
            "workspace/secrets.env",
            format!("TOKEN={SENTINEL}").as_bytes(),
        );
        fixture.write(
            "workspace/.kanna/transcript.jsonl",
            format!("{{\"text\":\"{SENTINEL}\"}}").as_bytes(),
        );
        Self {
            fixture,
            remote_path,
            remote,
            a,
            b,
        }
    }

    fn workspace(&self) -> PathBuf {
        self.fixture.root.join("workspace")
    }

    /// A publishes v1, then v2 with `previous` = v1.
    fn publish_two_versions(&self) -> (String, String) {
        write_mockup(&self.fixture, "v1", "body{color:#111}");
        write_mockup(&self.fixture, "v2", "body{color:#222}");
        let v1 = publish(
            &self.a,
            &self.workspace(),
            "v1",
            ArtifactContentKind::Mockup,
            None,
        )
        .unwrap();
        let v2 = publish(
            &self.a,
            &self.workspace(),
            "v2",
            ArtifactContentKind::Mockup,
            Some(&v1.artifact_id),
        )
        .unwrap();
        (v1.artifact_id, v2.artifact_id)
    }

    fn remote_git(&self, args: &[&str]) -> String {
        git(&self.remote_path, args)
    }
}

fn comment(store: &ArtifactStore, id: &str, author: &str, body: &str) -> String {
    store
        .record_comment(
            id,
            CommentRequest {
                author,
                body,
                anchor: None,
            },
        )
        .unwrap()
        .record_id
}

fn decide(store: &ArtifactStore, id: &str, who: &str, what: &str) -> String {
    store
        .record_decision(id, DecisionRequest { who, what })
        .unwrap()
        .record_id
}

#[test]
fn a_receiver_holding_only_the_tree_id_fetches_identical_bytes_and_history() {
    let homes = Homes::new("remote-roundtrip");
    let (v1, v2) = homes.publish_two_versions();
    let a_comment = comment(&homes.a, &v1, "designer", "logo too small");

    let pushed = push(&homes.a, &homes.remote, &v2).unwrap();
    assert_eq!(pushed.artifact_ids, [v2.clone(), v1.clone()]);
    // v1 and v2 content, two version records and one comment.
    assert_eq!(pushed.created_refs.len(), 5, "{:?}", pushed.created_refs);
    assert_eq!(
        push(&homes.a, &homes.remote, &v2)
            .unwrap()
            .created_refs
            .len(),
        0,
        "a second push changes nothing on the remote"
    );

    // B has never seen either id; the tree id is all it is told.
    assert!(matches!(
        homes.b.detail(&v2),
        Err(ArtifactError::NotFound { .. })
    ));
    let fetched = fetch(&homes.b, &homes.remote, &v2).unwrap();
    assert_eq!(fetched.fetched, [v2.clone(), v1.clone()]);
    assert_eq!(fetched.content_retained, [v2.clone(), v1.clone()]);
    assert_eq!(fetched.records_imported, 3);
    assert!(fetched.refused.is_empty() && fetched.missing.is_empty());

    let detail = homes.b.detail(&v2).unwrap();
    assert_eq!(detail.artifact_id, v2, "same tree id at the receiver");
    assert!(detail.retained);
    assert_eq!(detail.versions[0].previous.as_deref(), Some(v1.as_str()));
    assert_eq!(
        detail
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["css/site.css", "img/logo.png", "index.html"]
    );
    // Identical bytes, relative assets included, and the entrypoint's
    // relative references resolve inside the same tree.
    for (path, expected) in [
        (
            "index.html",
            b"<link rel=stylesheet href=css/site.css><img src=img/logo.png>".as_slice(),
        ),
        ("css/site.css", b"body{color:#222}".as_slice()),
        (
            "img/logo.png",
            [0x89, b'P', b'N', b'G', 0, 1, 2, 255].as_slice(),
        ),
    ] {
        assert_eq!(homes.b.read_file(&v2, path).unwrap().bytes, expected);
        assert_eq!(homes.a.read_file(&v2, path).unwrap().bytes, expected);
    }
    assert_eq!(
        homes.b.entrypoint(&v2).unwrap().as_deref(),
        Some("index.html")
    );
    let receiver_tree = git(
        &homes.fixture.root.join("b/artifacts.git"),
        &[
            "rev-parse",
            &format!("refs/kanna/artifacts/trees/{v2}^{{tree}}"),
        ],
    );
    assert_eq!(receiver_tree.trim(), v2);

    // The previous version came along with its comment, still bound to it.
    let older = homes.b.detail(&v1).unwrap();
    assert_eq!(older.comments.len(), 1);
    assert_eq!(older.comments[0].record_id, a_comment);
    assert_eq!(older.comments[0].about_artifact_id, v1);
    assert_eq!(
        homes.b.read_file(&v1, "css/site.css").unwrap().bytes,
        b"body{color:#111}"
    );

    // Refetching imports nothing new and leaves no private refs behind.
    assert_eq!(
        fetch(&homes.b, &homes.remote, &v2)
            .unwrap()
            .records_imported,
        0
    );
    let leftovers = git(
        &homes.fixture.root.join("b/artifacts.git"),
        &["for-each-ref", "refs/kanna/artifacts/incoming/"],
    );
    assert_eq!(leftovers, "");
}

#[test]
fn concurrent_records_from_both_homes_survive_either_push_and_fetch_order() {
    for a_pushes_first in [true, false] {
        let homes = Homes::new(if a_pushes_first {
            "remote-order-ab"
        } else {
            "remote-order-ba"
        });
        let (v1, v2) = homes.publish_two_versions();
        push(&homes.a, &homes.remote, &v2).unwrap();
        fetch(&homes.b, &homes.remote, &v2).unwrap();

        // Both homes annotate without talking to each other.
        let a_comment = comment(&homes.a, &v2, "alice", "spacing is off");
        let a_decision = decide(&homes.a, &v2, "alice", "approve v2");
        let b_comment = comment(&homes.b, &v2, "bob", "colour is wrong");
        let b_decision = decide(&homes.b, &v1, "bob", "reject v1");

        let (first, second) = if a_pushes_first {
            (&homes.a, &homes.b)
        } else {
            (&homes.b, &homes.a)
        };
        push(first, &homes.remote, &v2).unwrap();
        push(second, &homes.remote, &v2).unwrap();
        fetch(second, &homes.remote, &v2).unwrap();
        fetch(first, &homes.remote, &v2).unwrap();

        for store in [&homes.a, &homes.b] {
            let newer = store.detail(&v2).unwrap();
            let comments = newer
                .comments
                .iter()
                .map(|comment| comment.record_id.clone())
                .collect::<BTreeSet<_>>();
            assert_eq!(
                comments,
                BTreeSet::from([a_comment.clone(), b_comment.clone()])
            );
            assert_eq!(newer.decisions.len(), 1);
            assert_eq!(newer.decisions[0].record_id, a_decision);
            // The decision B made about the older revision stays bound to it.
            let older = store.detail(&v1).unwrap();
            assert_eq!(older.decisions.len(), 1);
            assert_eq!(older.decisions[0].record_id, b_decision);
            assert_eq!(older.decisions[0].about_artifact_id, v1);
            assert_eq!(older.decisions[0].what, "reject v1");
        }
        let records = homes.remote_git(&[
            "for-each-ref",
            "--format=%(refname)",
            "refs/kanna/artifacts/shared/records/",
        ]);
        // Two versions, two comments, two decisions: one immutable name each.
        assert_eq!(records.lines().count(), 6, "{records}");
    }
}

#[test]
fn identical_bytes_published_independently_share_one_id_without_conflict() {
    let homes = Homes::new("remote-identical");
    write_mockup(&homes.fixture, "same", "body{}");
    let from_a = publish(
        &homes.a,
        &homes.workspace(),
        "same",
        ArtifactContentKind::Mockup,
        None,
    )
    .unwrap();
    let from_b = publish(
        &homes.b,
        &homes.workspace(),
        "same",
        ArtifactContentKind::Document,
        None,
    )
    .unwrap();
    assert_eq!(from_a.artifact_id, from_b.artifact_id);
    assert_ne!(from_a.version.storage.commit, from_b.version.storage.commit);

    push(&homes.a, &homes.remote, &from_a.artifact_id).unwrap();
    push(&homes.b, &homes.remote, &from_b.artifact_id).unwrap();
    let content = homes.remote_git(&[
        "for-each-ref",
        "--format=%(refname)",
        &format!(
            "refs/kanna/artifacts/shared/content/{}/",
            from_a.artifact_id
        ),
    ]);
    assert_eq!(
        content.lines().count(),
        2,
        "both retaining commits: {content}"
    );

    let fetched = fetch(&homes.a, &homes.remote, &from_a.artifact_id).unwrap();
    assert!(
        fetched.content_retained.is_empty(),
        "A keeps its own commit"
    );
    assert_eq!(fetched.detail.versions.len(), 2);
}

#[test]
fn fetching_an_id_the_remote_does_not_hold_reports_it_missing() {
    let homes = Homes::new("remote-missing");
    let (_, v2) = homes.publish_two_versions();
    push(&homes.a, &homes.remote, &v2).unwrap();

    let unknown = "0123456789abcdef0123456789abcdef01234567";
    let error = fetch(&homes.b, &homes.remote, unknown).unwrap_err();
    assert_eq!(error.code(), "artifact_not_on_remote");
    assert!(error.to_string().contains("missing"), "{error}");
    assert!(error.to_string().contains(unknown), "{error}");
    assert!(matches!(
        homes.b.detail(unknown),
        Err(ArtifactError::NotFound { .. })
    ));

    // An abbreviation is refused before anything is contacted.
    assert!(matches!(
        fetch(&homes.b, &homes.remote, &v2[..12]),
        Err(ArtifactError::InvalidId(_))
    ));
    // Pushing something this home never published is a local not-found.
    assert!(matches!(
        push(&homes.b, &homes.remote, unknown),
        Err(ArtifactError::NotFound { .. })
    ));

    // An unreachable remote is a remote failure, not a missing artifact.
    let gone = ArtifactRemote::parse(
        homes.fixture.root.join("nowhere.git").to_str().unwrap(),
        &homes.fixture.root,
        &homes.fixture.root.join("b/artifacts.git"),
    )
    .unwrap();
    assert_eq!(
        fetch(&homes.b, &gone, &v2).unwrap_err().code(),
        "artifact_remote_failed"
    );
}

/// Assert the remote's whole inventory: which refs exist, that every object
/// in it is reachable from them, and what each blob is.
#[test]
fn the_remote_holds_only_artifact_content_and_its_records() {
    let homes = Homes::new("remote-inventory");
    let (v1, v2) = homes.publish_two_versions();
    comment(&homes.a, &v2, "alice", "ok");
    push(&homes.a, &homes.remote, &v2).unwrap();
    fetch(&homes.b, &homes.remote, &v2).unwrap();
    decide(&homes.b, &v1, "bob", "reject");
    push(&homes.b, &homes.remote, &v2).unwrap();

    let refs = homes.remote_git(&["for-each-ref", "--format=%(refname) %(objecttype)"]);
    let mut content_refs = 0;
    for line in refs.lines() {
        let (name, kind) = line.split_once(' ').unwrap();
        assert_eq!(kind, "commit", "{line}");
        if let Some(rest) = name.strip_prefix("refs/kanna/artifacts/shared/content/") {
            let (tree, _) = rest.split_once('/').unwrap();
            assert!(tree == v1 || tree == v2, "{line}");
            content_refs += 1;
        } else {
            assert!(
                name.starts_with("refs/kanna/artifacts/shared/records/"),
                "unexpected ref on the remote: {line}"
            );
        }
    }
    assert_eq!(content_refs, 2);
    assert_eq!(refs.lines().count(), 2 + 2 + 1 + 1, "{refs}");

    let all_objects = homes
        .remote_git(&[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname)",
        ])
        .lines()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let reachable = homes
        .remote_git(&["rev-list", "--objects", "--all"])
        .lines()
        .map(|line| line.split(' ').next().unwrap().to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(all_objects, reachable, "nothing unreachable was sent");

    let remote = Repository::open_bare(&homes.remote_path).unwrap();
    let payload: [&[u8]; 4] = [
        b"<link rel=stylesheet href=css/site.css><img src=img/logo.png>",
        b"body{color:#111}",
        b"body{color:#222}",
        &[0x89, b'P', b'N', b'G', 0, 1, 2, 255],
    ];
    let mut records = 0;
    for id in &all_objects {
        let object = remote
            .find_object(Oid::from_str(id).unwrap(), None)
            .unwrap();
        match object.kind() {
            Some(git2::ObjectType::Blob) => {
                let bytes = object.as_blob().unwrap().content();
                assert!(
                    !String::from_utf8_lossy(bytes).contains(SENTINEL),
                    "workspace secrets reached the remote"
                );
                if payload.contains(&bytes) {
                    continue;
                }
                let record: serde_json::Value = serde_json::from_slice(bytes)
                    .unwrap_or_else(|_| panic!("blob {id} is neither payload nor a record"));
                assert_eq!(record["schemaVersion"], ARTIFACT_RECORD_SCHEMA_VERSION);
                records += 1;
            }
            Some(git2::ObjectType::Commit) => {
                let commit = object.as_commit().unwrap();
                assert_eq!(commit.parent_count(), 0);
                assert_eq!(commit.author().name(), Some("Kanna"));
            }
            Some(git2::ObjectType::Tree) => {}
            other => panic!("unexpected object {id}: {other:?}"),
        }
    }
    assert_eq!(records, 2 + 1 + 1);
}

/// Objects written straight into the remote by someone who is not Kanna.
struct Forger {
    repository: Repository,
    remote: PathBuf,
}

impl Forger {
    fn new(root: &Path, remote: &Path) -> Self {
        Self {
            repository: Repository::init_bare(root.join("forger.git")).unwrap(),
            remote: remote.to_path_buf(),
        }
    }

    fn commit(&self, entries: &[(&str, &[u8], i32)], time: i64) -> Oid {
        let mut builder = self.repository.treebuilder(None).unwrap();
        for (name, bytes, mode) in entries {
            let blob = self.repository.blob(bytes).unwrap();
            builder.insert(name, blob, *mode).unwrap();
        }
        let tree = self.repository.find_tree(builder.write().unwrap()).unwrap();
        let signature = Signature::new("Kanna", "kanna@localhost", &Time::new(time, 0)).unwrap();
        self.repository
            .commit(None, &signature, &signature, "forged", &tree, &[])
            .unwrap()
    }

    /// A record commit exactly as Kanna builds one, so it passes the
    /// canonical-commit check: only its content is forged.
    fn canonical_record(&self, directory: &str, about: &str, record_id: &str, bytes: &[u8]) -> Oid {
        let mut builder = self.repository.treebuilder(None).unwrap();
        let blob = self.repository.blob(bytes).unwrap();
        builder.insert("record.json", blob, 0o100_644).unwrap();
        let tree = self.repository.find_tree(builder.write().unwrap()).unwrap();
        let signature = Signature::new("Kanna", "kanna@localhost", &Time::new(0, 0)).unwrap();
        self.repository
            .commit(
                None,
                &signature,
                &signature,
                &format!("kanna artifact {directory} {about} {record_id}\n"),
                &tree,
                &[],
            )
            .unwrap()
    }

    fn push(&self, commit: Oid, name: &str) {
        git(
            self.repository.path(),
            &[
                "push",
                "--quiet",
                self.remote.to_str().unwrap(),
                &format!("{commit}:{name}"),
            ],
        );
    }
}

fn comment_json(about: &str, record_id: &str, body: &str) -> Vec<u8> {
    serde_json::to_vec_pretty(&ArtifactComment {
        schema_version: ARTIFACT_RECORD_SCHEMA_VERSION,
        record_id: record_id.to_string(),
        repo_id: "repo-forger".to_string(),
        about_artifact_id: about.to_string(),
        created_at: "2026-09-23T00:00:00.000Z".to_string(),
        author: "mallory".to_string(),
        body: body.to_string(),
        anchor: None,
    })
    .unwrap()
}

#[test]
fn malformed_remote_refs_are_refused_and_immutable_names_are_never_overwritten() {
    let homes = Homes::new("remote-hostile");
    let (_, v2) = homes.publish_two_versions();
    let a_comment = comment(&homes.a, &v2, "alice", "the real comment");
    let forger = Forger::new(&homes.fixture.root, &homes.remote_path);

    // Someone else already wrote different bytes under the name A's comment
    // will take. A's push must not overwrite it, and says so.
    let squatted = format!("refs/kanna/artifacts/shared/records/{v2}/comments/{a_comment}");
    forger.push(
        forger.commit(
            &[(
                "record.json",
                &comment_json(&v2, &a_comment, "forged"),
                0o100_644,
            )],
            0,
        ),
        &squatted,
    );
    let error = push(&homes.a, &homes.remote, &v2).unwrap_err();
    match &error {
        ArtifactError::RemoteConflict { refs, .. } => {
            assert_eq!(refs.len(), 1, "{refs:?}");
            assert!(refs[0].starts_with(&squatted), "{refs:?}");
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    // Everything else still went: pushing one name never blocks another.
    let content = homes.remote_git(&[
        "for-each-ref",
        "--format=%(refname)",
        &format!("refs/kanna/artifacts/shared/content/{v2}/"),
    ]);
    assert_eq!(content.lines().count(), 1);

    // More forgeries about v2: a record whose body names another artifact,
    // a record commit that is not canonical, and a malformed name.
    let other_id = "0123456789abcdef0123456789abcdef01234567";
    let wrong_about = "1790000000000-00000000000000aa";
    forger.push(
        forger.commit(
            &[(
                "record.json",
                &comment_json(other_id, wrong_about, "about something else"),
                0o100_644,
            )],
            0,
        ),
        &format!("refs/kanna/artifacts/shared/records/{v2}/comments/{wrong_about}"),
    );
    let not_canonical = "1790000000000-00000000000000bb";
    forger.push(
        forger.commit(
            &[(
                "record.json",
                &comment_json(&v2, not_canonical, "signed now"),
                0o100_644,
            )],
            1_790_000_000,
        ),
        &format!("refs/kanna/artifacts/shared/records/{v2}/comments/{not_canonical}"),
    );
    forger.push(
        forger.commit(&[("record.json", b"{}", 0o100_644)], 0),
        &format!(
            "refs/kanna/artifacts/shared/records/{v2}/verdicts/1790000000000-00000000000000cc"
        ),
    );

    let fetched = fetch(&homes.b, &homes.remote, &v2).unwrap();
    let refused = fetched
        .refused
        .iter()
        .map(|refusal| (refusal.ref_name.as_str(), refusal.reason.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(refused.len(), 4, "{refused:?}");
    let reason = |suffix: &str| {
        refused
            .iter()
            .find(|(name, _)| name.ends_with(suffix))
            .map(|(_, reason)| *reason)
            .unwrap_or_else(|| panic!("{suffix} not refused: {refused:?}"))
    };
    assert!(reason(&a_comment).contains("canonical"), "{refused:?}");
    assert!(reason(wrong_about).contains("about"), "{refused:?}");
    assert!(reason(not_canonical).contains("canonical"), "{refused:?}");
    assert!(reason("00cc").contains("not a record kind"), "{refused:?}");
    let detail = homes.b.detail(&v2).unwrap();
    assert!(
        detail.comments.is_empty(),
        "no forged comment was imported: {:?}",
        detail.comments
    );
    assert_eq!(detail.versions.len(), 1, "the genuine version was");
}

#[test]
fn content_kanna_could_not_have_published_is_refused() {
    let homes = Homes::new("remote-symlink");
    let forger = Forger::new(&homes.fixture.root, &homes.remote_path);
    let commit = forger.commit(
        &[
            ("index.html", b"<a href=link>x</a>", 0o100_644),
            ("link", b"/etc/passwd", 0o120_000),
        ],
        0,
    );
    let tree = forger
        .repository
        .find_commit(commit)
        .unwrap()
        .tree_id()
        .to_string();
    forger.push(
        commit,
        &format!("refs/kanna/artifacts/shared/content/{tree}/{commit}"),
    );

    let error = fetch(&homes.b, &homes.remote, &tree).unwrap_err();
    assert_eq!(error.code(), "artifact_remote_failed");
    assert!(error.to_string().contains("mode 120000"), "{error}");
    assert!(matches!(
        homes.b.detail(&tree),
        Err(ArtifactError::NotFound { .. })
    ));
    let local = git(
        &homes.fixture.root.join("b/artifacts.git"),
        &["for-each-ref", "--format=%(refname)"],
    );
    assert_eq!(local, "", "nothing was retained or left behind");
}

#[test]
fn remote_configuration_refuses_helpers_options_and_relative_paths() {
    let root = Path::new("/srv/home");
    let store = Path::new("/srv/home/.kanna/repos/r/artifacts.git");
    for refused in [
        "ext::sh -c touch% /tmp/x",
        "fd::3",
        "-uupload-pack=touch /tmp/x",
        "relative/remote.git",
        "ftp://host/remote.git",
        "",
        "/srv/a\nb.git",
    ] {
        let error = ArtifactRemote::parse(refused, root, store).unwrap_err();
        assert_eq!(error.code(), "artifact_remote_invalid", "{refused:?}");
    }
    for (accepted, display) in [
        (
            "git@github.com:team/artifacts.git",
            "git@github.com:team/artifacts.git",
        ),
        (
            "ssh://git@host:2222/srv/a.git",
            "ssh://***@host:2222/srv/a.git",
        ),
        (
            "https://user:ghp_secret@example.com/team/a.git",
            "https://***@example.com/team/a.git",
        ),
        ("~/shared/artifacts.git", "/srv/home/shared/artifacts.git"),
        ("/Volumes/team/artifacts.git", "/Volumes/team/artifacts.git"),
        ("file:///Volumes/team/a.git", "file:///Volumes/team/a.git"),
    ] {
        let remote = ArtifactRemote::parse(accepted, root, store).unwrap();
        assert_eq!(remote.display(), display, "{accepted}");
    }
    let fixture = Fixture::new("remote-self");
    let store = ArtifactStore::open_or_create(&fixture.root.join("artifacts.git"), "r").unwrap();
    drop(store);
    let itself = fixture.root.join("artifacts.git");
    assert_eq!(
        ArtifactRemote::parse(itself.to_str().unwrap(), &fixture.root, &itself)
            .unwrap_err()
            .code(),
        "artifact_remote_invalid"
    );
}

/// A password may contain '/', '@' or only digits. The display must drop all
/// of it; an '@' in a path is redacted too rather than risk echoing a secret,
/// and an scp-style user name stays as written.
#[test]
fn a_password_containing_a_slash_is_redacted_from_the_display() {
    let root = Path::new("/srv/home");
    let store = Path::new("/srv/home/.kanna/repos/r/artifacts.git");
    for (configured, display, secret) in [
        (
            "https://user:pa/ss@host/repo.git",
            "https://***@host/repo.git",
            Some("pa/ss"),
        ),
        (
            "https://user:p/a/s/s@example.com/team/a.git",
            "https://***@example.com/team/a.git",
            Some("p/a/s/s"),
        ),
        (
            "ssh://git:x/y@host:2222/a.git",
            "ssh://***@host:2222/a.git",
            Some("x/y"),
        ),
        (
            "https://user:/secret@host/a.git",
            "https://***@host/a.git",
            Some("secret"),
        ),
        // Digits after the colon are a password here, not a port.
        (
            "https://user:123/secret@host/repo.git",
            "https://***@host/repo.git",
            Some("123/secret"),
        ),
        (
            "https://user:123@host/repo.git",
            "https://***@host/repo.git",
            Some("123"),
        ),
        // A password containing both '/' and '@'.
        (
            "https://user:pa/ss@word@host/repo.git",
            "https://***@host/repo.git",
            Some("word"),
        ),
        // An '@' in a path is indistinguishable from credentials: hidden.
        ("https://host:8080/p@x", "https://***@x", None),
        ("https://host/team/a.git", "https://host/team/a.git", None),
        ("user@host:path/a.git", "user@host:path/a.git", None),
        (
            "git@github.com:team/artifacts.git",
            "git@github.com:team/artifacts.git",
            None,
        ),
    ] {
        let remote = ArtifactRemote::parse(configured, root, store).unwrap();
        assert_eq!(remote.display(), display, "{configured}");
        if let Some(secret) = secret {
            assert!(!remote.display().contains(secret), "{configured}");
            // A refusal names the remote through the same redaction.
            let refused = configured
                .replacen("https://", "ftp://", 1)
                .replacen("ssh://", "ftp://", 1);
            let message = ArtifactRemote::parse(&refused, root, store)
                .unwrap_err()
                .to_string();
            assert!(!message.contains(secret), "{message}");
            assert!(message.contains("ftp://***@"), "{message}");
        }
    }
}

/// A peer with write access plants a well-formed version record for a
/// shared tree whose `previous` names a tree the receiver published but never
/// shared. The receiver's ordinary reply push must not follow that link.
#[test]
fn a_planted_previous_link_never_pulls_an_unshared_tree_into_a_push() {
    let homes = Homes::new("remote-planted-previous");
    write_mockup(&homes.fixture, "shared", "body{}");
    let shared = publish(
        &homes.a,
        &homes.workspace(),
        "shared",
        ArtifactContentKind::Mockup,
        None,
    )
    .unwrap()
    .artifact_id;
    push(&homes.a, &homes.remote, &shared).unwrap();

    // B's private work, never pushed.
    homes
        .fixture
        .write("workspace/private/plan.md", b"B's unshared plan");
    let private = publish(
        &homes.b,
        &homes.workspace(),
        "private",
        ArtifactContentKind::Document,
        None,
    )
    .unwrap()
    .artifact_id;

    let forger = Forger::new(&homes.fixture.root, &homes.remote_path);
    let record_id = "1790000000000-00000000000000dd";
    let planted = serde_json::to_vec_pretty(&ArtifactVersion {
        schema_version: ARTIFACT_RECORD_SCHEMA_VERSION,
        record_id: record_id.to_string(),
        repo_id: "repo-b".to_string(),
        artifact_id: shared.clone(),
        kind: ArtifactContentKind::Mockup,
        entrypoint: Some("index.html".to_string()),
        created_at: "2026-09-23T00:00:00.000Z".to_string(),
        previous: Some(private.clone()),
        retention: ArtifactRetention::Keep,
        produced_by: ArtifactProducer {
            task_id: "task-1".to_string(),
        },
        file_count: 3,
        total_bytes: 1,
        storage: ArtifactStorage {
            commit: shared.clone(),
            ref_name: "refs/kanna/artifacts/trees/x".to_string(),
        },
    })
    .unwrap();
    forger.push(
        forger.canonical_record("versions", &shared, record_id, &planted),
        &format!("refs/kanna/artifacts/shared/records/{shared}/versions/{record_id}"),
    );

    // B receives the shared tree, planted record included, and replies.
    let b_repository = homes.fixture.root.join("b/artifacts.git");
    let metadata_tip = || {
        Oid::from_str(
            git(
                &b_repository,
                &["rev-parse", "refs/kanna/artifacts/metadata"],
            )
            .trim(),
        )
        .unwrap()
    };
    let before_import = metadata_tip();
    let fetched = fetch(&homes.b, &homes.remote, &shared).unwrap();
    let after_import = metadata_tip();
    assert!(
        fetched
            .detail
            .versions
            .iter()
            .any(|version| version.record_id == record_id),
        "the planted record is well formed and imported as data"
    );
    // The planted record and its received mark land in one import commit,
    // so any one snapshot classifies the link the same way: absent before
    // the import, received (never this home's own) after it.
    let shared_oid = Oid::from_str(&shared).unwrap();
    let private_oid = Oid::from_str(&private).unwrap();
    assert!(
        git(
            &b_repository,
            &["rev-list", &format!("{before_import}..{after_import}")]
        )
        .lines()
        .count()
            == 1
    );
    assert_eq!(
        homes
            .b
            .previous_links_at(Some(before_import), shared_oid)
            .unwrap(),
        []
    );
    assert_eq!(
        homes
            .b
            .previous_links_at(Some(after_import), shared_oid)
            .unwrap(),
        [(private_oid, false)]
    );
    assert_eq!(
        homes.b.previous_links_with_provenance(shared_oid).unwrap(),
        [(private_oid, false)]
    );

    comment(&homes.b, &shared, "bob", "looks fine");
    let pushed = push(&homes.b, &homes.remote, &shared).unwrap();
    assert_eq!(pushed.artifact_ids, std::slice::from_ref(&shared));

    let refs = homes.remote_git(&["for-each-ref", "--format=%(refname)"]);
    for name in refs.lines() {
        assert!(
            !name.contains(&private),
            "the unshared tree reached the remote: {name}"
        );
    }
    assert!(
        refs.contains(&format!("records/{shared}/comments/")),
        "{refs}"
    );
}

/// Two overlapping pushes of the same records: a pre-receive hook on the
/// remote holds both after ref advertisement, so the second to update finds
/// every name already created by the first. Both must succeed.
///
/// The two pushes come from two homes holding the same records (B received
/// them through a staging remote): pushes from one home hold its repository
/// lock for their whole window, so they no longer overlap each other.
#[test]
fn racing_pushes_of_the_same_records_both_succeed() {
    use std::os::unix::fs::PermissionsExt;

    let homes = Homes::new("remote-race");
    write_mockup(&homes.fixture, "raced", "body{}");
    let id = publish(
        &homes.a,
        &homes.workspace(),
        "raced",
        ArtifactContentKind::Mockup,
        None,
    )
    .unwrap()
    .artifact_id;
    comment(&homes.a, &id, "alice", "raced comment");
    git(
        &homes.fixture.root,
        &["init", "--bare", "--quiet", "staging.git"],
    );
    let staging = ArtifactRemote::parse(
        homes.fixture.root.join("staging.git").to_str().unwrap(),
        &homes.fixture.root,
        &homes.fixture.root.join("a/artifacts.git"),
    )
    .unwrap();
    push(&homes.a, &staging, &id).unwrap();
    fetch(&homes.b, &staging, &id).unwrap();

    let log = homes.fixture.root.join("pre-receive.log");
    let hook = homes.remote_path.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\ncat >/dev/null\necho run >> '{}'\nsleep 2\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let pushes = [("a", "repo-a"), ("b", "repo-b")]
        .into_iter()
        .map(|(home, repo_id)| {
            let barrier = std::sync::Arc::clone(&barrier);
            let remote = homes.remote.clone();
            let store_path = homes.fixture.root.join(home).join("artifacts.git");
            let id = id.clone();
            std::thread::spawn(move || {
                let store = ArtifactStore::open_existing(&store_path, repo_id)
                    .unwrap()
                    .unwrap();
                barrier.wait();
                push(&store, &remote, &id)
            })
        })
        .collect::<Vec<_>>();
    let outcomes = pushes
        .into_iter()
        .map(|handle| handle.join().unwrap().expect("no false conflict"))
        .collect::<Vec<_>>();

    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        2,
        "both pushes reached the update phase, so they raced"
    );
    let created = outcomes
        .iter()
        .map(|outcome| outcome.created_refs.len())
        .sum::<usize>();
    let settled = outcomes
        .iter()
        .map(|outcome| outcome.up_to_date_refs)
        .sum::<usize>();
    // Content, version and comment: each created once, found once.
    assert_eq!((created, settled), (3, 3), "{outcomes:?}");
    let refs = homes.remote_git(&["for-each-ref", "--format=%(refname)"]);
    assert_eq!(refs.lines().count(), 3, "{refs}");
    assert_eq!(
        refs.lines()
            .filter(|name| name.contains(&format!("records/{id}/comments/")))
            .count(),
        1
    );
}

fn fake_git(fixture: &Fixture, name: &str, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = fixture.write(name, format!("#!/bin/sh\n{script}").as_bytes());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn process_gone(pid: i32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        // SAFETY: signal 0 only checks for existence.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn recorded_pids(path: &Path) -> Vec<i32> {
    std::fs::read_to_string(path)
        .unwrap()
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect()
}

fn fault_remote(fixture: &Fixture) -> ArtifactRemote {
    ArtifactRemote::parse(
        fixture.root.join("unused.git").to_str().unwrap(),
        &fixture.root,
        &fixture.root.join("artifacts.git"),
    )
    .unwrap()
}

/// Git exits at once, but a descendant (as ssh or a credential helper might)
/// inherits stdout and stderr and keeps them open far past the deadline.
#[test]
fn a_descendant_holding_git_pipes_cannot_hold_the_caller() {
    let fixture = Fixture::new("fake-git-descendant");
    let pids = fixture.root.join("pids");
    let program = fake_git(
        &fixture,
        "bin/git",
        &format!(
            "sleep 60 &\necho $! > '{}'\necho reported\nexit 0\n",
            pids.display()
        ),
    );
    let started = Instant::now();
    let output = run_git_bounded(
        &program,
        &fixture.root,
        &fault_remote(&fixture),
        &[],
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(output.success);
    assert_eq!(output.stdout, "reported\n", "what git wrote is kept");
    for pid in recorded_pids(&pids) {
        assert!(process_gone(pid), "descendant {pid} left running");
    }
}

/// Git never exits: the caller gets the timeout error at the deadline, and
/// git and its descendants are gone.
#[test]
fn a_git_that_never_exits_is_killed_with_its_descendants_at_the_deadline() {
    let fixture = Fixture::new("fake-git-hang");
    let pids = fixture.root.join("pids");
    let program = fake_git(
        &fixture,
        "bin/git",
        &format!(
            "echo $$ > '{0}'\nsleep 60 &\necho $! >> '{0}'\nwait\n",
            pids.display()
        ),
    );
    let started = Instant::now();
    let error = run_git_bounded(
        &program,
        &fixture.root,
        &fault_remote(&fixture),
        &[],
        Duration::from_secs(1),
    )
    .err()
    .expect("a hung git is a timeout");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(error.code(), "artifact_remote_failed");
    assert!(error.to_string().contains("did not finish"), "{error}");
    let pids = recorded_pids(&pids);
    assert_eq!(pids.len(), 2);
    for pid in pids {
        assert!(process_gone(pid), "process {pid} left running");
    }
}
