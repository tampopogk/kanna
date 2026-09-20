use std::process;

use crate::api::{
    add_repo_via_api, clear_standing_constraint_via_api, eject_agent_via_api, get_task_via_api,
    list_repo_agents_via_api, list_repos_via_api, list_standing_constraints_via_api,
    reconcile_repo_metadata_via_api, set_standing_constraint_via_api, show_agent_via_api,
    signal_agent_via_api,
};
use crate::commands::print_json;
use crate::config::resolve_guide_task_id;
use crate::config::resolve_server_base_url_from_env;
use crate::models::{
    AddRepoRequest, ClearStandingConstraintRequest, EjectAgentRequest,
    ReconcileRepoMetadataRequest, SetStandingConstraintRequest, SignalAgentRequest,
};
use crate::{RepoAgentCommands, RepoCommands, RepoConstraintCommands};

pub(crate) fn build_add_repo_request(path: String, name: Option<String>) -> AddRepoRequest {
    AddRepoRequest { path, name }
}

pub(crate) fn build_reconcile_repo_metadata_request(apply: bool) -> ReconcileRepoMetadataRequest {
    ReconcileRepoMetadataRequest { apply }
}

pub(crate) fn build_signal_agent_request(
    message: String,
    agent_provider: Option<String>,
    effort: Option<String>,
) -> SignalAgentRequest {
    SignalAgentRequest {
        message,
        agent_provider,
        effort,
    }
}

pub(crate) async fn run(command: RepoCommands) {
    match command {
        RepoCommands::List { server_url } => {
            let base_url = resolve_server_base_url_from_env(server_url.as_deref());
            let repos = list_repos_via_api(&base_url).await.unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                process::exit(1);
            });
            if let Err(e) = print_json(&repos) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        RepoCommands::Add {
            path,
            name,
            server_url,
        } => {
            let base_url = resolve_server_base_url_from_env(server_url.as_deref());
            let request = build_add_repo_request(path, name);
            let repo = add_repo_via_api(&base_url, &request)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("Error: {e}");
                    process::exit(1);
                });
            if let Err(e) = print_json(&repo) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        RepoCommands::ReconcileMetadata {
            repo_id,
            apply,
            server_url,
        } => {
            let base_url = resolve_server_base_url_from_env(server_url.as_deref());
            let request = build_reconcile_repo_metadata_request(apply);
            let response = reconcile_repo_metadata_via_api(&base_url, &repo_id, &request)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("Error: {e}");
                    process::exit(1);
                });
            if let Err(e) = print_json(&response) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        RepoCommands::Agent { command } => match command {
            RepoAgentCommands::List {
                repo_id,
                server_url,
            } => {
                let base_url = resolve_server_base_url_from_env(server_url.as_deref());
                let agents = list_repo_agents_via_api(&base_url, &repo_id)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("Error: {e}");
                        process::exit(1);
                    });
                if let Err(e) = print_json(&agents) {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            }
            RepoAgentCommands::Signal {
                repo_id,
                agent,
                message,
                agent_provider,
                effort,
                server_url,
            } => {
                let base_url = resolve_server_base_url_from_env(server_url.as_deref());
                let request = build_signal_agent_request(message, agent_provider, effort);
                let response = signal_agent_via_api(&base_url, &repo_id, &agent, &request)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("Error: {e}");
                        process::exit(1);
                    });
                if let Err(e) = print_json(&response) {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            }
            RepoAgentCommands::Show {
                repo_id,
                agent,
                raw,
                server_url,
            } => {
                let base_url = resolve_server_base_url_from_env(server_url.as_deref());
                let repo_id = resolve_default_repo_id(&base_url, repo_id).await;
                let definition = show_agent_via_api(&base_url, &repo_id, &agent, raw)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("Error: {e}");
                        process::exit(1);
                    });
                if let Err(e) = print_json(&definition) {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            }
            RepoAgentCommands::Eject {
                repo_id,
                agent,
                force,
                server_url,
            } => {
                let base_url = resolve_server_base_url_from_env(server_url.as_deref());
                let repo_id = resolve_default_repo_id(&base_url, repo_id).await;
                let request = EjectAgentRequest { force };
                let result = eject_agent_via_api(&base_url, &repo_id, &agent, &request)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("Error: {e}");
                        process::exit(1);
                    });
                if let Err(e) = print_json(&result) {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            }
        },
        RepoCommands::Constraint { command } => run_constraint(command).await,
    }
}

/// Resolve the repository a call is about, when the CLI accepts an optional
/// `--repo-id`.
///
/// Repository defaulting is shared tool policy (`repo_context_task_id`), so the
/// typed CLI resolves it the same way the catalog clients do: an explicit
/// `--repo-id` wins, otherwise the calling task session's repository, otherwise
/// the caller is told plainly rather than sending a request that cannot name a
/// repository.
async fn resolve_default_repo_id(base_url: &str, repo_id: Option<String>) -> String {
    if let Some(repo_id) = repo_id.filter(|repo_id| !repo_id.trim().is_empty()) {
        return repo_id;
    }
    let env_pairs = std::env::vars().collect::<Vec<_>>();
    let borrowed = env_pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let Some(task_id) = resolve_guide_task_id(&borrowed) else {
        eprintln!("Error: --repo-id is required when KANNA_TASK_ID is not available");
        process::exit(1);
    };
    let task = get_task_via_api(base_url, &task_id)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Error: failed to infer repo_id from KANNA_TASK_ID={task_id}: {e}");
            process::exit(1);
        });
    task.repo_id
}

async fn run_constraint(command: RepoConstraintCommands) {
    match command {
        RepoConstraintCommands::List {
            repo_id,
            include_cleared,
            tail,
            server_url,
        } => {
            let base_url = resolve_server_base_url_from_env(server_url.as_deref());
            let repo_id = resolve_default_repo_id(&base_url, repo_id).await;
            let constraints =
                list_standing_constraints_via_api(&base_url, &repo_id, include_cleared, tail)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("Error: {e}");
                        process::exit(1);
                    });
            if let Err(e) = print_json(&constraints) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        RepoConstraintCommands::Set {
            repo_id,
            kind,
            text,
            subject_task_id,
            declared_by,
            declared_by_task_id,
            server_url,
        } => {
            let base_url = resolve_server_base_url_from_env(server_url.as_deref());
            let repo_id = resolve_default_repo_id(&base_url, repo_id).await;
            let request = SetStandingConstraintRequest {
                repo_id,
                kind,
                text,
                subject_task_id,
                declared_by,
                declared_by_task_id,
            };
            let response = set_standing_constraint_via_api(&base_url, &request)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("Error: {e}");
                    process::exit(1);
                });
            if let Err(e) = print_json(&response) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        RepoConstraintCommands::Clear {
            constraint_id,
            cleared_by,
            cleared_by_task_id,
            note,
            server_url,
        } => {
            let base_url = resolve_server_base_url_from_env(server_url.as_deref());
            let request = ClearStandingConstraintRequest {
                cleared_by,
                cleared_by_task_id,
                note,
            };
            let response = clear_standing_constraint_via_api(&base_url, &constraint_id, &request)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("Error: {e}");
                    process::exit(1);
                });
            if let Err(e) = print_json(&response) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
    }
}
