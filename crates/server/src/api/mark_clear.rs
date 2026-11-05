//! Mark Clear (PRD §6.3): an Operator confirms a Printer's bed is empty, and
//! the Printer goes from Awaiting Clear back to Idle. A Succeeded Job can be
//! overruled to Failed at the same time; nothing else can be overruled.
use axum::extract::{Path, State};
use axum::{Extension, Json};
use gantry_core::JobRecord;
use serde::Deserialize;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::users::User;

#[derive(Debug, Deserialize)]
pub struct MarkClearForm {
    /// The Job Succeeded, but the part failed (spaghetti, warping, …).
    #[serde(default)]
    overrule_to_failed: bool,
}

/// `POST /api/printers/{id}/mark-clear`: returns the Job as now recorded.
pub async fn mark_clear(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(form): Json<MarkClearForm>,
) -> Result<Json<JobRecord>, ApiError> {
    let result = state
        .fleet()
        .mark_clear(&id, form.overrule_to_failed)
        .await
        .map_err(ApiError::from);
    let job_id = result.as_ref().ok().map(|j| j.id);
    audit::record(
        &state,
        audit::by(&user, Action::MarkClear)
            .printer(&id)
            .detail(serde_json::json!({
                "job": job_id,
                "overrule_to_failed": form.overrule_to_failed,
            }))
            .result(&result),
    )
    .await;
    let job = result?;
    tracing::info!(
        printer = %id,
        job = job.id,
        overruled = form.overrule_to_failed,
        "Mark Clear"
    );
    Ok(Json(job))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::http::StatusCode;
    use gantry_core::{PrinterState, PrinterView};
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

    async fn simulate(client: &Client, id: &str, action: Value) {
        let (status, body) = client
            .post(&format!("/api/printers/{id}/simulation"), action)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }

    /// A Printer Awaiting Clear after a Job that ended `how`.
    async fn awaiting_clear(client: &Client, name: &str, how: &str) -> String {
        let id = client.add_simulated(name, json!({})).await;
        until(client, &id, |v| v.status.connection.is_connected()).await;
        simulate(client, &id, json!({ "action": "set_speed", "speed": 100 })).await;
        let duration = if how == "succeed" { 10 } else { 60_000 };
        simulate(
            client,
            &id,
            json!({ "action": "start_print", "duration_secs": duration, "layers": 3 }),
        )
        .await;
        if how == "fail" {
            until(client, &id, |v| v.job.is_some()).await;
            simulate(client, &id, json!({ "action": "fail_print" })).await;
        }
        until(client, &id, |v| {
            v.status.state == Some(PrinterState::AwaitingClear)
        })
        .await;
        id
    }

    async fn mark_clear(client: &Client, id: &str, overrule: bool) -> (StatusCode, Value) {
        client
            .post(
                &format!("/api/printers/{id}/mark-clear"),
                json!({ "overrule_to_failed": overrule }),
            )
            .await
    }

    #[tokio::test]
    async fn mark_clear_returns_the_printer_to_idle() {
        let client = Client::new().await;
        let id = awaiting_clear(&client, "Clear me", "succeed").await;
        let (status, job) = mark_clear(&client, &id, false).await;
        assert_eq!(status, StatusCode::OK, "{job}");
        assert_eq!(job["outcome"], "succeeded");
        until(&client, &id, |v| v.status.state == Some(PrinterState::Idle)).await;
    }

    #[tokio::test]
    async fn a_succeeded_job_can_be_overruled_to_failed() {
        let client = Client::new().await;
        let id = awaiting_clear(&client, "Spaghetti", "succeed").await;
        let (status, job) = mark_clear(&client, &id, true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(job["outcome"], "failed");
        assert_eq!(job["overruled"], true);
        let (_, jobs) = client.get(&format!("/api/printers/{id}/jobs")).await;
        assert_eq!(jobs[0]["outcome"], "failed");
        assert_eq!(jobs[0]["overruled"], true);
    }

    #[tokio::test]
    async fn no_other_outcome_can_be_overruled() {
        let client = Client::new().await;
        let id = awaiting_clear(&client, "Already failed", "fail").await;
        let (status, body) = mark_clear(&client, &id, true).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("only a Succeeded Job")
        );
        // Refused, so still Awaiting Clear.
        assert_eq!(
            client.state.fleet().view(&id).unwrap().status.state,
            Some(PrinterState::AwaitingClear)
        );
        assert_eq!(mark_clear(&client, &id, false).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn mark_clear_is_only_for_awaiting_clear() {
        let client = Client::new().await;
        let id = client.add_simulated("Idle", json!({})).await;
        until(&client, &id, |v| v.status.connection.is_connected()).await;
        let (status, body) = mark_clear(&client, &id, false).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["error"].as_str().unwrap().contains("Awaiting Clear"));
        assert_eq!(
            mark_clear(&client, "nope", false).await.0,
            StatusCode::NOT_FOUND
        );
    }
}
