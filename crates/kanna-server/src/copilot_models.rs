//! Read Copilot's locally recorded recent model IDs without exposing its
//! configuration, which can contain authentication material.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CopilotModel {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopilotConfig {
    #[serde(default)]
    recent_model_ids: Vec<String>,
}

pub(crate) fn discover() -> Result<Vec<CopilotModel>, String> {
    let Some(config_dir) = crate::task_creator::home_child("COPILOT_CONFIG_DIR", ".copilot") else {
        return Ok(Vec::new());
    };
    let config_path = config_dir.join("config.json");
    let contents = match std::fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("Could not read Copilot recent models".to_string()),
    };
    let config: CopilotConfig = serde_json::from_str(&contents).map_err(|_| {
        "Copilot recent models are unavailable because its config is invalid".to_string()
    })?;
    Ok(recent_models(config))
}

fn recent_models(config: CopilotConfig) -> Vec<CopilotModel> {
    let mut seen = HashSet::new();
    config
        .recent_model_ids
        .into_iter()
        .filter(|id| !id.trim().is_empty() && seen.insert(id.clone()))
        .map(|id| CopilotModel { id })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_model_ids_return_only_nonempty_unique_ids() {
        let config: CopilotConfig = serde_json::from_str(
            r#"{"recentModelIds":["gpt-5.6-terra","","gpt-5.6-terra","gpt-5.6-luna"]}"#,
        )
        .unwrap();
        let models = recent_models(config);
        assert_eq!(
            serde_json::to_value(models).unwrap(),
            serde_json::json!([{"id":"gpt-5.6-terra"},{"id":"gpt-5.6-luna"}])
        );
    }
}
