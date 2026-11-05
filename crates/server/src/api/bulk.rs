//! Bulk pause, resume and Cancel (PRD §6.4). Each Printer gets its own
//! independent command, run side by side, with its own result: done, rejected
//! (disconnected, refused, timed out) or skipped (it wasn't doing anything the
//! command applies to). One Printer's failure never affects another's, and
//! each part is audited on its own.
use axum::extract::State;
use axum::{Extension, Json};
use gantry_core::{Command, PrinterState};
use serde::{Deserialize, Serialize};

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::{Action, Outcome};
use crate::store::users::User;

/// The commands a bulk action can send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkCommand {
    Pause,
    Resume,
    Cancel,
}

impl BulkCommand {
    fn command(self) -> Command {
        match self {
            Self::Pause => Command::Pause,
            Self::Resume => Command::Resume,
            Self::Cancel => Command::Cancel,
        }
    }

    /// Why this command doesn't apply to a Printer in `state`, if it doesn't.
    fn skip_reason(self, state: Option<PrinterState>) -> Option<&'static str> {
        let applies = match (self, state) {
            (Self::Pause, Some(PrinterState::Printing | PrinterState::Preparing)) => true,
            (Self::Resume, Some(PrinterState::Paused)) => true,
            (Self::Cancel, Some(s)) => s.has_job(),
            _ => false,
        };
        (!applies).then_some(match self {
            Self::Pause => "not printing",
            Self::Resume => "not paused",
            Self::Cancel => "no Job running",
        })
    }
}

/// `POST /api/bulk/commands` body. Without `printers`, every Printer.
#[derive(Debug, Deserialize)]
pub struct BulkForm {
    command: BulkCommand,
    #[serde(default)]
    printers: Option<Vec<String>>,
}

/// How one Printer's part went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PartResult {
    Done,
    Rejected,
    Skipped,
}

/// One Printer's part of a bulk action.
#[derive(Debug, Clone, Serialize)]
pub struct Part {
    pub printer_id: String,
    pub name: Option<String>,
    pub result: PartResult,
    /// Why it was rejected or skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `POST /api/bulk/commands`: one result per Printer, in the order asked
/// (or by name, for every Printer).
pub async fn send(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<BulkForm>,
) -> Result<Json<Vec<Part>>, ApiError> {
    let ids = match form.printers {
        Some(ids) => ids,
        None => state
            .fleet()
            .snapshot()
            .iter()
            .map(|v| v.id.clone())
            .collect(),
    };
    let parts = futures::future::join_all(
        ids.into_iter()
            .map(|id| part(&state, &user, form.command, id)),
    )
    .await;
    tracing::info!(
        command = ?form.command,
        done = parts.iter().filter(|p| p.result == PartResult::Done).count(),
        of = parts.len(),
        "bulk action"
    );
    Ok(Json(parts))
}

/// One Printer's part: checked, sent and audited on its own.
async fn part(state: &AppState, user: &User, command: BulkCommand, id: String) -> Part {
    let view = state.fleet().view(&id).ok();
    let name = view.as_ref().map(|v| v.name.clone());
    let (result, message, outcome) = match &view {
        None => (
            PartResult::Rejected,
            Some("no such Printer".to_owned()),
            Outcome::Refused,
        ),
        // Disconnected (or still connecting) is rejected by the Fleet below,
        // with the reason, rather than skipped.
        Some(v) if v.status.connection.is_connected() => {
            match command.skip_reason(v.status.state) {
                Some(why) => (PartResult::Skipped, Some(why.to_owned()), Outcome::Skipped),
                None => send_one(state, &id, command).await,
            }
        }
        Some(_) => send_one(state, &id, command).await,
    };
    let mut entry = audit::by(user, Action::PrinterCommand)
        .printer(&id)
        .detail(serde_json::json!({ "command": command, "bulk": true }));
    entry.printer_name.clone_from(&name);
    entry.outcome = outcome;
    entry.message.clone_from(&message);
    audit::record(state, entry).await;
    Part {
        printer_id: id,
        name,
        result,
        message,
    }
}

async fn send_one(
    state: &AppState,
    id: &str,
    command: BulkCommand,
) -> (PartResult, Option<String>, Outcome) {
    match state
        .fleet()
        .command(id, command.command())
        .await
        .map_err(ApiError::from)
    {
        Ok(()) => (PartResult::Done, None, Outcome::Ok),
        Err(e) => (PartResult::Rejected, Some(e.message()), audit::outcome(&e)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::http::StatusCode;
    use gantry_core::{PrinterState, PrinterView};
    use serde_json::{Value, json};

    use crate::store::audit::Filter;
    use crate::store::users::Role;
    use crate::test_support::Client;

    async fn until(client: &Client, id: &str, pred: impl Fn(&PrinterView) -> bool) {
        client
            .state
            .fleet()
            .wait_for(id, Duration::from_secs(5), pred)
            .await
            .unwrap();
    }

    async fn printing(client: &Client, name: &str) -> String {
        let id = client.add_simulated(name, json!({})).await;
        until(client, &id, |v| v.status.connection.is_connected()).await;
        client
            .post(
                &format!("/api/printers/{id}/simulation"),
                json!({ "action": "start_print", "duration_secs": 6000, "layers": 10 }),
            )
            .await;
        until(client, &id, |v| v.job.is_some()).await;
        id
    }

    fn result_of<'a>(parts: &'a Value, id: &str) -> &'a Value {
        parts
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["printer_id"] == id)
            .unwrap()
    }

