use super::store::{
    inject_failure, ArtifactStore, CommentRequest, DecisionRequest, FailPoint, PublishLimits,
    PublishRequest,
};
use super::types::{ArtifactAnchor, ArtifactContentKind, ArtifactReference, ArtifactRetention};
use super::{resolve_repository_path, rfc3339_utc, ArtifactError, ArtifactStorageContext};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A fixture directory under this worktree's `.tmp/`, removed on drop.
pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
}

impl Fixture {
    pub(crate) fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/artifact-tests")
            .join(format!(
                "{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        Self { root }
    }

    pub(crate) fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn store(&self) -> ArtifactStore {
        ArtifactStore::open_or_create(&self.root.join("artifacts.git"), "repo-test").unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(super) fn publish(
    store: &ArtifactStore,
    workspace: &Path,
    source: &str,
    kind: ArtifactContentKind,
    previous: Option<&str>,
) -> Result<super::types::PublishedArtifact, ArtifactError> {
    publish_with(
        store,
        workspace,
        source,
        kind,
        previous,
        PublishLimits::default(),
    )
}

fn publish_with(
    store: &ArtifactStore,
    workspace: &Path,
    source: &str,
    kind: ArtifactContentKind,
    previous: Option<&str>,
    limits: PublishLimits,
) -> Result<super::types::PublishedArtifact, ArtifactError> {
    store.publish(PublishRequest {
        task_id: "task-1",
        workspace_root: workspace,
        source_path: source,
        kind,
        entrypoint: None,
        previous,
        retention: ArtifactRetention::ThirtyDays,
        limits,
    })
}

pub(super) fn write_mockup(fixture: &Fixture, directory: &str, css: &str) {
    fixture.write(
        &format!("workspace/{directory}/index.html"),
        b"<link rel=stylesheet href=css/site.css><img src=img/logo.png>",
    );
    fixture.write(
        &format!("workspace/{directory}/css/site.css"),
        css.as_bytes(),
    );
    fixture.write(
        &format!("workspace/{directory}/img/logo.png"),
        &[0x89, b'P', b'N', b'G', 0, 1, 2, 255],
    );
}

pub(super) fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn identical_bytes_have_identical_tree_identity_regardless_of_metadata_and_order() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("identity");
    write_mockup(&fixture, "a", "body{color:red}");
    // The same bytes, created in the opposite order, with other modes and
    // timestamps.
    fixture.write(
        "workspace/b/img/logo.png",
        &[0x89, b'P', b'N', b'G', 0, 1, 2, 255],
    );
    fixture.write("workspace/b/css/site.css", b"body{color:red}");
    let index = fixture.write(
        "workspace/b/index.html",
        b"<link rel=stylesheet href=css/site.css><img src=img/logo.png>",
    );
    std::fs::set_permissions(&index, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&index)
        .unwrap()
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400))
        .unwrap();
    let store = fixture.store();
    let workspace = fixture.root.join("workspace");

    let first = publish(&store, &workspace, "a", ArtifactContentKind::Mockup, None).unwrap();
    let second = publish(&store, &workspace, "b", ArtifactContentKind::Mockup, None).unwrap();

    assert_eq!(first.artifact_id, second.artifact_id);
    assert!(first.content_created);
    assert!(!second.content_created, "identical content is stored once");
    assert_ne!(first.version.record_id, second.version.record_id);
    assert_eq!(first.version.entrypoint.as_deref(), Some("index.html"));
    assert_eq!(first.version.retention, ArtifactRetention::ThirtyDays);
    assert_eq!(first.version.file_count, 3);
    assert_eq!(
        first.reference,
        ArtifactReference::Stored {
            repo_id: "repo-test".to_string(),
            artifact_id: first.artifact_id.clone(),
            kind: ArtifactContentKind::Mockup,
        }
    );
    // The public id is the tree, not the retention commit.
    assert_ne!(first.version.storage.commit, first.artifact_id);
    let detail = store.detail(&first.artifact_id).unwrap();
    assert_eq!(
        detail.versions.len(),
        2,
        "each publication keeps its own record"
    );
    assert_eq!(
        detail
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["css/site.css", "img/logo.png", "index.html"]
    );

