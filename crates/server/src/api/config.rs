//! Config export and import, for Admins (PRD §5.4). See [`crate::config_io`].
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::json;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::config_io::{self, Plan};
use crate::store::audit::Action;
use crate::store::users::User;

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    /// Include Printer credentials, in plain text.
    #[serde(default)]
    credentials: bool,
}

/// `GET /api/config/export?credentials=true|false`: the TOML file.
pub async fn export(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let result = config_io::export(state.store(), state.key(), query.credentials)
        .await
        .map_err(ApiError::internal);
    audit::record(
        &state,
        audit::by(&user, Action::ConfigExported)
            .detail(json!({ "credentials": query.credentials }))
            .result(&result),
    )
    .await;
    let body = result?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/toml; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"gantry-config.toml\"",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response())
}

/// An import: the file's text, and whether to remove what it doesn't name.
#[derive(Deserialize)]
pub struct ImportForm {
    toml: String,
    #[serde(default)]
    prune: bool,
}

impl std::fmt::Debug for ImportForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportForm")
            .field("bytes", &self.toml.len())
            .field("prune", &self.prune)
            .finish()
    }
}

async fn plan(state: &AppState, form: &ImportForm) -> Result<Plan, ApiError> {
    config_io::plan(
        state.store(),
        state.key(),
        state.fleet().registry(),
        &form.toml,
        form.prune,
    )
    .await
    .map_err(ApiError::internal)
}

/// `POST /api/config/import/preview`: what the import would change. Nothing
/// is written.
pub async fn preview(
    State(state): State<AppState>,
    Json(form): Json<ImportForm>,
) -> Result<Json<Plan>, ApiError> {
    Ok(Json(plan(&state, &form).await?))
}

/// `POST /api/config/import`: applies the import, all of it or none of it,
/// then restarts the Printers it added or changed.
pub async fn import(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<ImportForm>,
) -> Result<Response, ApiError> {
    let plan = plan(&state, &form).await?;
    if !plan.errors.is_empty() {
        let message = format!(
            "the import has {} problem(s); nothing was changed",
            plan.errors.len()
        );
        audit::record(
            &state,
            audit::by(&user, Action::ConfigImported)
                .detail(json!({ "errors": plan.errors }))
                .result::<()>(&Err(ApiError::Conflict(message.clone()))),
        )
        .await;
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": message, "plan": plan })),
        )
            .into_response());
    }
    let result = config_io::apply(state.store(), state.key(), &plan)
        .await
        .map_err(ApiError::internal);
    audit::record(
        &state,
        audit::by(&user, Action::ConfigImported)
            .detail(json!({ "changes": plan.changes, "prune": form.prune }))
            .result(&result),
    )
    .await;
    let applied = result?;
    for id in &applied.removed {
        if state.fleet().contains(id) {
            let _ = state.fleet().stop(id).await;
        }
    }
    for id in &applied.started {
        if let Err(e) = super::printers::start(&state, id).await {
            tracing::warn!(printer = %id, error = %e, "could not start an imported Printer");
        }
    }
    if let Err(e) = super::settings::apply(&state).await {
        tracing::warn!(error = %e, "could not apply the imported settings");
    }
    tracing::info!(changes = plan.changes.len(), "imported a config file");
    Ok(Json(plan).into_response())
}
