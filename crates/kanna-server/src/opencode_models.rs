//! Read OpenCode's own resolved inventory on the execution machine. Connection
//! settings and credentials stay with OpenCode; only display metadata leaves here.
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpencodeModel {
    id: String,
    name: String,
    connection: Option<String>,
    local: bool,
    context: Option<u64>,
}

async fn read_cli(
    executable: &str,
    cwd: &str,
    env: &HashMap<String, String>,
    args: &[&str],
) -> Result<String, String> {
    let mut command = tokio::process::Command::new(executable);
    command
        .args(args)
        .envs(env)
        .current_dir(cwd)
        .kill_on_drop(true)
        .env("OPENCODE_DISABLE_AUTOUPDATE", "true")
        .env("OPENCODE_DISABLE_MODELS_FETCH", "true")
        .stdin(std::process::Stdio::null());
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .map_err(|_| "OpenCode model discovery timed out (30s)".to_string())?
        .map_err(|_| "Could not start OpenCode model discovery".to_string())?;
    // Never return stderr or raw config: either can contain credentials.
    if !output.status.success() {
        return Err(format!(
            "OpenCode {} failed ({})",
            args.join(" "),
            output.status
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "OpenCode returned non-UTF-8 output".into())
}

pub(crate) async fn discover(
    executable: &str,
    cwd: &str,
    env: &HashMap<String, String>,
) -> Result<Vec<OpencodeModel>, String> {
    let models = read_cli(executable, cwd, env, &["--pure", "models", "--verbose"]).await?;
    let config = read_cli(executable, cwd, env, &["--pure", "debug", "config"]).await?;
    let config: Value = serde_json::from_str(&config)
        .map_err(|_| "OpenCode returned an invalid resolved config".to_string())?;
    parse_models(&models, &config)
}

fn parse_models(mut text: &str, config: &Value) -> Result<Vec<OpencodeModel>, String> {
    let mut models = Vec::new();
    while !text.trim().is_empty() {
        let (id, rest) = text
            .trim_start()
            .split_once('\n')
            .ok_or("OpenCode returned an unsupported model inventory format")?;
        let id = id.trim_end_matches('\r');
        let (provider, _) = id
            .split_once('/')
            .ok_or("OpenCode returned an invalid model identifier")?;
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        let metadata = stream
            .next()
            .and_then(Result::ok)
            .ok_or("OpenCode returned invalid model metadata")?;
        text = &rest[stream.byte_offset()..];
        let base = config["provider"][provider]["options"]["baseURL"]
            .as_str()
            .or_else(|| metadata["api"]["url"].as_str());
        let url = base.and_then(|value| reqwest::Url::parse(value).ok());
        let local = url
            .as_ref()
            .and_then(|url| url.host_str())
            .is_some_and(|host| {
                host == "localhost"
                    || host
                        .trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
        // Origin only: paths, query strings and user info can all contain secrets.
        let connection = url.map(|url| url.origin().ascii_serialization());
        models.push(OpencodeModel {
            id: id.to_string(),
            name: metadata["name"].as_str().unwrap_or(id).to_string(),
            connection,
            local,
            context: metadata["limit"]["context"].as_u64(),
        });
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_inventory_keeps_full_ids_and_redacts_connections() {
        let result = parse_models("omlx/qwen-a\n{\"name\":\"Qwen\",\"limit\":{\"context\":32768}}\ncloud/model\n{}\n", &serde_json::json!({
            "provider": {"omlx": {"options": {"baseURL": "http://user:secret@127.0.0.1:8000/private-token/v1?key=secret"}}}
        })).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, "omlx/qwen-a");
        assert_eq!(
            result[0].connection.as_deref(),
            Some("http://127.0.0.1:8000")
        );
        assert!(result[0].local);
        assert_eq!(result[0].context, Some(32768));
        assert!(!result[1].local);
        assert!(!serde_json::to_string(&result).unwrap().contains("secret"));
    }
    #[test]
    fn invalid_inventory_is_an_error_not_an_empty_success() {
        assert!(parse_models("warning\n{}", &Value::Null).is_err());
    }
}
