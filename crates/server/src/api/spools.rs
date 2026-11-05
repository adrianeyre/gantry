//! The filament inventory (PRD §7.5): spools with type, colour, weight and
//! cost. Operators manage it; Viewers only see it. A spool loaded on a
//! Printer is used up by its Jobs (see [`crate::history`]).
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use gantry_core::FieldError;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::history::{Spool, SpoolInput};
use crate::store::users::User;

fn valid_colour(colour: &str) -> bool {
    let colour = colour.trim();
    colour.len() == 7
        && colour.starts_with('#')
        && colour[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Checks a spool, and that the Printer it is loaded on exists.
async fn check(state: &AppState, mut input: SpoolInput) -> Result<SpoolInput, ApiError> {
    let mut errors = Vec::new();
    let mut error = |field: &str, message: &str| {
        errors.push(FieldError {
            field: field.into(),
            message: message.into(),
        });
    };
    if input.kind.trim().is_empty() {
        error("kind", "Say what material it is, such as PLA");
    }
    if !valid_colour(&input.colour) {
        error("colour", "Use a colour such as #FF8000");
    }
    if !(input.weight_g.is_finite() && input.weight_g > 0.0) {
        error("weight_g", "Must be more than 0 g");
    }
    if let Some(left) = input.remaining_g
        && !(left.is_finite() && (0.0..=input.weight_g).contains(&left))
    {
        error("remaining_g", "Must be between 0 g and its full weight");
    }
    if !(input.cost.is_finite() && input.cost >= 0.0) {
        error("cost", "Must be 0 or more");
    }
    input.name = input
        .name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty());
    input.printer_id = input.printer_id.filter(|p| !p.is_empty());
    input.slot_id = input.slot_id.filter(|s| !s.is_empty());
    if input.printer_id.is_none() {
        input.slot_id = None;
    }
    if let Some(printer) = &input.printer_id
        && state.store().printer(state.key(), printer).await?.is_none()
    {
        error("printer_id", "No such Printer");
    }
    if !errors.is_empty() {
        return Err(ApiError::Invalid(errors));
    }
    Ok(input)
}

fn detail(spool: &Spool) -> serde_json::Value {
    serde_json::json!({
        "spool": spool.id,
        "kind": spool.kind,
        "colour": spool.colour,
        "remaining_g": spool.remaining_g,
        "printer_id": spool.printer_id,
    })
}

/// `GET /api/spools`: the inventory.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Spool>>, ApiError> {
    Ok(Json(state.store().spools().await?))
}

/// `POST /api/spools`: adds a spool.
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(input): Json<SpoolInput>,
) -> Result<(StatusCode, Json<Spool>), ApiError> {
    let result = async {
        let input = check(&state, input).await?;
        Ok(state.store().insert_spool(&input).await?)
    }
    .await;
    let mut entry = audit::by(&user, Action::SpoolCreated).result(&result);
    if let Ok(spool) = &result {
        entry = entry.target(spool.kind.clone()).detail(detail(spool));
    }
    audit::record(&state, entry).await;
    result.map(|s| (StatusCode::CREATED, Json(s)))
}

/// `PUT /api/spools/{id}`: changes a spool, such as loading it on a Printer
/// or correcting what is left.
pub async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
    Json(input): Json<SpoolInput>,
) -> Result<Json<Spool>, ApiError> {
    let result = async {
        let input = check(&state, input).await?;
        state
            .store()
            .update_spool(id, &input)
            .await?
            .ok_or_else(|| ApiError::NotFound("no such spool".into()))
    }
    .await;
    let mut entry = audit::by(&user, Action::SpoolUpdated)
        .target(id.to_string())
        .result(&result);
    if let Ok(spool) = &result {
        entry = entry.target(spool.kind.clone()).detail(detail(spool));
    }
    audit::record(&state, entry).await;
    result.map(Json)
}

/// `DELETE /api/spools/{id}`: removes a spool. Jobs that used it keep their
/// filament and cost.
pub async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let result = async {
        if !state.store().delete_spool(id).await? {
            return Err(ApiError::NotFound("no such spool".into()));
        }
        Ok(())
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::SpoolRemoved)
            .target(id.to_string())
            .result(&result),
    )
    .await;
    result.map(|()| StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::store::users::Role;
    use crate::test_support::Client;

    #[tokio::test]
    async fn operators_manage_the_inventory_and_viewers_only_see_it() {
        let client = Client::new().await;
        let operator = client.as_role(Role::Operator).await;
        let viewer = client.as_role(Role::Viewer).await;
        let spool = json!({ "name": "Black PETG", "kind": "PETG", "colour": "#000000",
                            "weight_g": 1000, "remaining_g": 500, "cost": 22.5 });
        let (status, _) = viewer.post("/api/spools", spool.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, created) = operator.post("/api/spools", spool).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["remaining_g"], 500.0);
        let id = created["id"].clone();

        let (status, list) = viewer.get("/api/spools").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list.as_array().unwrap().len(), 1);
        let (status, _) = viewer
            .put(
                &format!("/api/spools/{id}"),
                json!({ "kind": "PLA", "colour": "#000000",
                                                        "weight_g": 1000, "cost": 1 }),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = viewer.delete(&format!("/api/spools/{id}")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, updated) = operator
            .put(
                &format!("/api/spools/{id}"),
                json!({ "kind": "PETG", "colour": "#000000", "weight_g": 1000,
                        "remaining_g": 250, "cost": 22.5 }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["remaining_g"], 250.0);
        let (status, _) = operator.delete(&format!("/api/spools/{id}")).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (_, audit) = client.get("/api/audit").await;
        let actions: Vec<_> = audit["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["action"].as_str().unwrap())
            .filter(|a| a.starts_with("spool."))
            .collect();
        assert_eq!(actions, ["spool.removed", "spool.updated", "spool.created"]);
    }

    #[tokio::test]
    async fn a_spool_is_checked() {
        let client = Client::new().await;
        let (status, body) = client
            .post(
                "/api/spools",
                json!({ "kind": " ", "colour": "red", "weight_g": 0, "cost": -1,
                        "printer_id": "nope" }),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let fields: Vec<_> = body["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["field"].as_str().unwrap())
            .collect();
        assert_eq!(fields, ["kind", "colour", "weight_g", "cost", "printer_id"]);
    }
}