    write_mockup(&fixture, "c", "body{color:blue}");
    let changed = publish(&store, &workspace, "c", ArtifactContentKind::Mockup, None).unwrap();
    assert_ne!(changed.artifact_id, first.artifact_id);

    // A single file is a one-file tree named by its basename.
    let single = publish(
        &store,
        &workspace,
        "a/css/site.css",
        ArtifactContentKind::Document,
        None,
    )
    .unwrap();
    assert_eq!(single.version.entrypoint.as_deref(), Some("site.css"));
    assert_eq!(
        store
            .read_file(&single.artifact_id, "site.css")
            .unwrap()
            .bytes,
        b"body{color:red}"
    );
}

#[test]
fn versions_link_previous_and_annotations_bind_the_exact_older_id_across_reopen() {
    let fixture = Fixture::new("versions");
    write_mockup(&fixture, "v1", "v1");
    write_mockup(&fixture, "v2", "v2");
    let workspace = fixture.root.join("workspace");
    let store = fixture.store();
    let v1 = publish(&store, &workspace, "v1", ArtifactContentKind::Mockup, None).unwrap();
    store
        .record_comment(
            &v1.artifact_id,
            CommentRequest {
                author: "reviewer (declared)",
                body: "logo is too small",
                anchor: Some(ArtifactAnchor {
                    path: Some("index.html".into()),
                    position: Some("img".into()),
                    excerpt: Some("<img src=img/logo.png>".into()),
                }),
            },
        )
        .unwrap();
    let v2 = publish(
        &store,
        &workspace,
        "v2",
        ArtifactContentKind::Mockup,
        Some(&v1.artifact_id),
    )
    .unwrap();
    assert_eq!(
        v2.version.previous.as_deref(),
        Some(v1.artifact_id.as_str())
    );
    // Annotating the older id after a newer version exists must stay on it.
    let decision = store
        .record_decision(
            &v1.artifact_id,
            DecisionRequest {
                who: "owner",
                what: "reject v1",
            },
        )
        .unwrap();
    assert_eq!(decision.about_artifact_id, v1.artifact_id);
    drop(store);

    let reopened = ArtifactStore::open_existing(&fixture.root.join("artifacts.git"), "repo-test")
        .unwrap()
        .expect("repository persists");
    let old = reopened.detail(&v1.artifact_id).unwrap();
    assert_eq!(old.comments.len(), 1);
    assert_eq!(old.comments[0].about_artifact_id, v1.artifact_id);
    assert_eq!(
        old.comments[0].anchor.as_ref().unwrap().path.as_deref(),
        Some("index.html")
    );
    assert_eq!(old.decisions.len(), 1);
    assert_eq!(old.decisions[0].what, "reject v1");
    assert!(old.retained);
    let new = reopened.detail(&v2.artifact_id).unwrap();
    assert!(new.comments.is_empty() && new.decisions.is_empty());
    assert_eq!(
        new.versions[0].previous.as_deref(),
        Some(v1.artifact_id.as_str())
    );
    // The older version's content stays openable.
    assert_eq!(
        reopened
            .read_file(&v1.artifact_id, "css/site.css")
            .unwrap()
            .bytes,
        b"v1"
    );
}

