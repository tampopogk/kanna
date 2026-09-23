use std::env;
use std::process;

use serde_json::Value;

use crate::api::complete_stage_via_api;
use crate::commands::parse_metadata_json;
use crate::config::resolve_server_base_url;
use crate::models::CompleteStageRequest;
use kanna_runtime_defaults::stage_verdict::StageVerdict;

pub(crate) fn build_complete_stage_request(
    run_id: Option<String>,
    completion_attempt_key: Option<String>,
    status: String,
    summary: String,
    metadata: Option<Value>,
    workflow_definition: Option<Value>,
    expected_definition: Option<Value>,
) -> CompleteStageRequest {
    CompleteStageRequest {
        run_id,
        completion_attempt_key,
        status,
        summary,
        metadata,
        workflow_definition,
        expected_definition,
        exit: None,
    }
}

pub(crate) fn render_stage_complete_confirmation(
    task_id: &str,
    status: &str,
    response_task_id: &str,
) -> String {
    if response_task_id != task_id {
        return format!(
            "Stage completion recorded for task {task_id} (status: {status}); advanced to task {response_task_id}."
        );
    }

    format!("Stage completion recorded for task {task_id} (status: {status}).")
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    task_id: String,
    status: String,
    summary: String,
    metadata: Option<String>,
    workflow_definition: Option<String>,
    expected_definition: Option<String>,
    exit: Option<String>,
    server_url: Option<&str>,
) {
    // Validated here against the shared vocabulary table, before anything is
    // sent, so a guessed word is refused with the whole list rather than
    // travelling to the server to come back as a 400.
    if let Err(refusal) = StageVerdict::parse(&status) {
        eprintln!("Error: --{refusal}");
        process::exit(1);
    }
    let metadata_value = parse_metadata_json(&metadata).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    // Refused here rather than at the server, so a planner that sent only one
    // half learns it before a verdict is recorded.
    if workflow_definition.is_some() != expected_definition.is_some() {
        eprintln!("Error: --workflow-definition and --expected-definition must be passed together");
        process::exit(1);
    }
    let parse_definition = |raw: Option<String>, flag: &str| -> Option<Value> {
        raw.map(|raw| {
            serde_json::from_str::<Value>(&raw).unwrap_or_else(|error| {
                eprintln!("Error: {flag} must be a JSON object: {error}");
                process::exit(1);
            })
        })
    };
    let workflow_definition = parse_definition(workflow_definition, "--workflow-definition");
    let expected_definition = parse_definition(expected_definition, "--expected-definition");

    let env_pairs = env::vars().collect::<Vec<_>>();
    let borrowed_pairs = env_pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let base_url = resolve_server_base_url(&borrowed_pairs, server_url);
    let mut request = build_complete_stage_request(
        None,
        None,
        status.clone(),
        summary.clone(),
        metadata_value,
        workflow_definition,
        expected_definition,
    );
    request.exit = exit;
    bind_completion_request(&base_url, &task_id, &mut request)
        .await
        .unwrap_or_else(|error| {
            eprintln!("Error: {error}");
            process::exit(1);
        });
    let response = complete_stage_via_api(&base_url, &task_id, &request)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });
    println!(
        "{}",
        render_stage_complete_confirmation(&task_id, &status, &response.task_id)
    );
    if let Some(message) = response
        .routing
        .as_ref()
        .and_then(|routing| routing.get("message"))
        .and_then(Value::as_str)
    {
        println!("{message}");
    }
    if request.workflow_definition.is_some() {
        // An older server ignores the arguments and answers without the flag;
        // saying so is the difference between a published workflow and a plan
        // whose stages silently do not exist.
        match response.workflow_extended {
            Some(true) => println!("The task's remaining stages were published with this plan."),
            _ => {
                eprintln!(
                    "Error: this server did not confirm the workflow extension \
                     (no workflowExtended in its response); the remaining stages were NOT published."
                );
                process::exit(1);
            }
        }
    }
}

async fn bind_completion_request(
    _base_url: &str,
    _task_id: &str,
    request: &mut CompleteStageRequest,
) -> Result<(), String> {
    let body = serde_json::to_value(&*request)
        .map_err(|error| format!("failed to encode completion request: {error}"))?;
    let attempt_key = kanna_tool_catalog::completion_attempt_key(&body)?;
    if let Some(path) = env::var_os(kanna_tool_catalog::KANNA_COMPLETION_CONTEXT_ENV) {
        let path = std::path::PathBuf::from(path);
        let context = kanna_tool_catalog::read_completion_context(&path)?;
        request.run_id = Some(
            context
                .run_for_attempt(&attempt_key)
                .unwrap_or(&context.run_id)
                .to_string(),
        );
    } else if let Ok(run_id) = env::var(kanna_tool_catalog::KANNA_STAGE_RUN_ID_ENV) {
        if !run_id.trim().is_empty() {
            request.run_id = Some(run_id);
        }
    }
    request.completion_attempt_key = Some(attempt_key);
    Ok(())
}
