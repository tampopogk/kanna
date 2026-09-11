//! Bounded, incremental collection of provider session usage.
//!
//! Scanning is scoped to the working directories this repository's own task
//! runs recorded, so it never walks an operator's whole home looking for
//! transcripts, and it resumes from the byte offset the last scan reached so a
//! long-running session's file is read once rather than on every request.

use super::{claude, codex, ParsedUsage, SessionContext};
use crate::db::{Db, RepoRunWindow as RunWindow, TokenUsageRecord, UsageScanCheckpoint};
use crate::task_creator::{claude_project_slug, claude_projects_dir, home_child, same_cwd};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;

#[cfg(test)]
static CODEX_DISCOVERY_INSPECTIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// What a collection pass actually managed to observe.
///
/// Reported rather than hidden: a provider whose session files this desktop
/// cannot read is a hole in the numbers, and the view has to say so instead
/// of showing a confident zero.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectionReport {
    pub files_scanned: usize,
    pub records_written: usize,
    /// Providers this repository's runs used for which no usage record could
    /// be read — an unsupported CLI, or one whose session files are gone.
    pub providers_without_usage: Vec<String>,
}

/// Read every provider session file belonging to this repository's task
/// worktrees and store any usage record not already held.
pub fn collect_repo_token_usage(db: &Db, repo_id: &str) -> Result<CollectionReport, String> {
    let runs = db
        .list_repo_run_windows(repo_id)
        .map_err(|error| format!("db error: {error}"))?;
    let mut report = CollectionReport::default();
    if runs.is_empty() {
        return Ok(report);
    }

    // Only inspect a provider's session store when this repository actually
    // ran that CLI. Codex discovery below is further bounded to the dated
    // directory and recorded session context of each run.
    let providers: HashSet<String> = runs.iter().filter_map(|run| run.provider.clone()).collect();
    let mut uncovered = HashSet::new();
    if providers.contains(SUPPORTED_PROVIDERS[0]) {
        let discovery = claude_session_files(&runs);
        if !discovery.complete {
            uncovered.insert("claude".to_string());
        }
        for (path, context) in discovery.files {
            let outcome = scan_file(
                db,
                repo_id,
                &path,
                "claude",
                context,
                &runs,
                claude::parse_line,
            )?;
            report.files_scanned += 1;
            match outcome {
                ScanFileOutcome::Read(written) => report.records_written += written,
                ScanFileOutcome::Unchanged => {}
                ScanFileOutcome::Unreadable => {
                    uncovered.insert("claude".to_string());
                }
            }
        }
    }
    if providers.contains(SUPPORTED_PROVIDERS[1]) {
        let discovery = codex_session_files(db, &runs)?;
        if !discovery.complete {
            uncovered.insert("codex".to_string());
        }
        for (path, context) in discovery.files {
            let outcome = scan_file(
                db,
                repo_id,
                &path,
                "codex",
                context,
                &runs,
                codex::parse_line,
            )?;
            report.files_scanned += 1;
            match outcome {
                ScanFileOutcome::Read(written) => report.records_written += written,
                ScanFileOutcome::Unchanged => {}
                ScanFileOutcome::Unreadable => {
                    uncovered.insert("codex".to_string());
                }
            }
        }
    }

    for provider in &providers {
        if !SUPPORTED_PROVIDERS.contains(&provider.as_str()) {
            uncovered.insert(provider.clone());
            continue;
        }
        let covered = db
            .repo_provider_run_ids_with_usage(repo_id, provider)
            .map_err(|error| format!("db error: {error}"))?;
        if runs.iter().any(|run| {
            run.provider.as_deref() == Some(provider.as_str()) && !covered.contains(&run.run_id)
        }) {
            uncovered.insert(provider.clone());
        }
    }
    report.providers_without_usage = uncovered.into_iter().collect();
    report.providers_without_usage.sort();
    report.providers_without_usage.dedup();
    Ok(report)
}

/// The agent CLIs whose local session files record usage Kanna can read.
/// Anything else a repository runs is reported as uncovered.
const SUPPORTED_PROVIDERS: [&str; 2] = ["claude", "codex"];

/// Claude stores a project's transcripts in one directory derived from the
/// working directory, so the repository's own run cwds name exactly the
/// directories worth reading.
struct DiscoveryResult {
    files: Vec<(PathBuf, SessionContext)>,
    complete: bool,
}

