//! The simulation endpoint behind each Simulated Printer's panel (PRD §5.6).
//! End-to-end tests drive Simulated Printers through it too. It exists only
//! for Simulated Printers, only for Admins, and behind the same session gate
//! as the rest of the API.
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use gantry_core::{DisconnectReason, Fault, FaultSeverity, Filament};
use gantry_driver_simulated::{SimulatedPrinter, SimulationError, StartPrint};
use serde::Deserialize;
use serde_json::json;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::users::{Role, User};

/// One thing to make a Simulated Printer do.
#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    StartPrint(StartPrint),
    FailPrint,
    SetSpeed {
        speed: f64,
    },
    RaiseFault {
        code: String,
        severity: FaultSeverity,
        message: String,
    },
    ClearFault {
        code: String,
    },
    DropConnection {
        #[serde(default = "unreachable")]
        reason: DisconnectReason,
    },
    RestoreConnection,
    SetFilamentSlot {
        slot: String,
        filament: Option<Filament>,
    },
}

fn unreachable() -> DisconnectReason {
    DisconnectReason::Unreachable
}

fn simulated(state: &AppState, id: &str) -> Result<SimulatedPrinter, ApiError> {
    let handle = state.fleet().handle(id)?;
    handle
        .as_any()
        .downcast_ref::<SimulatedPrinter>()
        .cloned()
        .ok_or_else(|| ApiError::NotFound("this Printer is not a Simulated Printer".into()))
}

/// Also enforced by the role check on the route; kept here so the panel can
/// never be reached by a lower role through another path.
fn require_admin(user: &User) -> Result<(), ApiError> {
    if user.role >= Role::Admin {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "the simulation panel is for Admins".into(),
        ))
    }
}

fn simulation_error(e: SimulationError) -> ApiError {
    ApiError::Command(gantry_core::CommandError::invalid_state(e.to_string()))
}

/// `GET /api/printers/{id}/simulation`: the simulation's own settings.
pub async fn show(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&user)?;
    let sim = simulated(&state, &id)?;
    Ok(Json(json!({ "speed": sim.speed() })))
}

/// `POST /api/printers/{id}/simulation`.
pub async fn act(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(action): Json<Action>,
) -> Result<StatusCode, ApiError> {
    let detail = serde_json::to_value(&action).unwrap_or_default();
    let result = perform(&state, &user, &id, action);
    audit::record(
        &state,
        audit::by(&user, crate::store::audit::Action::Simulation)
            .printer(&id)
            .detail(detail)
            .result(&result),
    )
    .await;
    result?;
    Ok(StatusCode::NO_CONTENT)
}

