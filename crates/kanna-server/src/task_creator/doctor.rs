//! Static candidate validation. No Git, command execution, writes, or activation.
use super::*;
use crate::task_creator::provider;
use serde_json::{Map, Value};
use std::sync::LazyLock;

static CONFIG_SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema =
        serde_json::from_str(include_str!("../../../../.kanna/config.schema.json")).unwrap();
    jsonschema::validator_for(&schema).expect("bundled config schema")
});

// Candidate files may use the compatibility forms accepted by
// parse_workflow_definition. Keep the authoring schema's field/type checks,
// but admit those spellings before checking the parser's normalized workflow.
// Validating only a serialized WorkflowDefinition would lose unknown fields.
static WORKFLOW_CANDIDATE_SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let mut schema: Value =
        serde_json::from_str(include_str!("../../../../.kanna/workflows/schema.json")).unwrap();
    let stage = &mut schema["properties"]["stages"]["items"];
    stage["required"] = serde_json::json!(["name"]);
    let transition = stage["properties"]["policy"]["properties"]["transition"].clone();
    let execution = serde_json::json!({"enum": ["new_task", "continue"]});
    stage["properties"]["transition"] = transition.clone();
    stage["properties"]["mode"] = execution.clone();
    stage["properties"]["policy"]["properties"]["execution"] = execution;
    let mut post_action = stage["properties"]["post"].clone();
    post_action["required"] = serde_json::json!(["name"]);
    post_action["properties"]["transition"] = transition;
    stage["properties"]["post_action"] = post_action;
    jsonschema::validator_for(&schema).expect("compatible workflow candidate schema")
});

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Finding {
    file: String,
    location: String,
    problem: String,
    guidance: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DoctorReport {
    pub(crate) candidate_path: String,
    scope: &'static str,
    activation: &'static str,
    pub(crate) errors: Vec<Finding>,
    pub(crate) warnings: Vec<Finding>,
}

impl DoctorReport {
    fn error(&mut self, file: &str, location: &str, problem: impl ToString, guidance: &str) {
        self.errors.push(Finding {
            file: file.into(),
            location: location.into(),
            problem: problem.to_string(),
            guidance: guidance.into(),
        });
    }
    fn warning(&mut self, file: &str, location: &str, problem: impl ToString, guidance: &str) {
        self.warnings.push(Finding {
            file: file.into(),
            location: location.into(),
            problem: problem.to_string(),
            guidance: guidance.into(),
        });
    }
    fn schema(&mut self, file: &str, value: &Value, schema: &jsonschema::Validator) {
        for error in schema.iter_errors(value) {
            self.error(file, &error.instance_path().to_string(), &error, "Use the supported fields and value types in the running version's schema (kanna_guide).");
        }
    }
    fn entries(&mut self, definitions: &RepoDefinitions, directory: &str) -> Vec<String> {
        match definitions.snapshot.list_direct_entries(directory) {
            Ok(entries) => entries,
            Err(error) => {
                self.error(
                    directory,
                    "",
                    error,
                    "Use a directory containing definition files.",
                );
                Vec::new()
            }
        }
    }
    fn tuning(
        &mut self,
        file: &str,
        location: &str,
        providers: &[String],
        model: Option<&str>,
        effort: Option<&str>,
    ) {
        for name in providers {
            if let Err(error) = AgentProvider::from_str(name) {
                self.error(
                    file,
                    location,
                    error,
                    "Use a provider id from kanna_guide; model ids belong in model.",
                );
            }
        }
        let result = provider::validate_model_shape(model)
            .and_then(|()| provider::validate_effort_shape(effort))
            .and_then(|()| {
                if let Some(selected) = providers
                    .first()
                    .and_then(|name| AgentProvider::from_str(name).ok())
                {
                    provider::validate_provider_model(selected, model)?;
                    provider::validate_provider_effort(selected, effort)?;
                }
                Ok(())
            });
        if let Err(error) = result {
            self.error(file, location, error, "Keep model and effort coherent with the leading provider; fallback providers use their own defaults.");
        }
    }
}

pub(crate) fn check(root: &Path) -> DoctorReport {
    let mut report = DoctorReport {
        candidate_path: root.to_string_lossy().into_owned(),
        scope: "candidate files under .kanna; no commands executed or files modified; runtime behavior and arbitrary custom agent prose are not validated",
        activation: "Not an active-configuration check. Shared definitions resolve from origin's recorded default-branch snapshot, local overrides from the registered checkout, and existing tasks retain pinned workflows. Without an origin snapshot, shared candidate definitions await integration/activation.",
        errors: vec![], warnings: vec![],
    };
    let snapshot = match RepoDefinitionSnapshot::candidate(root) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            report.error(
                ".kanna",
                "",
                error,
                "Make candidate definitions readable regular files within .kanna.",
            );
            return report;
        }
    };
    let mut raw = Map::new();
    for file in [".kanna/config.json", ".kanna/config.local.json"] {
        match snapshot.read_optional_utf8(file) {
            Ok(Some(content)) => match serde_json::from_str::<Value>(&content) {
                Ok(mut value) => {
                    // Deprecated external spelling remains accepted by resolution.
                    if let Some(object) = value.as_object_mut() {
                        if let Some(legacy) = object.remove("pipeline") {
                            object.entry("workflow").or_insert(legacy);
                            report.warning(
                                file,
                                "/pipeline",
                                "Deprecated pipeline key",
                                "Rename pipeline to workflow.",
                            );
                        }
                    }
                    report.schema(file, &value, &CONFIG_SCHEMA);
                    if let Some(preferences) =
                        value.get("agentProviders").and_then(Value::as_object)
                    {
                        for (name, value) in preferences {
                            if let Some(preference) = parse_agent_provider_preference(value) {
                                report.tuning(
                                    file,
                                    &format!("/agentProviders/{name}"),
                                    &preference.providers,
                                    preference.model.as_deref(),
                                    preference.effort.as_deref(),
                                );
                            }
                        }
                    }
                    if file == ".kanna/config.json" {
                        raw = value.as_object().cloned().unwrap_or_default();
                    }
                }
                Err(error) => report.error(
                    file,
                    "",
                    error,
                    "Correct JSON syntax at the reported line/column.",
                ),
            },
            Ok(None) => {}
            Err(error) => report.error(file, "", error, "Provide a readable UTF-8 JSON object."),
        }
    }
    // Check both the portable configuration and the actual local-override merge,
    // so a local workflow preference cannot hide a broken shared selection.
    let shared_config = repo_config_from_object(&raw);
    let mut effective = raw.clone();
    if let Err(error) = apply_local_config_override(root, &mut effective) {
        report.error(
            ".kanna/config.local.json",
            "",
            error,
            "Use only supported machine-local fields; put portable semantics in config.json.",
        );
        effective = raw;
    }
    let definitions = RepoDefinitions {
        snapshot,
        config: repo_config_from_object(&effective),
    };
    for (file, config) in [
        (".kanna/config.json", &shared_config),
        (".kanna/config.local.json", &definitions.config),
    ] {
        if let Some(workflow) = &config.workflow {
            if let Err(error) = definitions.workflow(workflow) {
                report.error(file, "/workflow", error, "Select an existing built-in or provide .kanna/workflows/<name>.json; omit a deferred choice.");
            }
        }
    }
    let mut workflows = BTreeSet::new();
    for directory in [".kanna/pipelines", ".kanna/workflows"] {
        for name in report.entries(&definitions, directory) {
            if let Some(name) = name.strip_suffix(".json").filter(|name| *name != "schema") {
                let file = format!("{directory}/{name}.json");
                match definitions.snapshot.read_optional_utf8(&file) {
                    Ok(Some(content)) => match parse_workflow_definition(&content) {
                        Ok(workflow) => {
                            let raw: Value = serde_json::from_str(&content).unwrap();
                            report.schema(&file, &raw, &WORKFLOW_CANDIDATE_SCHEMA);
                            check_workflow(&mut report, &definitions, &file, &workflow);
                        }
                        Err(error) => report.error(&file, "", error, "Correct the workflow structure, transition, or provider selector using kanna_guide workflows."),
                    },
                    Err(error) => report.error(&file, "", error, "Provide a readable UTF-8 workflow JSON file."),
                    _ => {}
                }
                workflows.insert(name.to_string());
            }
        }
    }
    for name in [
        shared_config
            .workflow
            .as_deref()
            .unwrap_or(crate::task_creator::FALLBACK_WORKFLOW_NAME),
        definitions
            .config
            .workflow
            .as_deref()
            .unwrap_or(crate::task_creator::FALLBACK_WORKFLOW_NAME),
    ] {
        if workflows.insert(name.to_string()) {
            if let Ok(workflow) = definitions.workflow(name) {
                check_workflow(&mut report, &definitions, ".kanna/config.json", &workflow);
            }
        }
    }
    for name in report.entries(&definitions, ".kanna/agents") {
        for basename in ["AGENT.md", "EXTEND.md"] {
            let file = format!(".kanna/agents/{name}/{basename}");
            match definitions.snapshot.read_optional_utf8(&file) {
                Ok(Some(content)) => {
                    if content.trim_start_matches('\u{feff}').starts_with("---")
                        && split_frontmatter(&content).0.is_none()
                    {
                        report.warning(&file, "frontmatter", "No recognized frontmatter block; the parser treats this as body text", "Use matching --- delimiter lines with a newline after the closing delimiter, or remove an unintended opener.");
                    }
                    let parsed = if basename == "AGENT.md" {
                        parse_agent_definition(&content).map(|_| ())
                    } else {
                        parse_agent_extension(&content).map(|_| ())
                    };
                    if let Err(error) = parsed {
                        report.error(
                            &file,
                            "frontmatter",
                            error,
                            "Correct frontmatter using kanna_guide agents.",
                        );
                    }
                    check_agent(&mut report, &definitions, &file, "frontmatter", &name);
                }
                Err(error) => report.error(
                    &file,
                    "",
                    error,
                    "Provide a readable UTF-8 agent definition.",
                ),
                _ => {}
            }
        }
    }
    if let Some(flavors) = &definitions.config.flavors {
        let mut roles = flavors.keys().collect::<Vec<_>>();
        roles.sort();
        for role in roles {
            check_agent(
                &mut report,
                &definitions,
                ".kanna/config.json",
                &format!("/flavors/{role}"),
                role,
            );
        }
    }
    report.errors.sort_by(|a, b| {
        (&a.file, &a.location, &a.problem).cmp(&(&b.file, &b.location, &b.problem))
    });
    report
        .errors
        .dedup_by(|a, b| a.file == b.file && a.location == b.location && a.problem == b.problem);
    report.warnings.sort_by(|a, b| {
        (&a.file, &a.location, &a.problem).cmp(&(&b.file, &b.location, &b.problem))
    });
    report
        .warnings
        .dedup_by(|a, b| a.file == b.file && a.location == b.location && a.problem == b.problem);
    report
}

