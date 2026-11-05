//! Gantry-wide settings an Admin changes in the UI (ADR-0004). Stored in
//! SQLite's `settings` table, so they travel with config export and import.
use axum::extract::State;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::users::User;

/// The setting key for the low-filament threshold, in percent remaining.
pub const LOW_FILAMENT: &str = "low_filament_percent";

/// The setting key for the Print File library's size limit, in MB.
pub const LIBRARY_LIMIT: &str = "library_limit_mb";

/// The setting key for the timelapses' size limit, in MB.
pub const TIMELAPSE_LIMIT: &str = "timelapse_limit_mb";

/// The setting key for looking for Printers on the LAN.
pub const DISCOVERY: &str = "discovery_enabled";

/// The largest library limit an Admin can set, in MB (1 TB).
pub const MAX_LIBRARY_LIMIT_MB: u64 = 1024 * 1024;

/// The settings the UI edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// A Filament Slot at or below this percentage is marked low (PRD §7.1),
    /// which can raise a low-filament Alert. 0 turns it off.
    pub low_filament_percent: u8,
    /// When the Print File library grows past this many MB, its oldest
    /// Print Files that no queued or running Job uses are removed (PRD
    /// §7.5). 0, the default, means no limit. Job History is never removed.
    pub library_limit_mb: u64,
    /// When timelapses grow past this many MB, the oldest are removed. 0,
    /// the default, means no limit. The Jobs they belong to stay in History.
    pub timelapse_limit_mb: u64,
    /// Every user must set up two-factor authentication (TOTP), and may do
    /// nothing else until they have (PRD §9.2). Off by default.
    pub require_totp: bool,
    /// Look for Printers on the LAN (SSDP and mDNS) to offer to add (PRD
    /// §7.6). On by default.
    pub discovery_enabled: bool,
}

/// The stored settings, with defaults for any not set.
pub async fn current(state: &AppState) -> Result<Settings, sqlx::Error> {
    let stored = state.store().settings().await?;
    Ok(Settings {
        low_filament_percent: stored
            .get(LOW_FILAMENT)
            .and_then(serde_json::Value::as_u64)
            .map_or(gantry_core::fleet::DEFAULT_LOW_FILAMENT_PERCENT, |p| {
                p.min(100) as u8
            }),
        library_limit_mb: stored
            .get(LIBRARY_LIMIT)
            .and_then(serde_json::Value::as_u64)
            .map_or(0, |mb| mb.min(MAX_LIBRARY_LIMIT_MB)),
        timelapse_limit_mb: stored
            .get(TIMELAPSE_LIMIT)
            .and_then(serde_json::Value::as_u64)
            .map_or(0, |mb| mb.min(MAX_LIBRARY_LIMIT_MB)),
        require_totp: stored
            .get(crate::store::two_factor::REQUIRE_TOTP)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        discovery_enabled: stored
            .get(DISCOVERY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    })
}

/// Puts the stored settings into effect: at start, and after an import.
pub async fn apply(state: &AppState) -> Result<(), sqlx::Error> {
    let settings = current(state).await?;
    state
        .fleet()
        .set_low_filament_threshold(settings.low_filament_percent);
    Ok(())
}

/// `GET /api/settings`.
pub async fn get(State(state): State<AppState>) -> Result<Json<Settings>, ApiError> {
    Ok(Json(current(&state).await?))
}

#[derive(Debug, Deserialize)]
pub struct SettingsForm {
    low_filament_percent: Option<u32>,
    library_limit_mb: Option<u64>,
    timelapse_limit_mb: Option<u64>,
    require_totp: Option<bool>,
    discovery_enabled: Option<bool>,
}

/// `PUT /api/settings`: changes the settings given, Admins only.
pub async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<SettingsForm>,
) -> Result<Json<Settings>, ApiError> {
    let result = async {
        if let Some(percent) = form.low_filament_percent {
            if percent > 100 {
                return Err(ApiError::field("low_filament_percent", "Must be 0 to 100"));
            }
            state
                .store()
                .set_setting(LOW_FILAMENT, &serde_json::json!(percent))
                .await?;
        }
        if let Some(mb) = form.library_limit_mb {
            if mb > MAX_LIBRARY_LIMIT_MB {
                return Err(ApiError::field(
                    "library_limit_mb",
                    "Must be 0 (no limit) to 1048576 MB",
                ));
            }
            state
                .store()
                .set_setting(LIBRARY_LIMIT, &serde_json::json!(mb))
                .await?;
        }
        if let Some(mb) = form.timelapse_limit_mb {
            if mb > MAX_LIBRARY_LIMIT_MB {
                return Err(ApiError::field(
                    "timelapse_limit_mb",
                    "Must be 0 (no limit) to 1048576 MB",
                ));
            }
            state
                .store()
                .set_setting(TIMELAPSE_LIMIT, &serde_json::json!(mb))
                .await?;
        }
        if let Some(on) = form.require_totp {
            state
                .store()
                .set_setting(
                    crate::store::two_factor::REQUIRE_TOTP,
                    &serde_json::json!(on),
                )
                .await?;
        }
        if let Some(on) = form.discovery_enabled {
            state
                .store()
                .set_setting(DISCOVERY, &serde_json::json!(on))
                .await?;
            if !on {
                state.discovered().clear();
            }
            state.notify_discovery();
        }
        apply(&state).await?;
        Ok(current(&state).await?)
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::SettingsUpdated)
            .detail(serde_json::json!({
                "low_filament_percent": form.low_filament_percent,
                "library_limit_mb": form.library_limit_mb,
                "timelapse_limit_mb": form.timelapse_limit_mb,
                "require_totp": form.require_totp,
                "discovery_enabled": form.discovery_enabled,
            }))
            .result(&result),
    )
    .await;
    if result.is_ok() && form.library_limit_mb.is_some() {
        crate::retention::clean_library_logged(&state, None).await;
    }
    if result.is_ok() && form.timelapse_limit_mb.is_some() {
        crate::timelapse::clean_logged(&state).await;
    }
    result.map(Json)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::store::users::Role;
    use crate::test_support::Client;

    #[tokio::test]
    async fn the_low_filament_threshold_marks_slots_low() {
        let client = Client::new().await;
        let printer = client
            .connected_simulated("X1", json!({ "filament_slots": 4 }))
            .await;
        let (_, settings) = client.get("/api/settings").await;
        assert_eq!(settings["low_filament_percent"], 15);
        // Slot 3 starts at 70%: not low until the threshold passes it.
        client
            .simulate(
                &printer,
                json!({ "action": "set_filament_slot", "slot": "0",
                        "filament": { "kind": "PLA", "colour": "#FFFFFF", "remaining_percent": 10 } }),
            )
            .await;
        let view = client
            .until(&printer, |v| v.status.filament_slots[0].low)
            .await;
        assert!(!view.status.filament_slots[2].low);

        let (status, settings) = client
            .put("/api/settings", json!({ "low_filament_percent": 75 }))
            .await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        assert_eq!(settings["low_filament_percent"], 75);
        client
            .until(&printer, |v| v.status.filament_slots[2].low)
            .await;
        let (status, _) = client
            .put("/api/settings", json!({ "low_filament_percent": 101 }))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        let operator = client.as_role(Role::Operator).await;
        let (status, _) = operator
            .put("/api/settings", json!({ "low_filament_percent": 5 }))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = operator.get("/api/settings").await;
        assert_eq!(status, StatusCode::OK);
    }
}