fn claude_session_files(runs: &[RunWindow]) -> DiscoveryResult {
    let Some(projects_dir) = claude_projects_dir() else {
        return DiscoveryResult {
            files: Vec::new(),
            complete: false,
        };
    };
    let mut seen: HashMap<PathBuf, SessionContext> = HashMap::new();
    let mut complete = true;
    for cwd in distinct_cwds(runs) {
        let directory = projects_dir.join(claude_project_slug(&cwd));
        let Ok(entries) = std::fs::read_dir(&directory) else {
            complete = false;
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
                seen.insert(
                    path,
                    SessionContext {
                        cwd: Some(cwd.clone()),
                        ..Default::default()
                    },
                );
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            let subagents = path.join("subagents");
            match std::fs::read_dir(&subagents) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|extension| extension.to_str())
                            == Some("jsonl")
                        {
                            seen.insert(
                                path,
                                SessionContext {
                                    cwd: Some(cwd.clone()),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => complete = false,
            }
        }
    }
    DiscoveryResult {
        files: seen.into_iter().collect(),
        complete,
    }
}

/// Codex files are date-partitioned rather than laid out by working directory.
/// Inspect only the start-date directory recorded by each run, then retain its
/// matching candidates until that directory's generation changes.
fn codex_session_files(db: &Db, runs: &[RunWindow]) -> Result<DiscoveryResult, String> {
    let Some(config_dir) = home_child("CODEX_HOME", ".codex") else {
        return Ok(DiscoveryResult {
            files: Vec::new(),
            complete: false,
        });
    };
    let mut files: HashMap<PathBuf, SessionContext> = HashMap::new();
    let mut complete = true;
    for run in runs
        .iter()
        .filter(|run| run.provider.as_deref() == Some("codex"))
    {
        let Some(date) = run.started_at.get(..10) else {
            complete = false;
            continue;
        };
        let mut date_parts = date.split('-');
        let (Some(year), Some(month), Some(day)) =
            (date_parts.next(), date_parts.next(), date_parts.next())
        else {
            complete = false;
            continue;
        };
        let directory = config_dir.join("sessions").join(year).join(month).join(day);
        let Ok(metadata) = std::fs::metadata(&directory) else {
            complete = false;
            continue;
        };
        let modified_ns = modified_ns(&metadata);
        let discovery_key = format!(
            "codex:{}:{}",
            run.run_id,
            run.provider_session_id.as_deref().unwrap_or("unknown")
        );
        let directory_path = directory.to_string_lossy().to_string();
        let cached = db
            .usage_discovery_state(&discovery_key)
            .map_err(|error| format!("db error: {error}"))?;
        let candidate_paths = if let Some(cached) = cached.filter(|cached| {
            cached.directory_path == directory_path && cached.directory_modified_ns == modified_ns
        }) {
            cached.candidate_paths
        } else {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                complete = false;
                continue;
            };
            let mut candidates = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                    continue;
                }
                #[cfg(test)]
                CODEX_DISCOVERY_INSPECTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(header) = read_first_line(&path) else {
                    continue;
                };
                let (_, context) = codex::parse_line(&header);
                let matches = match run.provider_session_id.as_deref() {
                    Some(session_id) => context.session_id.as_deref() == Some(session_id),
                    None => context
                        .cwd
                        .as_deref()
                        .is_some_and(|cwd| same_cwd(&run.cwd, cwd)),
                };
                if matches {
                    candidates.push(path.to_string_lossy().to_string());
                }
            }
            db.record_usage_discovery(
                &discovery_key,
                "codex",
                &directory_path,
                modified_ns,
                &candidates,
            )
            .map_err(|error| format!("db error: {error}"))?;
            candidates
        };
        if candidate_paths.is_empty() {
            complete = false;
        }
        for path in candidate_paths {
            files.insert(
                PathBuf::from(path),
                SessionContext {
                    session_id: run.provider_session_id.clone(),
                    cwd: Some(run.cwd.clone()),
                    ..Default::default()
                },
            );
        }
    }
    Ok(DiscoveryResult {
        files: files.into_iter().collect(),
        complete,
    })
}

