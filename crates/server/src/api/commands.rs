//! Commands to one Printer: pause, resume and Cancel (PRD §6.4, §7.2).
//!
//! A command to a Printer that isn't connected is refused at once with the
//! reason, and never held for later. The UI asks for confirmation first.
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use gantry_core::Command;
use serde::Deserialize;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::users::User;

/// The body: `{ "command": "pause" }`, or a fine control with its values,
/// such as `{ "command": "set_fan", "fan": "part", "percent": 50 }`.
#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct CommandForm {
    command: Command,
}

/// `POST /api/printers/{id}/commands`.
pub async fn send(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(form): Json<CommandForm>,
) -> Result<StatusCode, ApiError> {
    let detail = serde_json::to_value(&form.command).unwrap_or_default();
    let result = state
        .fleet()
        .command(&id, form.command.clone())
        .await
        .map_err(ApiError::from);
    match &result {
        Ok(()) => tracing::info!(printer = %id, command = ?form.command, "command done"),
        Err(e) => {
            tracing::info!(printer = %id, command = ?form.command, error = %e.message(), "command refused")
        }
    }
    audit::record(
        &state,
        audit::by(&user, Action::PrinterCommand)
            .printer(&id)
            .detail(detail)
            .result(&result),
    )
    .await;
    result?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use axum::http::StatusCode;
    use gantry_core::{JobOutcome, PrinterState, PrinterView};
    use serde_json::{Value, json};

    use crate::test_support::Client;

    async fn until(client: &Client, id: &str, pred: impl Fn(&PrinterView) -> bool) -> PrinterView {
        (*client
            .state
            .fleet()
            .wait_for(id, Duration::from_secs(5), pred)
            .await
            .unwrap())
        .clone()
    }

    async fn printing(client: &Client, name: &str, settings: Value) -> String {
        let id = client.add_simulated(name, settings).await;
        until(client, &id, |v| v.status.connection.is_connected()).await;
        let (status, _) = client
            .post(
                &format!("/api/printers/{id}/simulation"),
                json!({ "action": "start_print", "duration_secs": 6000, "layers": 10 }),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        until(client, &id, |v| v.job.is_some()).await;
        id
    }

    async fn command(client: &Client, id: &str, command: &str) -> (StatusCode, Value) {
        client
            .post(
                &format!("/api/printers/{id}/commands"),
                json!({ "command": command }),
            )
            .await
    }

    #[tokio::test]
    async fn pause_resume_and_cancel() {
        let client = Client::new().await;
        let id = printing(&client, "Commands", json!({})).await;

        assert_eq!(
            command(&client, &id, "pause").await.0,
            StatusCode::NO_CONTENT
        );
        until(&client, &id, |v| {
            v.status.state == Some(PrinterState::Paused)
        })
        .await;
        assert_eq!(
            command(&client, &id, "resume").await.0,
            StatusCode::NO_CONTENT
        );
        until(&client, &id, |v| {
            matches!(
                v.status.state,
                Some(PrinterState::Printing | PrinterState::Preparing)
            )
        })
        .await;

        assert_eq!(
            command(&client, &id, "cancel").await.0,
            StatusCode::NO_CONTENT
        );
        let view = until(&client, &id, |v| {
            v.status.state == Some(PrinterState::AwaitingClear)
        })
        .await;
        assert_eq!(
            view.awaiting_clear.unwrap().outcome,
            Some(JobOutcome::Cancelled)
        );
        let (_, jobs) = client.get(&format!("/api/printers/{id}/jobs")).await;
        assert_eq!(jobs[0]["outcome"], "cancelled");
    }

    #[tokio::test]
    async fn fine_controls_reach_the_driver_through_the_api() {
        let client = Client::new().await;
        let id = client.add_simulated("Fine", json!({})).await;
        until(&client, &id, |v| v.status.connection.is_connected()).await;
        let send = |body: Value| {
            let client = &client;
            let id = id.clone();
            async move {
                client
                    .post(&format!("/api/printers/{id}/commands"), body)
                    .await
            }
        };

        let (status, body) =
            send(json!({ "command": "set_speed_profile", "profile": "sport" })).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        send(json!({ "command": "set_temperature", "heater": "nozzle", "target": 200 })).await;
        send(json!({ "command": "set_light", "light": "work", "on": true })).await;
        send(json!({ "command": "set_fan", "fan": "part", "percent": 30 })).await;
        send(json!({ "command": "home" })).await;
        let view = until(&client, &id, |v| {
            v.status.speed_profile.as_deref() == Some("sport")
                && v.status.temperatures.nozzle.and_then(|t| t.target) == Some(200.0)
                && v.status
                    .lights
                    .iter()
                    .any(|l| l.on && l.id == gantry_core::LightId::Work)
                && v.status
                    .fans
                    .iter()
                    .any(|f| f.id == "part" && f.percent == 30)
                && v.status.position.is_some()
        })
        .await;
        assert_eq!(view.status.position.unwrap().x, 0.0);

        // Out of range: refused before it reaches the Driver, as invalid.
        let (status, body) =
            send(json!({ "command": "set_temperature", "heater": "bed", "target": 9000 })).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["reason"]["kind"], "invalid");

        // A Printer without the capability says so.
        let plain = client
            .add_simulated("No fine controls", json!({ "control": false }))
            .await;
        until(&client, &plain, |v| v.status.connection.is_connected()).await;
        let (status, body) = client
            .post(
                &format!("/api/printers/{plain}/commands"),
                json!({ "command": "home" }),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason"]["kind"], "unsupported");

        // Each is audited with what was asked.
        let entries = client
            .state
            .store()
            .audit_entries(&crate::store::audit::Filter {
                action: Some("printer.command".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(entries.iter().any(
            |e| e.detail == Some(json!({ "command": "set_fan", "fan": "part", "percent": 30 }))
        ));
    }

    #[tokio::test]
    async fn a_disconnected_printer_refuses_at_once_and_nothing_is_queued() {
        let client = Client::new().await;
        let id = printing(&client, "Dropped", json!({})).await;
        client
            .post(
                &format!("/api/printers/{id}/simulation"),
                json!({ "action": "drop_connection", "reason": "unreachable" }),
            )
            .await;
        until(&client, &id, |v| !v.status.connection.is_connected()).await;

        let started = Instant::now();
        let (status, body) = command(&client, &id, "pause").await;
        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason"]["kind"], "disconnected");
        assert_eq!(body["reason"]["reason"], "unreachable");
        assert!(body["error"].as_str().unwrap().contains("disconnected"));

        // Back online, the Printer is still printing: the pause was not held.
        client
            .post(
                &format!("/api/printers/{id}/simulation"),
                json!({ "action": "restore_connection" }),
            )
            .await;
        until(&client, &id, |v| v.status.connection.is_connected()).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        let view = client.state.fleet().view(&id).unwrap();
        assert_ne!(view.status.state, Some(PrinterState::Paused));
    }

    #[tokio::test]
    async fn commands_the_printer_cannot_take_are_refused_with_a_reason() {
        let client = Client::new().await;
        let id = client.add_simulated("Idle", json!({})).await;
        until(&client, &id, |v| v.status.connection.is_connected()).await;
        let (status, body) = command(&client, &id, "pause").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason"]["kind"], "invalid_state");

        let status_only = printing(&client, "Status only", json!({ "control": false })).await;
        let (status, body) = command(&client, &status_only, "cancel").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason"]["kind"], "unsupported");

        assert_eq!(
            command(&client, "nope", "pause").await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            command(&client, &id, "explode").await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