fn perform(state: &AppState, user: &User, id: &str, action: Action) -> Result<(), ApiError> {
    require_admin(user)?;
    let sim = simulated(state, id)?;
    tracing::info!(printer = %id, action = ?action, "simulation");
    match action {
        Action::StartPrint(print) => {
            sim.start_print(print).map_err(simulation_error)?;
        }
        Action::FailPrint => sim.fail_print().map_err(simulation_error)?,
        Action::SetSpeed { speed } => sim.set_speed(speed).map_err(simulation_error)?,
        Action::RaiseFault {
            code,
            severity,
            message,
        } => {
            if code.trim().is_empty() || message.trim().is_empty() {
                return Err(ApiError::Invalid(vec![gantry_core::FieldError {
                    field: if code.trim().is_empty() {
                        "code"
                    } else {
                        "message"
                    }
                    .into(),
                    message: "Required".into(),
                }]));
            }
            sim.raise_fault(Fault {
                code: code.trim().to_owned(),
                severity,
                message: message.trim().to_owned(),
            });
        }
        Action::ClearFault { code } => sim.clear_fault(code.trim()),
        Action::DropConnection { reason } => sim.drop_connection(reason),
        Action::RestoreConnection => sim.restore_connection(),
        Action::SetFilamentSlot { slot, filament } => sim
            .set_filament_slot(&slot, filament)
            .map_err(simulation_error)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::http::StatusCode;
    use gantry_core::{Connection, DisconnectReason, FaultSeverity, PrinterState, PrinterView};
    use serde_json::json;

    use crate::test_support::Client;

    async fn connected(client: &Client, name: &str) -> String {
        let id = client.add_simulated(name, json!({})).await;
        client
            .state
            .fleet()
            .wait_for(&id, Duration::from_secs(2), |v| {
                v.status.connection.is_connected()
            })
            .await
            .unwrap();
        id
    }

    async fn act(client: &Client, id: &str, action: serde_json::Value) -> StatusCode {
        client
            .post(&format!("/api/printers/{id}/simulation"), action)
            .await
            .0
    }

    async fn view(client: &Client, id: &str, pred: impl Fn(&PrinterView) -> bool) -> PrinterView {
        (*client
            .state
            .fleet()
            .wait_for(id, Duration::from_secs(2), pred)
            .await
            .unwrap())
        .clone()
    }

    #[tokio::test]
    async fn every_behaviour_can_be_triggered() {
        let client = Client::new().await;
        let id = connected(&client, "Sim").await;

        let ok = StatusCode::NO_CONTENT;
        assert_eq!(
            act(&client, &id, json!({ "action": "start_print", "print_file": "cube.gcode", "duration_secs": 120, "layers": 12 })).await,
            ok
        );
        let v = view(&client, &id, |v| v.status.job.is_some()).await;
        assert_eq!(v.status.job.unwrap().total_layers, Some(12));

        assert_eq!(
            act(&client, &id, json!({ "action": "set_speed", "speed": 20 })).await,
            ok
        );
        let (_, body) = client.get(&format!("/api/printers/{id}/simulation")).await;
        assert_eq!(body["speed"], 20.0);

        for severity in ["warning", "error"] {
            assert_eq!(
                act(&client, &id, json!({ "action": "raise_fault", "code": severity, "severity": severity, "message": "Something" })).await,
                ok
            );
        }
        let v = view(&client, &id, |v| v.status.faults.len() == 2).await;
        assert!(
            v.status
                .faults
                .iter()
                .any(|f| f.severity == FaultSeverity::Error)
        );
        // A Fault sits alongside the Job; it is not a Printer State.
        assert!(v.status.state.is_some_and(PrinterState::has_job));
        assert_eq!(
            act(
                &client,
                &id,
                json!({ "action": "clear_fault", "code": "error" })
            )
            .await,
            ok
        );
        view(&client, &id, |v| v.status.faults.len() == 1).await;

        assert_eq!(
            act(&client, &id, json!({ "action": "set_filament_slot", "slot": "3", "filament": { "kind": "TPU", "colour": "#00FF00", "remaining_percent": 10 } })).await,
            ok
        );
        view(&client, &id, |v| {
            v.status.filament_slots[3]
                .filament
                .as_ref()
                .is_some_and(|f| f.kind == "TPU")
        })
        .await;
        assert_eq!(
            act(
                &client,
                &id,
                json!({ "action": "set_filament_slot", "slot": "3", "filament": null })
            )
            .await,
            ok
        );
        view(&client, &id, |v| {
            v.status.filament_slots[3].filament.is_none()
        })
        .await;

        assert_eq!(
            act(
                &client,
                &id,
                json!({ "action": "drop_connection", "reason": "authentication_rejected" })
            )
            .await,
            ok
        );
        let v = view(&client, &id, |v| !v.status.connection.is_connected()).await;
        assert_eq!(
            v.status.connection,
            Connection::disconnected(DisconnectReason::AuthenticationRejected)
        );
        assert_eq!(
            act(&client, &id, json!({ "action": "restore_connection" })).await,
            ok
        );
        view(&client, &id, |v| v.status.connection.is_connected()).await;

        assert_eq!(
            act(&client, &id, json!({ "action": "fail_print" })).await,
            ok
        );
        view(&client, &id, |v| v.status.job.is_none()).await;
    }

    #[tokio::test]
    async fn bad_requests_are_refused() {
        let client = Client::new().await;
        let id = connected(&client, "Sim").await;
        assert_eq!(
            act(&client, &id, json!({ "action": "fail_print" })).await,
            StatusCode::CONFLICT
        );
        assert_eq!(
            act(&client, &id, json!({ "action": "set_speed", "speed": -1 })).await,
            StatusCode::CONFLICT
        );
        assert_eq!(
            act(
                &client,
                &id,
                json!({ "action": "raise_fault", "code": "", "severity": "error", "message": "x" })
            )
            .await,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            act(&client, &id, json!({ "action": "explode" })).await,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            act(
                &client,
                "no-such-printer",
                json!({ "action": "fail_print" })
            )
            .await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn only_simulated_printers_have_it() {
        let client = Client::new().await;
        let (status, body) = client
            .post(
                "/api/printers",
                json!({ "name": "Inert", "driver": crate::test_support::INERT, "settings": {} }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let id = body["id"].as_str().unwrap();
        assert_eq!(
            act(&client, id, json!({ "action": "restore_connection" })).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            client
                .get(&format!("/api/printers/{id}/simulation"))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn it_needs_a_session() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let client = Client::new().await;
        let id = connected(&client, "Sim").await;
        let response = crate::app(client.state.clone())
            .oneshot(
                Request::post(format!("/api/printers/{id}/simulation"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "action": "drop_connection" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        // Nothing happened.
        assert!(
            client
                .state
                .fleet()
                .view(&id)
                .unwrap()
                .status
                .connection
                .is_connected()
        );
    }
}