fn modified_ns(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn distinct_cwds(runs: &[RunWindow]) -> Vec<String> {
    let mut cwds: Vec<String> = runs.iter().map(|run| run.cwd.clone()).collect();
    cwds.sort();
    cwds.dedup();
    cwds
}

fn read_first_line(path: &std::path::Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    BufReader::new(file).lines().next()?.ok()
}

enum ScanFileOutcome {
    Read(usize),
    Unchanged,
    Unreadable,
}

fn scan_file(
    db: &Db,
    repo_id: &str,
    path: &std::path::Path,
    provider: &str,
    discovered: SessionContext,
    runs: &[RunWindow],
    parse_line: fn(&str) -> (Option<ParsedUsage>, SessionContext),
) -> Result<ScanFileOutcome, String> {
    let file_path = path.to_string_lossy().to_string();
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(ScanFileOutcome::Unreadable);
    };
    let file_size = metadata.len() as i64;
    let state = db
        .usage_scan_state(&file_path)
        .map_err(|error| format!("db error: {error}"))?;

    // A file that has not grown holds nothing new. A file that shrank was
    // rewritten, so the offset it left behind means nothing and it is read
    // from the start again — the content-derived keys make that harmless.
    let mut context = SessionContext::default();
    let start_offset = match &state {
        Some(state) if file_size == state.file_size => return Ok(ScanFileOutcome::Unchanged),
        Some(state) if file_size > state.file_size => {
            context.absorb(SessionContext {
                session_id: state.session_id.clone(),
                cwd: state.cwd.clone(),
                model: state.model.clone(),
            });
            state.byte_offset
        }
        _ => 0,
    };
    context.absorb(SessionContext {
        cwd: discovered.cwd.clone(),
        session_id: discovered.session_id.clone(),
        model: discovered.model.clone(),
    });

    let Ok(file) = std::fs::File::open(path) else {
        return Ok(ScanFileOutcome::Unreadable);
    };
    let mut reader = BufReader::new(file);
    if start_offset > 0 && reader.seek(SeekFrom::Start(start_offset as u64)).is_err() {
        return Ok(ScanFileOutcome::Unreadable);
    }

    let mut consumed = start_offset;
    let mut records = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => return Ok(ScanFileOutcome::Unreadable),
        };
        // A line still being written has no terminator yet. Stopping before
        // it keeps the offset on a record boundary, so the complete line is
        // read by the next pass instead of being parsed in half.
        if !line.ends_with('\n') {
            break;
        }
        consumed += read as i64;
        let (usage, line_context) = parse_line(line.trim_end());
        context.absorb(line_context);
        if let Some(usage) = usage {
            records.push(to_record(usage, provider, repo_id, &context, runs));
        }
    }

    let written = db
        .record_token_usage(
            &records,
            Some(UsageScanCheckpoint {
                file_path: &file_path,
                provider,
                file_size,
                byte_offset: consumed,
                session_id: context.session_id.as_deref(),
                cwd: context.cwd.as_deref(),
                model: context.model.as_deref(),
            }),
        )
        .map_err(|error| format!("db error: {error}"))?;
    Ok(ScanFileOutcome::Read(written))
}

fn to_record(
    mut usage: ParsedUsage,
    provider: &str,
    repo_id: &str,
    context: &SessionContext,
    runs: &[RunWindow],
) -> TokenUsageRecord {
    let total_tokens = usage.total_tokens();
    // Normalized before attribution, not after: a run window is stored in
    // SQLite's spelling, and `'T' > ' '` would otherwise put every ISO
    // timestamp after its own run's end.
    usage.occurred_at = normalize_timestamp(&usage.occurred_at);
    let (task_id, run_id) = attribute(&usage, context, runs);
    TokenUsageRecord {
        usage_key: usage.usage_key,
        provider: provider.to_string(),
        provider_session_id: usage.session_id.or_else(|| context.session_id.clone()),
        // A record whose task could not be determined is not this repository's
        // to claim either; leaving both null is what keeps it out of the
        // totals instead of silently inflating them.
        repo_id: task_id.as_ref().map(|_| repo_id.to_string()),
        task_id,
        run_id,
        model: usage.model.or_else(|| context.model.clone()),
        occurred_at: usage.occurred_at,
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        total_tokens,
    }
}

/// Provider timestamps are ISO 8601 (`2026-09-01T10:00:00.500Z`); every
/// timestamp this database already holds is SQLite's `2026-09-01 10:00:00`.
/// Storing one spelling is what lets a window bound compare against a usage
/// row at all — `'T' > ' '` would otherwise silently mis-order them.
fn normalize_timestamp(value: &str) -> String {
    let trimmed = value.trim_end_matches('Z');
    let without_fraction = trimmed.split('.').next().unwrap_or(trimmed);
    without_fraction.replacen('T', " ", 1)
}