fn check_agent(
    report: &mut DoctorReport,
    definitions: &RepoDefinitions,
    file: &str,
    location: &str,
    selector: &str,
) {
    match definitions.agent(selector) {
        Ok(agent) => report.tuning(file, location, &agent.agent_providers, agent.model.as_deref(), agent.effort.as_deref()),
        Err(error) => report.error(file, location, error, "Name an existing agent or provide .kanna/agents/<name>/AGENT.md; an EXTEND.md needs a base agent."),
    }
    let selected = AgentSelector::resolve(selector, definitions.config.flavors.as_ref());
    if let Some(flavor) = selected.selected_flavor() {
        let path = format!(
            ".kanna/agents/{}/flavors/{flavor}/AGENT.md",
            canonical_builtin_agent_name(&selected.role)
        );
        if compiled_builtin_resource(&path).is_none() {
            report.warning(file, location, format!("Flavor `{flavor}` has no bundled definition; resolution uses the role's base/override"), "Choose an available built-in flavor or encode the intended behavior in a project agent/extension; repo flavor directories are not a resolution source.");
        }
    }
}

fn check_workflow(
    report: &mut DoctorReport,
    definitions: &RepoDefinitions,
    file: &str,
    workflow: &WorkflowDefinition,
) {
    let mut names = BTreeSet::new();
    let mut push_only = false;
    for (index, stage) in workflow.stages.iter().enumerate() {
        if let Some(environment) = &stage.environment {
            if !workflow
                .environments
                .as_ref()
                .is_some_and(|all| all.contains_key(environment))
            {
                report.error(
                    file,
                    &format!("/stages/{index}/environment"),
                    format!("Unknown environment `{environment}`"),
                    "Define it in environments or remove the reference.",
                );
            }
        }
        for (location, name, agent) in
            std::iter::once((format!("/stages/{index}"), &stage.name, &stage.agent)).chain(
                stage
                    .post
                    .iter()
                    .map(|post| (format!("/stages/{index}/post"), &post.name, &post.agent)),
            )
        {
            if !names.insert(name) {
                report.error(
                    file,
                    &location,
                    format!("Duplicate stage/post name `{name}`"),
                    "Give every stage and post a unique name.",
                );
            }
            if let Some(selector) = agent {
                check_agent(
                    report,
                    definitions,
                    file,
                    &format!("{location}/agent"),
                    selector,
                );
                if let Ok(agent) = definitions.agent(selector) {
                    let stock_push =
                        compiled_builtin_resource(".kanna/agents/pr/flavors/push-only/AGENT.md")
                            .and_then(|text| parse_agent_definition(text).ok());
                    let stock_approve = compiled_builtin_resource(".kanna/agents/approve/AGENT.md")
                        .and_then(|text| parse_agent_definition(text).ok());
                    if stock_push.is_some_and(|stock| stock.prompt == agent.prompt) {
                        push_only = true;
                    }
                    if push_only && stock_approve.is_some_and(|stock| stock.prompt == agent.prompt)
                    {
                        report.error(file, &location, "Built-in push-only publishing creates no PR but built-in approve requires a PR", "Remove/replace approve or choose PR publishing. Static checks cannot prove custom agent prose correct.");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> tempfile::TempDir {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
        std::fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }
    fn write(root: &Path, file: &str, content: impl AsRef<str>) {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content.as_ref()).unwrap();
    }
    fn problems(report: &DoctorReport) -> String {
        serde_json::to_string(report).unwrap()
    }

    #[test]
    fn doctor_allows_deferrals_and_never_executes_commands_or_writes_files() {
        let root = fixture();
        assert!(check(root.path()).errors.is_empty());
        let config =
            json!({"setup": ["touch doctor-must-not-run"], "test": ["exit 1"]}).to_string();
        write(root.path(), ".kanna/config.json", &config);
        let report = check(root.path());
        assert!(report.errors.is_empty(), "{}", problems(&report));
        assert!(!root.path().join("doctor-must-not-run").exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join(".kanna/config.json")).unwrap(),
            config
        );
        assert!(report
            .activation
            .contains("Not an active-configuration check"));
    }

    #[test]
    fn doctor_resolves_custom_agents_extensions_hidden_workflows_and_builtins_without_git() {
        let root = fixture();
        write(
            root.path(),
            ".kanna/config.json",
            r#"{"workflow":"custom","agentProviders":{"*":{"provider":["opencode","codex"],"model":"local/unlisted-model:30b"}}}"#,
        );
        write(root.path(), ".kanna/workflows/custom.json", json!({"name":"custom","visibility":"internal","stages":[{"name":"configure","agent":"custom","policy":{"transition":"manual"},"post":{"name":"save","agent":"commit","prompt":"Save changes"}}]}).to_string());
        write(root.path(), ".kanna/agents/custom/AGENT.md", "---\nname: custom\ndescription: Project-specific agent\nagent_provider: codex\nmodel: future-model\neffort: high\n---\nProject policy.");
        write(
            root.path(),
            ".kanna/agents/custom/EXTEND.md",
            "Additional project instructions.",
        );
        let report = check(root.path());
        assert!(report.errors.is_empty(), "{}", problems(&report));
        assert!(report.warnings.is_empty(), "{}", problems(&report));
    }

    #[test]
    fn doctor_accepts_legacy_workflows_like_normal_resolution() {
        let canonical = json!({"name":"legacy","stages":[{
            "name":"work","agent":"implement","policy":{"transition":"manual"}
        }]});
        let mut with_post = canonical.clone();
        with_post["stages"][0]["post"] =
            json!({"name":"save","agent":"commit","prompt":"Save changes"});
        let cases = [
            (
                json!({"name":"legacy","stages":[{"name":"work","agent":"implement","transition":"manual"}]}),
                canonical,
            ),
            (
                json!({"name":"legacy","stages":[{"name":"work","agent":"implement","transition":"manual","mode":"new_task","post_action":{"name":"save","agent":"commit","prompt":"Save changes","transition":"auto"}}]}),
                with_post.clone(),
            ),
            (
                json!({"name":"legacy","stages":[{"name":"work","agent":"implement","transition":"manual"},{"name":"save","agent":"commit","prompt":"Save changes","transition":"auto","mode":"continue"}]}),
                with_post.clone(),
            ),
            (
                json!({"name":"legacy","stages":[{"name":"work","agent":"implement","policy":{"transition":"manual"}},{"name":"save","agent":"commit","prompt":"Save changes","policy":{"transition":"auto","execution":"continue"}}]}),
                with_post,
            ),
        ];
        for (legacy, canonical) in cases {
            let root = fixture();
            let mut resolved = Vec::new();
            for candidate in [legacy, canonical] {
                write(
                    root.path(),
                    ".kanna/workflows/legacy.json",
                    candidate.to_string(),
                );
                write(
                    root.path(),
                    ".kanna/config.json",
                    r#"{"workflow":"legacy"}"#,
                );
                let definitions = RepoDefinitions {
                    snapshot: RepoDefinitionSnapshot::candidate(root.path()).unwrap(),
                    config: RepoConfig::default(),
                };
                resolved
                    .push(serde_json::to_value(definitions.workflow("legacy").unwrap()).unwrap());
                let report = check(root.path());
                assert!(report.errors.is_empty(), "{}", problems(&report));
            }
            assert_eq!(resolved[0], resolved[1]);
        }
    }

    #[test]
    fn doctor_legacy_compatibility_keeps_schema_and_parser_errors() {
        for stage in [
            json!({"name":"work","transition":"sometimes"}),
            json!({"name":"work","transition":"manual","mode":"sometimes"}),
            json!({"name":"work","transition":"manual","unknown":true}),
            json!({"name":"work","policy":{"transition":"manual","unknown":true}}),
            json!({"name":"work","transition":"manual","agent_provider":"unknown-model"}),
            json!({"name":"work","transition":"manual","post_action":{"name":"save","transition":"sometimes"}}),
            json!({"name":"work","transition":"manual","post_action":{"name":"save","unknown":true}}),
            json!({"name":"work","transition":"manual","post_action":{"name":"save","agent_provider":"antigravity-model"}}),
            json!({"name":"work"}),
        ] {
            let root = fixture();
            let value = json!({"name":"invalid","stages":[stage]});
            write(
                root.path(),
                ".kanna/workflows/invalid.json",
                value.to_string(),
            );
            let report = check(root.path());
            assert!(!report.errors.is_empty(), "accepted {value}");
        }
    }

    #[test]
    fn doctor_reports_json_frontmatter_local_policy_and_provider_errors() {
        let root = fixture();
        write(
            root.path(),
            ".kanna/config.json",
            r#"{"unknown":true,"agentProviders":{"*":{"provider":"antigravity","model":"not-supported"}}}"#,
        );
        write(
            root.path(),
            ".kanna/config.local.json",
            r#"{"flavors":{"pr":"push-only"}}"#,
        );
        write(
            root.path(),
            ".kanna/agents/broken/AGENT.md",
            "---\nname: [wrong]\n---\nBroken",
        );
        write(root.path(), ".kanna/workflows/broken.json", "{");
        let report = check(root.path());
        for file in [
            ".kanna/config.json",
            ".kanna/config.local.json",
            ".kanna/agents/broken/AGENT.md",
            ".kanna/workflows/broken.json",
        ] {
            assert!(
                report.errors.iter().any(|finding| finding.file == file),
                "{}",
                problems(&report)
            );
        }
        assert!(report
            .errors
            .iter()
            .all(|finding| !finding.guidance.is_empty()));
    }

    #[test]
    fn doctor_catches_missing_references_and_workflow_structure() {
        let root = fixture();
        write(
            root.path(),
            ".kanna/config.json",
            r#"{"workflow":"missing"}"#,
        );
        write(
            root.path(),
            ".kanna/agents/orphan/EXTEND.md",
            "No base definition exists.",
        );
        write(root.path(), ".kanna/workflows/custom.json", json!({"name":"custom","stages":[{"name":"same","agent":"missing","environment":"absent","policy":{"transition":"manual"},"post":{"name":"same","agent":"commit"}}]}).to_string());
        let report = check(root.path());
        for problem in [
            "missing",
            "orphan",
            "Unknown environment",
            "Duplicate stage/post",
        ] {
            assert!(problems(&report).contains(problem), "{}", problems(&report));
        }
    }

    #[test]
    fn doctor_checks_shared_selection_even_when_local_overrides_it() {
        let root = fixture();
        write(
            root.path(),
            ".kanna/config.json",
            r#"{"workflow":"missing","ports":{"APP_PORT":3100}}"#,
        );
        write(
            root.path(),
            ".kanna/config.local.json",
            r#"{"workflow":"repository-setup","ports":{"APP_PORT":3200},"test":[]}"#,
        );
        let report = check(root.path());
        assert!(
            report
                .errors
                .iter()
                .any(|finding| finding.file == ".kanna/config.json"
                    && finding.location == "/workflow")
        );
        assert!(
            !report
                .errors
                .iter()
                .any(|finding| finding.file == ".kanna/config.local.json"),
            "{}",
            problems(&report)
        );
    }

    #[test]
    fn doctor_flags_stock_push_only_approval_but_does_not_judge_custom_prose() {
        let root = fixture();
        write(
            root.path(),
            ".kanna/config.json",
            r#"{"workflow":"no-review","flavors":{"pr":"push-only"}}"#,
        );
        let report = check(root.path());
        assert!(
            problems(&report).contains("creates no PR"),
            "{}",
            problems(&report)
        );
        write(root.path(), ".kanna/agents/approve/AGENT.md", "---\nname: approve\ndescription: Project approval without a forge PR\n---\nRecord local approval.");
        let report = check(root.path());
        assert!(report.errors.is_empty(), "{}", problems(&report));
    }

    #[test]
    fn doctor_warns_when_flavor_falls_back_and_rejects_invalid_transition_and_effort() {
        let root = fixture();
        write(
            root.path(),
            ".kanna/config.json",
            r#"{"flavors":{"pr":"typo"},"agentProviders":{"*":{"provider":"claude","effort":"imaginary"}}}"#,
        );
        write(
            root.path(),
            ".kanna/workflows/invalid.json",
            r#"{"name":"invalid","stages":[{"name":"a","policy":{"transition":"sometimes"}}]}"#,
        );
        let report = check(root.path());
        assert!(report
            .warnings
            .iter()
            .any(|finding| finding.problem.contains("typo")));
        assert!(report
            .errors
            .iter()
            .any(|finding| finding.location.contains("agentProviders")));
        assert!(report
            .errors
            .iter()
            .any(|finding| finding.file.ends_with("invalid.json")));
    }

    #[test]
    fn setup_alias_and_manual_workflow_resolve_without_product_posts() {
        let root = fixture();
        let definitions = RepoDefinitions {
            snapshot: RepoDefinitionSnapshot::candidate(root.path()).unwrap(),
            config: RepoConfig::default(),
        };
        assert_eq!(
            definitions.agent("setup").unwrap().prompt,
            definitions.agent("config-factory").unwrap().prompt
        );
        assert!(!definitions
            .agents()
            .unwrap()
            .iter()
            .any(|agent| agent.name == "config-factory"));
        let workflow = definitions.workflow("repository-setup").unwrap();
        assert_eq!(workflow.stages.len(), 1);
        assert_eq!(
            workflow.stages[0].policy.transition,
            WorkflowStageTransition::Manual
        );
        assert!(workflow.stages[0].post.is_none());
        assert!(!definitions
            .workflow_names()
            .unwrap()
            .contains(&"repository-setup".into()));
        write(
            root.path(),
            ".kanna/agents/config-factory/EXTEND.md",
            "Preserved legacy project customization.",
        );
        let definitions = RepoDefinitions {
            snapshot: RepoDefinitionSnapshot::candidate(root.path()).unwrap(),
            config: RepoConfig::default(),
        };
        assert!(definitions
            .agent("setup")
            .unwrap()
            .prompt
            .contains("Preserved legacy"));
    }
    #[test]
    fn doctor_reports_wrong_directory_shapes_and_unrecognized_extension_frontmatter() {
        let root = fixture();
        write(root.path(), ".kanna/workflows", "not a directory");
        write(
            root.path(),
            ".kanna/agents/setup/EXTEND.md",
            "---\nmodel: lost-without-closing-delimiter",
        );
        let report = check(root.path());
        assert!(report
            .errors
            .iter()
            .any(|finding| finding.file == ".kanna/workflows"));
        assert!(report
            .warnings
            .iter()
            .any(|finding| finding.problem.contains("No recognized frontmatter")));
    }

    #[test]
    fn doctor_checks_the_bundled_setup_candidate_with_authoritative_schemas() {
        let root = fixture();
        write(
            root.path(),
            ".kanna/workflows/repository-setup.json",
            compiled_builtin_resource(".kanna/workflows/repository-setup.json").unwrap(),
        );
        write(
            root.path(),
            ".kanna/agents/setup/AGENT.md",
            compiled_builtin_resource(".kanna/agents/setup/AGENT.md").unwrap(),
        );
        write(
            root.path(),
            ".kanna/config.json",
            r#"{"workflow":"repository-setup"}"#,
        );
        let report = check(root.path());
        assert!(report.errors.is_empty(), "{}", problems(&report));
        assert!(report.warnings.is_empty(), "{}", problems(&report));
    }
}