#[test]
fn identity_and_request_errors_are_explicit() {
    let fixture = Fixture::new("errors");
    write_mockup(&fixture, "m", "x");
    fixture.write("workspace/notes/readme.md", b"# notes");
    let workspace = fixture.root.join("workspace");
    let store = fixture.store();
    let published = publish(&store, &workspace, "m", ArtifactContentKind::Mockup, None).unwrap();
    let blob = store.detail(&published.artifact_id).unwrap();
    assert!(!blob.files.is_empty());

    for malformed in [
        "abc",
        "HEAD",
        &published.artifact_id.to_uppercase(),
        &format!("{}~1", &published.artifact_id[..38]),
    ] {
        assert!(
            matches!(store.detail(malformed), Err(ArtifactError::InvalidId(_))),
            "{malformed}"
        );
    }
    let unknown = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        store.detail(unknown),
        Err(ArtifactError::NotFound {
            repo_id: "repo-test".into(),
            artifact_id: unknown.into()
        })
    );
    // A real object that is not a published tree.
    let blob_id = git2::Repository::open_bare(fixture.root.join("artifacts.git"))
        .unwrap()
        .blob(b"x")
        .unwrap()
        .to_string();
    assert!(matches!(
        store.detail(&blob_id),
        Err(ArtifactError::WrongObjectType { .. })
    ));
    assert!(matches!(
        publish(
            &store,
            &workspace,
            "m",
            ArtifactContentKind::Mockup,
            Some(unknown)
        ),
        Err(ArtifactError::InvalidPrevious { .. })
    ));
    assert!(matches!(
        publish(
            &store,
            &workspace,
            "notes",
            ArtifactContentKind::Mockup,
            None
        ),
        Err(ArtifactError::InvalidEntrypoint(_))
    ));
    assert!(matches!(
        store.publish(PublishRequest {
            entrypoint: Some("missing.html"),
            ..request(&workspace, "m")
        }),
        Err(ArtifactError::InvalidEntrypoint(_))
    ));
    assert_eq!(
        store
            .read_file(&published.artifact_id, "css/missing.css")
            .err(),
        Some(ArtifactError::FileNotFound {
            artifact_id: published.artifact_id.clone(),
            path: "css/missing.css".into()
        })
    );
    assert!(matches!(
        store.read_file(&published.artifact_id, "../index.html"),
        Err(ArtifactError::InvalidPath(_))
    ));
    assert!(matches!(
        store.record_comment(
            &published.artifact_id,
            CommentRequest {
                author: " ",
                body: "x",
                anchor: None
            }
        ),
        Err(ArtifactError::InvalidRequest(_))
    ));
    assert!(matches!(
        store.record_comment(
            &published.artifact_id,
            CommentRequest {
                author: "a",
                body: "x",
                anchor: Some(ArtifactAnchor {
                    path: Some("nope.css".into()),
                    ..Default::default()
                })
            }
        ),
        Err(ArtifactError::FileNotFound { .. })
    ));
    assert!(matches!(
        store.record_decision(
            unknown,
            DecisionRequest {
                who: "a",
                what: "b"
            }
        ),
        Err(ArtifactError::NotFound { .. })
    ));
}

fn request<'a>(workspace: &'a Path, source: &'a str) -> PublishRequest<'a> {
    PublishRequest {
        task_id: "task-1",
        workspace_root: workspace,
        source_path: source,
        kind: ArtifactContentKind::Mockup,
        entrypoint: None,
        previous: None,
        retention: ArtifactRetention::Keep,
        limits: PublishLimits::default(),
    }
}

