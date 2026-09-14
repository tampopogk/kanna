use super::state::AppState;
use crate::db::Db;
use axum::extract::State;
use axum::Json;
use kanna_agent_protocol::StateChangeScope;
use std::sync::Arc;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DesktopSnapshot {
    #[serde(flatten)]
    snapshot: crate::db::UiSnapshot,
    cloud_account: CloudAccountSnapshot,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudAccountSnapshot {
    user_id: Option<String>,
    entitlement: Option<crate::relay_client::RelayEntitlement>,
}

pub(super) async fn get_snapshot(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DesktopSnapshot>, (axum::http::StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {}", e),
        )
    })?;
    let recorded_orphans = crate::mobile_api::record_orphaned_initialized_tasks(&db)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if recorded_orphans {
        state.publish_state_changed(StateChangeScope::Tasks);
    }
    let snapshot = db.ui_snapshot().map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {e}"),
        )
    })?;
    let (user_id, entitlement) = state.cloud_account_snapshot();
    Ok(Json(DesktopSnapshot {
        snapshot,
        cloud_account: CloudAccountSnapshot {
            user_id,
            entitlement,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn desktop_snapshot_projects_live_access_and_clears_it_on_account_change() {
        let state = super::super::tests::test_state_with_seed("desktop-access", "Access", |_| {});
        state.set_authenticated_account_uid(Some("user-1".to_string()));
        state.set_relay_entitlement(Some(crate::relay_client::RelayEntitlement {
            active: false,
            status: "grace".to_string(),
            current_period_ends_at: None,
            grace_ends_at: Some("2026-01-01T00:00:00Z".to_string()),
            reason: None,
        }));
        let snapshot =
            serde_json::to_value(get_snapshot(State(state.clone())).await.unwrap().0).unwrap();
        assert_eq!(snapshot["cloudAccount"]["userId"], "user-1");
        assert_eq!(snapshot["cloudAccount"]["entitlement"]["active"], false);
        assert_eq!(snapshot["cloudAccount"]["entitlement"]["status"], "grace");
        assert!(snapshot.get("entries").is_some());

        state.set_authenticated_account_uid(Some("user-2".to_string()));
        let snapshot = serde_json::to_value(get_snapshot(State(state)).await.unwrap().0).unwrap();
        assert_eq!(snapshot["cloudAccount"]["userId"], "user-2");
        assert!(snapshot["cloudAccount"]["entitlement"].is_null());
    }
}