/// Attribute a usage record to the run that was live in its working directory
/// when it happened.
///
/// Several stages of one task share a worktree, so the working directory alone
/// names a task but not a run. The record's own instant decides the run; when
/// no run's window contains it the task is still known and the run is left
/// unset, and when the directory itself is ambiguous — two tasks that reused
/// one path — nothing is claimed at all.
fn attribute(
    usage: &ParsedUsage,
    context: &SessionContext,
    runs: &[RunWindow],
) -> (Option<String>, Option<String>) {
    let Some(cwd) = context.cwd.as_deref() else {
        return (None, None);
    };
    let candidates: Vec<&RunWindow> = runs.iter().filter(|run| same_cwd(&run.cwd, cwd)).collect();
    if candidates.is_empty() {
        return (None, None);
    }

    let mut tasks: Vec<&str> = candidates.iter().map(|run| run.task_id.as_str()).collect();
    tasks.sort();
    tasks.dedup();

    let containing = candidates.iter().find(|run| {
        usage.occurred_at.as_str() >= run.started_at.as_str()
            && run
                .finished_at
                .as_deref()
                .is_none_or(|finished| usage.occurred_at.as_str() <= finished)
    });
    if let Some(run) = containing {
        return (Some(run.task_id.clone()), Some(run.run_id.clone()));
    }
    // Outside every run window but inside a directory only one task ever
    // owned: the task is certain, the run is not.
    match tasks.as_slice() {
        [task_id] => (Some((*task_id).to_string()), None),
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::{attribute, normalize_timestamp};
    use crate::db::RepoRunWindow as RunWindow;
    use crate::usage_collection::{ParsedUsage, SessionContext};

    fn run(task: &str, id: &str, cwd: &str, started: &str, finished: Option<&str>) -> RunWindow {
        RunWindow {
            task_id: task.into(),
            run_id: id.into(),
            provider: None,
            provider_session_id: None,
            cwd: cwd.into(),
            started_at: started.into(),
            finished_at: finished.map(str::to_string),
        }
    }

    fn usage_at(occurred_at: &str) -> ParsedUsage {
        ParsedUsage {
            occurred_at: occurred_at.into(),
            ..Default::default()
        }
    }

    fn context(cwd: &str) -> SessionContext {
        SessionContext {
            cwd: Some(cwd.into()),
            ..Default::default()
        }
    }

    #[test]
    fn provider_timestamps_are_stored_in_the_form_every_window_compares_against() {
        assert_eq!(
            normalize_timestamp("2026-09-01T10:00:00.500Z"),
            "2026-09-01 10:00:00"
        );
        assert_eq!(
            normalize_timestamp("2026-09-01 10:00:00"),
            "2026-09-01 10:00:00"
        );
    }

    #[test]
    fn usage_lands_on_the_run_that_was_live_when_it_happened() {
        let runs = vec![
            run(
                "t1",
                "r1",
                "/w",
                "2026-09-01T10:00:00Z",
                Some("2026-09-01T11:00:00Z"),
            ),
            run("t1", "r2", "/w", "2026-09-01T12:00:00Z", None),
        ];
        assert_eq!(
            attribute(&usage_at("2026-09-01T10:30:00Z"), &context("/w"), &runs),
            (Some("t1".into()), Some("r1".into()))
        );
        assert_eq!(
            attribute(&usage_at("2026-09-01T13:00:00Z"), &context("/w"), &runs),
            (Some("t1".into()), Some("r2".into()))
        );
    }

    #[test]
    fn usage_between_runs_still_belongs_to_the_task_that_owns_the_worktree() {
        let runs = vec![run(
            "t1",
            "r1",
            "/w",
            "2026-09-01T10:00:00Z",
            Some("2026-09-01T11:00:00Z"),
        )];
        assert_eq!(
            attribute(&usage_at("2026-09-01T11:30:00Z"), &context("/w"), &runs),
            (Some("t1".into()), None)
        );
    }

    #[test]
    fn a_directory_two_tasks_shared_outside_any_run_window_claims_neither() {
        let runs = vec![
            run(
                "t1",
                "r1",
                "/w",
                "2026-09-01T10:00:00Z",
                Some("2026-09-01T11:00:00Z"),
            ),
            run(
                "t2",
                "r2",
                "/w",
                "2026-09-02T10:00:00Z",
                Some("2026-09-02T11:00:00Z"),
            ),
        ];
        assert_eq!(
            attribute(&usage_at("2026-09-03T10:00:00Z"), &context("/w"), &runs),
            (None, None)
        );
    }

    #[test]
    fn usage_from_an_unknown_directory_is_never_claimed() {
        let runs = vec![run("t1", "r1", "/w", "2026-09-01T10:00:00Z", None)];
        assert_eq!(
            attribute(
                &usage_at("2026-09-01T10:30:00Z"),
                &context("/elsewhere"),
                &runs
            ),
            (None, None)
        );
        assert_eq!(
            attribute(
                &usage_at("2026-09-01T10:30:00Z"),
                &SessionContext::default(),
                &runs
            ),
            (None, None)
        );
    }
}

/// End-to-end collection against real session files on disk.
///
/// The parsers are unit-tested above; what these prove is the wiring — that a
/// file is found from a task's recorded worktree, attributed to that task's
/// run, read incrementally as it grows, and never counted twice.
#[cfg(test)]
mod collection_tests {
    use crate::db::Db;
    use std::io::Write;
    use std::sync::{Mutex, MutexGuard};

    /// `CLAUDE_CONFIG_DIR` and `CODEX_HOME` are process-global, so the tests
    /// that redirect them take turns.
    static PROVIDER_HOME: Mutex<()> = Mutex::new(());

    struct ProviderHomes {
        _guard: MutexGuard<'static, ()>,
        root: std::path::PathBuf,
        previous_claude: Option<String>,
        previous_codex: Option<String>,
    }

    impl ProviderHomes {
        fn new(label: &str) -> Self {
            let guard = PROVIDER_HOME
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let root = std::path::PathBuf::from(crate::test_paths::unique_test_file(
                &format!("usage-collection-{label}"),
                "dir",
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("claude/projects")).expect("claude dir");
            std::fs::create_dir_all(root.join("codex/sessions")).expect("codex dir");
            let homes = Self {
                previous_claude: std::env::var("CLAUDE_CONFIG_DIR").ok(),
                previous_codex: std::env::var("CODEX_HOME").ok(),
                _guard: guard,
                root,
            };
            std::env::set_var("CLAUDE_CONFIG_DIR", homes.root.join("claude"));
            std::env::set_var("CODEX_HOME", homes.root.join("codex"));
            homes
        }

        fn write_claude_transcript(&self, cwd: &str, session: &str, lines: &[String]) {
            let directory = self
                .root
                .join("claude/projects")
                .join(crate::task_creator::claude_project_slug(cwd));
            std::fs::create_dir_all(&directory).expect("project dir");
            let mut file =
                std::fs::File::create(directory.join(format!("{session}.jsonl"))).expect("create");
            for line in lines {
                writeln!(file, "{line}").expect("write");
            }
        }

        fn append_claude_transcript(&self, cwd: &str, session: &str, line: &str) {
            let path = self
                .root
                .join("claude/projects")
                .join(crate::task_creator::claude_project_slug(cwd))
                .join(format!("{session}.jsonl"));
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .expect("append");
            writeln!(file, "{line}").expect("write");
        }

        fn write_claude_subagent(&self, cwd: &str, session: &str, name: &str, lines: &[String]) {
            let directory = self
                .root
                .join("claude/projects")
                .join(crate::task_creator::claude_project_slug(cwd))
                .join(session)
                .join("subagents");
            std::fs::create_dir_all(&directory).expect("subagents dir");
            let mut file = std::fs::File::create(directory.join(format!("{name}.jsonl")))
                .expect("create subagent transcript");
            for line in lines {
                writeln!(file, "{line}").expect("write");
            }
        }

        fn write_codex_rollout(&self, name: &str, lines: &[String]) {
            let directory = self.root.join("codex/sessions/2026/04/17");
            std::fs::create_dir_all(&directory).expect("rollout dir");
            let mut file =
                std::fs::File::create(directory.join(format!("{name}.jsonl"))).expect("create");
            for line in lines {
                writeln!(file, "{line}").expect("write");
            }
        }
    }

    impl Drop for ProviderHomes {
        fn drop(&mut self) {
            match self.previous_claude.take() {
                Some(value) => std::env::set_var("CLAUDE_CONFIG_DIR", value),
                None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
            }
            match self.previous_codex.take() {
                Some(value) => std::env::set_var("CODEX_HOME", value),
                None => std::env::remove_var("CODEX_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn claude_turn(message_id: &str, timestamp: &str, cwd: &str, output: i64) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"sess-1","cwd":"{cwd}","timestamp":"{timestamp}","requestId":"req-{message_id}","message":{{"id":"{message_id}","model":"claude-opus-5","usage":{{"input_tokens":10,"cache_read_input_tokens":90,"cache_creation_input_tokens":5,"output_tokens":{output}}}}}}}"#
        )
    }

    fn seeded_db(label: &str, cwd: &str) -> Db {
        let db = Db::open_for_tests(&Db::test_db_path(label)).expect("open db");
        db.insert_test_repo("repo-1", "Repo One").expect("repo");
        db.insert_test_pipeline_item(
            "task-1",
            "repo-1",
            "prompt",
            Some("Task One"),
            "in progress",
            "2026-04-17 08:00:00",
        )
        .expect("task");
        db.insert_test_provider_stage_run(
            "run-1",
            "task-1",
            "in progress",
            "claude",
            cwd,
            "2026-04-17 09:00:00",
            Some("2026-04-17 11:00:00"),
        )
        .expect("stage run");
        db
    }

    #[test]
    fn a_transcript_in_a_task_worktree_is_read_attributed_and_never_counted_twice() {
        let homes = ProviderHomes::new("claude-basic");
        let cwd = homes.root.join("worktree").to_string_lossy().to_string();
        std::fs::create_dir_all(&cwd).expect("worktree");
        let db = seeded_db("usage-claude-basic", &cwd);
        homes.write_claude_transcript(
            &cwd,
            "sess-1",
            &[
                claude_turn("msg_1", "2026-04-17T09:30:00.000Z", &cwd, 40),
                claude_turn("msg_2", "2026-04-17T09:31:00.000Z", &cwd, 60),
            ],
        );

        let report = super::collect_repo_token_usage(&db, "repo-1").expect("collect");
        assert_eq!(report.files_scanned, 1);
        assert_eq!(report.records_written, 2);

        let summary = db.test_token_usage_summary().expect("summary");
        assert_eq!(summary.rows, 2);
        assert_eq!(summary.input, 10 + 10);
        assert_eq!(summary.cached_input, 90 + 90);
        assert_eq!(summary.cache_creation, 5 + 5);
        assert_eq!(summary.total, (10 + 90 + 5 + 40) + (10 + 90 + 5 + 60));
        assert_eq!(summary.task_id.as_deref(), Some("task-1"));
        assert_eq!(
            summary.run_id.as_deref(),
            Some("run-1"),
            "usage inside a run's window belongs to that run"
        );

        // Collecting again reads nothing new and changes no total.
        let again = super::collect_repo_token_usage(&db, "repo-1").expect("second collect");
        assert_eq!(again.records_written, 0);
        assert_eq!(db.count_test_token_usage_rows().expect("rows"), 2);
    }

    #[test]
    fn a_growing_transcript_is_appended_to_rather_than_reparsed() {
        let homes = ProviderHomes::new("claude-growth");
        let cwd = homes.root.join("worktree").to_string_lossy().to_string();
        std::fs::create_dir_all(&cwd).expect("worktree");
        let db = seeded_db("usage-claude-growth", &cwd);
        homes.write_claude_transcript(
            &cwd,
            "sess-1",
            &[claude_turn("msg_1", "2026-04-17T09:30:00.000Z", &cwd, 40)],
        );
        super::collect_repo_token_usage(&db, "repo-1").expect("first collect");

        homes.append_claude_transcript(
            &cwd,
            "sess-1",
            &claude_turn("msg_2", "2026-04-17T09:31:00.000Z", &cwd, 60),
        );
        let report = super::collect_repo_token_usage(&db, "repo-1").expect("second collect");
        assert_eq!(
            report.records_written, 1,
            "only the appended turn should have been parsed"
        );
        assert_eq!(db.count_test_token_usage_rows().expect("rows"), 2);
    }

    #[test]
    fn a_fork_that_copies_a_transcript_does_not_double_the_totals() {
        let homes = ProviderHomes::new("claude-fork");
        let cwd = homes.root.join("worktree").to_string_lossy().to_string();
        std::fs::create_dir_all(&cwd).expect("worktree");
        let db = seeded_db("usage-claude-fork", &cwd);
        let turn = claude_turn("msg_1", "2026-04-17T09:30:00.000Z", &cwd, 40);
        homes.write_claude_transcript(&cwd, "sess-1", std::slice::from_ref(&turn));
        // A resumed session writes a new file carrying the same history.
        homes.write_claude_transcript(&cwd, "sess-2", &[turn]);

        super::collect_repo_token_usage(&db, "repo-1").expect("collect");
        assert_eq!(
            db.count_test_token_usage_rows().expect("rows"),
            1,
            "one turn copied into two files is still one turn"
        );
    }

    #[test]
    fn nested_claude_subagents_are_collected_and_copied_history_is_deduplicated() {
        let homes = ProviderHomes::new("claude-subagents");
        let cwd = homes.root.join("worktree").to_string_lossy().to_string();
        std::fs::create_dir_all(&cwd).expect("worktree");
        let db = seeded_db("usage-claude-subagents", &cwd);
        let parent = claude_turn("msg_parent", "2026-04-17T09:30:00.000Z", &cwd, 40);
        let delegated = claude_turn("msg_delegated", "2026-04-17T09:31:00.000Z", &cwd, 60);
        homes.write_claude_transcript(&cwd, "sess-1", std::slice::from_ref(&parent));
        homes.write_claude_subagent(&cwd, "sess-1", "agent-a", &[parent, delegated.clone()]);
        homes.write_claude_subagent(&cwd, "sess-1", "agent-b", &[delegated]);

        let report = super::collect_repo_token_usage(&db, "repo-1").expect("collect");
        assert_eq!(report.files_scanned, 3);
        assert_eq!(db.count_test_token_usage_rows().expect("rows"), 2);
        assert!(report.providers_without_usage.is_empty());

        let again = super::collect_repo_token_usage(&db, "repo-1").expect("repeat collect");
        assert_eq!(again.records_written, 0);
        assert_eq!(db.count_test_token_usage_rows().expect("rows"), 2);
    }

    #[test]
    fn a_transcript_from_another_project_is_never_read() {
        let homes = ProviderHomes::new("claude-foreign");
        let cwd = homes.root.join("worktree").to_string_lossy().to_string();
        std::fs::create_dir_all(&cwd).expect("worktree");
        let db = seeded_db("usage-claude-foreign", &cwd);
        let elsewhere = homes.root.join("elsewhere").to_string_lossy().to_string();
        homes.write_claude_transcript(
            &elsewhere,
            "sess-9",
            &[claude_turn(
                "msg_9",
                "2026-04-17T09:30:00.000Z",
                &elsewhere,
                40,
            )],
        );

        let report = super::collect_repo_token_usage(&db, "repo-1").expect("collect");
        assert_eq!(report.files_scanned, 0);
        assert_eq!(db.count_test_token_usage_rows().expect("rows"), 0);
    }

    #[test]
    fn a_codex_rollout_in_a_task_worktree_counts_each_turn_once() {
        let homes = ProviderHomes::new("codex-basic");
        let cwd = homes.root.join("worktree").to_string_lossy().to_string();
        std::fs::create_dir_all(&cwd).expect("worktree");
        let db = seeded_db("usage-codex-basic", &cwd);
        db.insert_test_provider_stage_run(
            "run-codex",
            "task-1",
            "review",
            "codex",
            &cwd,
            "2026-04-17 09:00:00",
            Some("2026-04-17 11:00:00"),
        )
        .expect("codex run");
        db.set_test_stage_run_provider_session_id("run-codex", "s-1")
            .expect("provider session");
        let meta = format!(
            r#"{{"type":"session_meta","payload":{{"session_id":"s-1","id":"s-1","cwd":"{cwd}"}}}}"#
        );
        let turn_context = r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#.to_string();
        let first = r#"{"timestamp":"2026-04-17T09:30:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":50,"reasoning_output_tokens":5,"total_tokens":1050},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"cache_write_input_tokens":0,"output_tokens":50,"reasoning_output_tokens":5,"total_tokens":1050}}}}"#.to_string();
        // The running total grows; only the per-turn block may be counted.
        let second = r#"{"timestamp":"2026-04-17T09:31:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":3000,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":90,"reasoning_output_tokens":9,"total_tokens":3090},"last_token_usage":{"input_tokens":2000,"cached_input_tokens":500,"cache_write_input_tokens":0,"output_tokens":40,"reasoning_output_tokens":4,"total_tokens":2040}}}}"#.to_string();
        homes.write_codex_rollout(
            "rollout-2026-04-17T09-30-00-s-1",
            &[meta, turn_context, first, second],
        );

        super::collect_repo_token_usage(&db, "repo-1").expect("collect");
        let summary = db.test_token_usage_summary().expect("summary");
        assert_eq!(summary.rows, 2);
        // Codex counts cache reads inside input, so the fresh input is
        // (1000 - 200) + (2000 - 500).
        assert_eq!(summary.input, 800 + 1_500);
        assert_eq!(summary.cached_input, 200 + 500);
        assert_eq!(
            summary.output,
            50 + 40,
            "the running total must not be summed"
        );
        assert_eq!(summary.model.as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn codex_discovery_is_bounded_to_run_context_and_cached_between_reads() {
        let homes = ProviderHomes::new("codex-bounded");
        let cwd = homes.root.join("worktree").to_string_lossy().to_string();
        std::fs::create_dir_all(&cwd).expect("worktree");
        let db = seeded_db("usage-codex-bounded", &cwd);
        db.insert_test_provider_stage_run(
            "run-codex",
            "task-1",
            "review",
            "codex",
            &cwd,
            "2026-04-17 09:00:00",
            Some("2026-04-17 11:00:00"),
        )
        .expect("codex run");
        db.set_test_stage_run_provider_session_id("run-codex", "s-target")
            .expect("provider session");
        for index in 0..250 {
            homes.write_codex_rollout(
                &format!("unrelated-{index}"),
                &[format!(
                    r#"{{"type":"session_meta","payload":{{"id":"other-{index}","cwd":"/unrelated/{index}"}}}}"#
                )],
            );
        }
        let meta =
            format!(r#"{{"type":"session_meta","payload":{{"id":"s-target","cwd":"{cwd}"}}}}"#);
        let usage = r#"{"timestamp":"2026-04-17T09:30:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":15}}}}"#.to_string();
        homes.write_codex_rollout("target", &[meta, usage]);

        super::CODEX_DISCOVERY_INSPECTIONS.store(0, std::sync::atomic::Ordering::Relaxed);
        super::collect_repo_token_usage(&db, "repo-1").expect("first collect");
        let first = super::CODEX_DISCOVERY_INSPECTIONS.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(first, 251);
        super::collect_repo_token_usage(&db, "repo-1").expect("second collect");
        assert_eq!(
            super::CODEX_DISCOVERY_INSPECTIONS.load(std::sync::atomic::Ordering::Relaxed),
            first,
            "an unchanged candidate directory must reuse its discovery checkpoint"
        );
        assert_eq!(db.count_test_token_usage_rows().expect("rows"), 1);
    }

    #[test]
    fn missing_and_unreadable_supported_provider_files_are_reported() {
        let homes = ProviderHomes::new("supported-missing");
        let cwd = homes.root.join("worktree").to_string_lossy().to_string();
        std::fs::create_dir_all(&cwd).expect("worktree");
        let db = seeded_db("usage-supported-missing", &cwd);

        let missing = super::collect_repo_token_usage(&db, "repo-1").expect("missing collect");
        assert_eq!(missing.providers_without_usage, vec!["claude".to_string()]);

        let project = homes
            .root
            .join("claude/projects")
            .join(crate::task_creator::claude_project_slug(&cwd));
        std::fs::create_dir_all(project.join("unreadable.jsonl")).expect("directory fixture");
        let unreadable =
            super::collect_repo_token_usage(&db, "repo-1").expect("unreadable collect");
        assert_eq!(
            unreadable.providers_without_usage,
            vec!["claude".to_string()]
        );
    }

    #[test]
    fn a_provider_whose_usage_cannot_be_read_is_reported_rather_than_shown_as_zero() {
        let homes = ProviderHomes::new("unsupported");
        let cwd = homes.root.join("worktree").to_string_lossy().to_string();
        std::fs::create_dir_all(&cwd).expect("worktree");
        let db = seeded_db("usage-unsupported", &cwd);
        db.insert_test_provider_stage_run(
            "run-2",
            "task-1",
            "review",
            "opencode",
            &cwd,
            "2026-04-17 12:00:00",
            None,
        )
        .expect("stage run");

        let report = super::collect_repo_token_usage(&db, "repo-1").expect("collect");
        assert_eq!(
            report.providers_without_usage,
            vec!["claude".to_string(), "opencode".to_string()],
            "a CLI Kanna cannot read usage for is a hole, not a zero"
        );
    }
}