#[test]
fn workspace_boundaries_refuse_escapes_special_files_and_oversized_payloads() {
    let fixture = Fixture::new("boundaries");
    let workspace = fixture.root.join("workspace");
    write_mockup(&fixture, "ok", "x");
    fixture.write("outside/secret.txt", b"secret");
    let store = fixture.store();
    let invalid = |source: &str| {
        let result = publish(
            &store,
            &workspace,
            source,
            ArtifactContentKind::Document,
            None,
        );
        assert!(
            matches!(result, Err(ArtifactError::InvalidPath(_))),
            "{source}: {result:?}"
        );
    };

    invalid("../outside");
    invalid(fixture.root.join("outside/secret.txt").to_str().unwrap());
    invalid("");
    invalid(".git");

    std::os::unix::fs::symlink(fixture.root.join("outside"), workspace.join("link-dir")).unwrap();
    invalid("link-dir");
    invalid("link-dir/secret.txt");
    std::fs::create_dir_all(workspace.join("with-link")).unwrap();
    fixture.write("workspace/with-link/index.html", b"x");
    std::os::unix::fs::symlink(
        fixture.root.join("outside/secret.txt"),
        workspace.join("with-link/leak.txt"),
    )
    .unwrap();
    invalid("with-link");
    // Even a symlink that stays inside the workspace is refused.
    fixture.write("workspace/inner-link/index.html", b"x");
    std::os::unix::fs::symlink("index.html", workspace.join("inner-link/alias.html")).unwrap();
    invalid("inner-link");

    fixture.write("workspace/with-git/index.html", b"x");
    fixture.write("workspace/with-git/.git/HEAD", b"ref: refs/heads/main");
    invalid("with-git");

    fixture.write("workspace/with-fifo/index.html", b"x");
    let fifo = std::ffi::CString::new(workspace.join("with-fifo/pipe").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    invalid("with-fifo");

    std::fs::create_dir_all(workspace.join("empty/nested")).unwrap();
    invalid("empty");
    assert!(matches!(
        publish(
            &store,
            &workspace,
            "missing",
            ArtifactContentKind::Document,
            None
        ),
        Err(ArtifactError::SourceNotFound(_))
    ));

    let tight = |limits: PublishLimits| {
        let result = publish_with(
            &store,
            &workspace,
            "ok",
            ArtifactContentKind::Mockup,
            None,
            limits,
        );
        assert!(
            matches!(result, Err(ArtifactError::TooLarge(_))),
            "{limits:?}: {result:?}"
        );
    };
    let base = PublishLimits::default();
    tight(PublishLimits {
        max_files: 2,
        ..base
    });
    tight(PublishLimits {
        max_depth: 1,
        ..base
    });
    tight(PublishLimits {
        max_file_bytes: 20,
        ..base
    });
    tight(PublishLimits {
        max_total_bytes: 69,
        ..base
    });
    // Nothing refused above left a descriptor behind.
    assert!(
        publish(&store, &workspace, "ok", ArtifactContentKind::Mockup, None)
            .unwrap()
            .content_created
    );
}

#[test]
fn concurrent_annotations_from_separate_handles_all_survive() {
    let fixture = Fixture::new("concurrent");
    write_mockup(&fixture, "m", "x");
    let published = publish(
        &fixture.store(),
        &fixture.root.join("workspace"),
        "m",
        ArtifactContentKind::Mockup,
        None,
    )
    .unwrap();
    let repository = fixture.root.join("artifacts.git");
    let threads = (0..8)
        .map(|index| {
            let repository = repository.clone();
            let id = published.artifact_id.clone();
            std::thread::spawn(move || {
                let store = ArtifactStore::open_existing(&repository, "repo-test")
                    .unwrap()
                    .unwrap();
                if index % 2 == 0 {
                    store
                        .record_comment(
                            &id,
                            CommentRequest {
                                author: "a",
                                body: &format!("c{index}"),
                                anchor: None,
                            },
                        )
                        .map(|_| ())
                } else {
                    store
                        .record_decision(
                            &id,
                            DecisionRequest {
                                who: "w",
                                what: &format!("d{index}"),
                            },
                        )
                        .map(|_| ())
                }
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap().unwrap();
    }
    let detail = fixture.store().detail(&published.artifact_id).unwrap();
    assert_eq!(detail.comments.len(), 4);
    assert_eq!(detail.decisions.len(), 4);
}

#[test]
fn a_failed_publication_never_exposes_a_descriptor() {
    let fixture = Fixture::new("faults");
    write_mockup(&fixture, "m", "x");
    let workspace = fixture.root.join("workspace");
    let store = fixture.store();

    inject_failure(Some(FailPoint::BeforeContentRef));
    let error = publish(&store, &workspace, "m", ArtifactContentKind::Mockup, None).unwrap_err();
    inject_failure(None);
    assert!(matches!(error, ArtifactError::Storage(_)));
    let bare = git2::Repository::open_bare(fixture.root.join("artifacts.git")).unwrap();
    assert!(bare
        .references_glob("refs/kanna/artifacts/*")
        .unwrap()
        .next()
        .is_none());

    inject_failure(Some(FailPoint::BeforeMetadataRef));
    let error = publish(&store, &workspace, "m", ArtifactContentKind::Mockup, None).unwrap_err();
    inject_failure(None);
    assert!(matches!(error, ArtifactError::Storage(_)));
    let tree_ref = bare
        .references_glob("refs/kanna/artifacts/trees/*")
        .unwrap()
        .next()
        .expect("content was retained before the failure")
        .unwrap();
    let orphan = tree_ref.peel_to_commit().unwrap().tree_id().to_string();
    assert!(matches!(
        store.detail(&orphan),
        Err(ArtifactError::NotFound { .. })
    ));
    assert!(matches!(
        store.read_file(&orphan, "index.html"),
        Err(ArtifactError::NotFound { .. })
    ));

    let retried = publish(&store, &workspace, "m", ArtifactContentKind::Mockup, None).unwrap();
    assert_eq!(retried.artifact_id, orphan);
    assert!(!retried.content_created);
    assert_eq!(store.detail(&orphan).unwrap().versions.len(), 1);
}

#[test]
fn retained_refs_survive_aggressive_garbage_collection() {
    let fixture = Fixture::new("gc");
    write_mockup(&fixture, "v1", "v1");
    write_mockup(&fixture, "v2", "v2");
    let workspace = fixture.root.join("workspace");
    let store = fixture.store();
    let v1 = publish(&store, &workspace, "v1", ArtifactContentKind::Mockup, None).unwrap();
    let v2 = publish(
        &store,
        &workspace,
        "v2",
        ArtifactContentKind::Mockup,
        Some(&v1.artifact_id),
    )
    .unwrap();
    store
        .record_comment(
            &v1.artifact_id,
            CommentRequest {
                author: "a",
                body: "b",
                anchor: None,
            },
        )
        .unwrap();
    drop(store);

    git(
        &fixture.root.join("artifacts.git"),
        &["gc", "--prune=now", "--aggressive", "--quiet"],
    );
    git(
        &fixture.root.join("artifacts.git"),
        &["fsck", "--strict", "--no-dangling"],
    );

    let store = fixture.store();
    for (id, css) in [(&v1.artifact_id, b"v1"), (&v2.artifact_id, b"v2")] {
        assert!(store.detail(id).unwrap().retained);
        assert_eq!(&store.read_file(id, "css/site.css").unwrap().bytes, css);
    }
    assert_eq!(store.detail(&v1.artifact_id).unwrap().comments.len(), 1);
    // Content commits are parentless: v2 does not keep v1 reachable.
    let bare = git2::Repository::open_bare(fixture.root.join("artifacts.git")).unwrap();
    let v2_commit = bare
        .find_commit(git2::Oid::from_str(&v2.version.storage.commit).unwrap())
        .unwrap();
    assert_eq!(v2_commit.parent_count(), 0);
}

#[test]
fn publishing_from_a_working_repository_leaves_it_untouched() {
    let fixture = Fixture::new("pollution");
    let working = fixture.root.join("working");
    std::fs::create_dir_all(&working).unwrap();
    git(&working, &["init", "--quiet", "-b", "main"]);
    git(
        &working,
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
    std::fs::write(working.join(".git/info/exclude"), "mock/\n").unwrap();
    fixture.write("working/mock/index.html", b"<img src=big.png>");
    fixture.write("working/mock/big.png", &vec![7_u8; 64 * 1024]);
    let before_head = git(&working, &["rev-parse", "HEAD"]);
    let before_index = std::fs::read(working.join(".git/index")).ok();
    let before_objects = git(&working, &["count-objects", "-v"]);

    let store =
        ArtifactStore::open_or_create(&fixture.root.join("store/artifacts.git"), "repo-test")
            .unwrap();
    let published = store
        .publish(PublishRequest {
            workspace_root: &working,
            ..request(&working, "mock")
        })
        .unwrap();

    assert_eq!(git(&working, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(std::fs::read(working.join(".git/index")).ok(), before_index);
    assert_eq!(git(&working, &["count-objects", "-v"]), before_objects);
    assert_eq!(git(&working, &["status", "--porcelain"]), "");
    let working_repository = git2::Repository::open(&working).unwrap();
    let artifact_tree = git2::Oid::from_str(&published.artifact_id).unwrap();
    assert!(working_repository.find_object(artifact_tree, None).is_err());
    assert!(working_repository
        .references()
        .unwrap()
        .all(|reference| !reference
            .unwrap()
            .name()
            .unwrap()
            .contains("kanna/artifacts")));
}

#[test]
fn repository_location_defaults_under_home_and_never_overlaps_the_working_repository() {
    let fixture = Fixture::new("location");
    let home = fixture.root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let context = ArtifactStorageContext::with_home(&home);
    let working = fixture.root.join("working");
    std::fs::create_dir_all(&working).unwrap();
    git(&working, &["init", "--quiet", "-b", "main"]);
    git(
        &working,
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
    let linked = fixture.root.join("linked-worktree");
    git(
        &working,
        &["worktree", "add", "--quiet", linked.to_str().unwrap()],
    );
    std::os::unix::fs::symlink(&working, fixture.root.join("alias")).unwrap();

    assert_eq!(
        resolve_repository_path(&context, "repo-abc", &working, None).unwrap(),
        home.join(".kanna/repos/repo-abc/artifacts.git")
    );
    assert_eq!(
        resolve_repository_path(&context, "repo-abc", &working, Some("~/art/a.git")).unwrap(),
        home.join("art/a.git")
    );
    let absolute = fixture.root.join("elsewhere/a.git");
    assert_eq!(
        resolve_repository_path(
            &context,
            "repo-abc",
            &working,
            Some(absolute.to_str().unwrap())
        )
        .unwrap(),
        absolute
    );
    let rejected = |configured: Option<&str>, repo_id: &str| {
        let result = resolve_repository_path(&context, repo_id, &working, configured);
        assert!(
            matches!(result, Err(ArtifactError::Location(_))),
            "{configured:?}: {result:?}"
        );
    };
    rejected(Some("artifacts.git"), "repo-abc");
    rejected(Some(".git/artifacts"), "repo-abc");
    rejected(
        Some(fixture.root.join("alias/artifacts.git").to_str().unwrap()),
        "repo-abc",
    );
    rejected(
        Some(linked.join("artifacts.git").to_str().unwrap()),
        "repo-abc",
    );
    rejected(Some(fixture.root.to_str().unwrap()), "repo-abc");
    rejected(Some("../outside.git"), "repo-abc");
    rejected(None, "../escape");
    rejected(None, "");

    // An existing non-repository directory is never adopted.
    fixture.write("occupied/file.txt", b"x");
    assert!(matches!(
        ArtifactStore::open_or_create(&fixture.root.join("occupied"), "repo-abc"),
        Err(ArtifactError::Location(_))
    ));
    assert!(
        ArtifactStore::open_existing(&fixture.root.join("absent.git"), "repo-abc")
            .unwrap()
            .is_none()
    );
    assert!(!fixture.root.join("absent.git").exists());
}

#[test]
fn timestamps_are_rfc3339_utc() {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_758_540_123_456);
    assert_eq!(rfc3339_utc(time), "2025-09-22T11:22:03.456Z");
    assert_eq!(
        rfc3339_utc(std::time::UNIX_EPOCH),
        "1970-01-01T00:00:00.000Z"
    );
    let leap = std::time::UNIX_EPOCH + std::time::Duration::from_secs(951_782_400);
    assert_eq!(rfc3339_utc(leap), "2000-02-29T00:00:00.000Z");
}