    #[tokio::test]
    async fn each_printer_gets_its_own_result_and_one_failure_affects_no_other() {
        let client = Client::new().await;
        let a = printing(&client, "A").await;
        let b = printing(&client, "B").await;
        let idle = client.add_simulated("Idle", json!({})).await;
        until(&client, &idle, |v| v.status.connection.is_connected()).await;
        let offline = printing(&client, "Offline").await;
        client
            .post(
                &format!("/api/printers/{offline}/simulation"),
                json!({ "action": "drop_connection" }),
            )
            .await;
        until(&client, &offline, |v| !v.status.connection.is_connected()).await;
        let before = client.state.store().audit_count().await.unwrap();
        // Every Printer: no list given.
        let (status, parts) = client
            .post("/api/bulk/commands", json!({ "command": "pause" }))
            .await;
        assert_eq!(status, StatusCode::OK, "{parts}");
        assert_eq!(parts.as_array().unwrap().len(), 4);
        assert_eq!(result_of(&parts, &a)["result"], "done");
        assert_eq!(result_of(&parts, &b)["result"], "done");
        assert_eq!(result_of(&parts, &idle)["result"], "skipped");
        assert_eq!(result_of(&parts, &idle)["message"], "not printing");
        let offline_part = result_of(&parts, &offline);
        assert_eq!(offline_part["result"], "rejected");
        assert!(
            offline_part["message"]
                .as_str()
                .unwrap()
                .contains("disconnected")
        );
        until(&client, &a, |v| {
            v.status.state == Some(PrinterState::Paused)
        })
        .await;
        until(&client, &b, |v| {
            v.status.state == Some(PrinterState::Paused)
        })
        .await;

        // One audit entry per Printer, each its own.
        assert_eq!(
            client.state.store().audit_count().await.unwrap(),
            before + 4
        );
        let entries = client
            .state
            .store()
            .audit_entries(&Filter {
                action: Some("printer.command".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let bulk: Vec<_> = entries
            .iter()
            .filter(|e| e.detail.as_ref().is_some_and(|d| d["bulk"] == true))
            .collect();
        assert_eq!(bulk.len(), 4);
        let results: Vec<&str> = {
            let mut r: Vec<&str> = bulk
                .iter()
                .map(|e| match e.result {
                    crate::store::audit::Outcome::Ok => "ok",
                    crate::store::audit::Outcome::Refused => "refused",
                    crate::store::audit::Outcome::Failed => "failed",
                    crate::store::audit::Outcome::Skipped => "skipped",
                })
                .collect();
            r.sort_unstable();
            r
        };
        assert_eq!(results, ["ok", "ok", "refused", "skipped"]);

        // A selection: only those, in that order, with an unknown id rejected.
        let (_, parts) = client
            .post(
                "/api/bulk/commands",
                json!({ "command": "resume", "printers": [b, "nope", a] }),
            )
            .await;
        let ids: Vec<&str> = parts
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["printer_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [b.as_str(), "nope", a.as_str()]);
        assert_eq!(result_of(&parts, "nope")["result"], "rejected");
        assert_eq!(result_of(&parts, &a)["result"], "done");
    }

    #[tokio::test]
    async fn bulk_actions_need_an_operator() {
        let client = Client::new().await;
        let viewer = client.as_role(Role::Viewer).await;
        let (status, _) = viewer
            .post("/api/bulk/commands", json!({ "command": "pause" }))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let operator = client.as_role(Role::Operator).await;
        let (status, _) = operator
            .post(
                "/api/bulk/commands",
                json!({ "command": "cancel", "printers": [] }),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
}
