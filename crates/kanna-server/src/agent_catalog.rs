use kanna_agent_protocol::AgentProvider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const BUNDLED: &str = include_str!("../resources/agent-catalog.json");
const OVERRIDE_FILE: &str = "agent-catalog.json";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelDefinition {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessDefinition {
    pub models: Vec<ModelDefinition>,
    pub efforts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCatalog {
    pub version: u32,
    pub harnesses: HashMap<AgentProvider, HarnessDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

fn validate(catalog: &AgentCatalog) -> Result<(), String> {
    if catalog.version != 1 {
        return Err(format!(
            "unsupported agent catalog version {}",
            catalog.version
        ));
    }
    for provider in AgentProvider::ALL {
        let definition = catalog
            .harnesses
            .get(&provider)
            .ok_or_else(|| format!("catalog omits harness '{provider}'"))?;
        let mut models = std::collections::HashSet::new();
        for model in &definition.models {
            if model.id.trim() != model.id || model.id.is_empty() || model.label.trim().is_empty() {
                return Err(format!(
                    "harness '{provider}' has an invalid model definition"
                ));
            }
            if !models.insert(model.id.as_str()) {
                return Err(format!("harness '{provider}' repeats model '{}'", model.id));
            }
        }
        if definition
            .efforts
            .iter()
            .any(|value| value.is_empty() || value.trim() != value)
        {
            return Err(format!(
                "harness '{provider}' has an invalid effort definition"
            ));
        }
    }
    Ok(())
}

fn parse(contents: &str, source: &str) -> Result<AgentCatalog, String> {
    let mut catalog: AgentCatalog = serde_json::from_str(contents)
        .map_err(|error| format!("failed to parse {source}: {error}"))?;
    validate(&catalog)?;
    catalog.source = Some(source.to_string());
    Ok(catalog)
}

pub fn override_path() -> PathBuf {
    std::env::var_os("KANNA_AGENT_CATALOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            kanna_runtime_defaults::daemon_dir_for_current_runtime().join(OVERRIDE_FILE)
        })
}

fn load_from(path: &std::path::Path) -> AgentCatalog {
    if path.exists() {
        match std::fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))
            .and_then(|contents| parse(&contents, "override"))
        {
            Ok(catalog) => return catalog,
            Err(error) => log::warn!("[agent-catalog] {error}; using bundled definitions"),
        }
    }
    parse(BUNDLED, "bundled").expect("bundled agent catalog must be valid")
}

/// Read on every request so an atomic replacement becomes active without a
/// daemon or desktop restart. Invalid updates safely leave the bundle in use.
pub fn load() -> AgentCatalog {
    load_from(&override_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_covers_every_harness() {
        let catalog = parse(BUNDLED, "test bundle").unwrap();
        assert_eq!(catalog.harnesses.len(), AgentProvider::ALL.len());
    }

    #[test]
    fn malformed_catalog_is_refused() {
        let error = parse(r#"{"version":1,"harnesses":{}}"#, "fixture").unwrap_err();
        assert!(error.contains("omits harness 'claude'"));
    }

    #[test]
    fn replacement_is_observed_without_restarting() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(OVERRIDE_FILE);
        let mut replacement = parse(BUNDLED, "fixture").unwrap();
        replacement.source = None;
        replacement
            .harnesses
            .get_mut(&AgentProvider::Codex)
            .unwrap()
            .models = vec![ModelDefinition {
            id: "future-model".into(),
            label: "Future model".into(),
        }];
        std::fs::write(&path, serde_json::to_vec(&replacement).unwrap()).unwrap();
        assert_eq!(
            load_from(&path).harnesses[&AgentProvider::Codex].models[0].id,
            "future-model"
        );

        replacement
            .harnesses
            .get_mut(&AgentProvider::Codex)
            .unwrap()
            .models[0]
            .id = "newer-model".into();
        std::fs::write(&path, serde_json::to_vec(&replacement).unwrap()).unwrap();
        assert_eq!(
            load_from(&path).harnesses[&AgentProvider::Codex].models[0].id,
            "newer-model"
        );
    }
}
