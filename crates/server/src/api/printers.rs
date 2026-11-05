//! Adding, editing and removing Printers (PRD §7.6). The Admin chooses a
//! Driver and fills in its fields; credential fields are encrypted at rest and
//! never sent back (ADR-0005).
use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use gantry_core::{DriverInfo, FieldError, FieldKind, Secrets};
use serde::Deserialize;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::printers::{Automation, PrinterInput, PrinterRecord, SavePrinterError};
use crate::store::users::User;

/// Starts (or restarts) one stored Printer in the Fleet.
pub async fn start(state: &AppState, id: &str) -> anyhow::Result<()> {
    let Some((driver, config, decrypt_error)) =
        state.store().printer_config(state.key(), id).await?
    else {
        anyhow::bail!("no such Printer");
    };
    if decrypt_error.is_some() {
        tracing::warn!(
            printer = %config.name,
            "this Printer's credentials can't be read with the current secret key; re-enter them"
        );
    }
    if state.fleet().contains(id) {
        state.fleet().stop(id).await?;
    }
    let jobs = state.store().job_state(id).await?;
    let overrides = state.store().capability_overrides(id).await?;
    let automation = state.store().automation(id).await?.unwrap_or_default();
    state.fleet().start_with(
        &driver,
        config,
        jobs,
        gantry_core::StartOptions {
            overrides,
            auto_clear: automation.auto_clear,
        },
    )?;
    Ok(())
}

/// The body of `PUT /api/printers/{id}/automation`: either or both.
#[derive(Debug, Deserialize)]
pub struct AutomationForm {
    auto_start: Option<bool>,
    auto_clear: Option<bool>,
}

/// `PUT /api/printers/{id}/automation`: an Admin turns Auto-start (PRD §7.3)
/// or Auto-Clear (PRD §6.3) on or off. Applies at once, without
/// reconnecting the Printer.
pub async fn set_automation(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(form): Json<AutomationForm>,
) -> Result<Json<Automation>, ApiError> {
    let result = async {
        let mut automation = state
            .store()
            .automation(&id)
            .await?
            .ok_or_else(|| ApiError::NotFound("no such Printer".into()))?;
        if let Some(on) = form.auto_start {
            automation.auto_start = on;
        }
        if let Some(on) = form.auto_clear {
            automation.auto_clear = on;
        }
        state.store().set_automation(&id, automation).await?;
        if state.fleet().contains(&id) {
            state.fleet().set_auto_clear(&id, automation.auto_clear)?;
        }
        Ok(automation)
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::PrinterAutomation)
            .printer(&id)
            .detail(serde_json::json!({
                "auto_start": form.auto_start,
                "auto_clear": form.auto_clear,
            }))
            .result(&result),
    )
    .await;
    result.map(Json)
}

/// `GET /api/printers/{id}/jobs`: the Printer's Jobs, newest first.
pub async fn jobs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<gantry_core::JobRecord>>, ApiError> {
    if state.store().printer(state.key(), &id).await?.is_none() {
        return Err(ApiError::NotFound("no such Printer".into()));
    }
    Ok(Json(state.store().printer_jobs(&id, 100).await?))
}

/// The body of `PUT /api/printers/{id}/capabilities`.
#[derive(Debug, Deserialize)]
pub struct OverridesForm {
    /// `true` adds a capability, `false` removes it; one left out follows
    /// the Driver. An empty map resets every override.
    overrides: gantry_core::Overrides,
}

/// `PUT /api/printers/{id}/capabilities`: an Admin overrides what the Driver
/// reports (PRD §6.1). Stored, so it survives a restart and a reconnect.
pub async fn set_capabilities(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(form): Json<OverridesForm>,
) -> Result<Json<gantry_core::PrinterView>, ApiError> {
    let result = async {
        if state.store().printer(state.key(), &id).await?.is_none() {
            return Err(ApiError::NotFound("no such Printer".into()));
        }
        state
            .store()
            .set_capability_overrides(&id, &form.overrides)
            .await?;
        state.fleet().set_overrides(&id, form.overrides.clone())?;
        let overrides = form.overrides.clone();
        Ok(state
            .fleet()
            .wait_for(&id, std::time::Duration::from_secs(5), move |v| {
                v.capability_overrides == overrides
            })
            .await?)
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::CapabilitiesOverridden)
            .printer(&id)
            .detail(serde_json::json!({ "overrides": form.overrides }))
            .result(&result),
    )
    .await;
    Ok(Json((*result?).clone()))
}

/// `GET /api/drivers`: the Drivers compiled in, with the fields each needs.
pub async fn drivers(State(state): State<AppState>) -> Json<Vec<DriverInfo>> {
    Json(state.fleet().registry().list())
}

/// `GET /api/printers`.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<PrinterRecord>>, ApiError> {
    Ok(Json(state.store().printers(state.key()).await?))
}

/// `GET /api/printers/{id}`.
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PrinterRecord>, ApiError> {
    state
        .store()
        .printer(state.key(), &id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("no such Printer".into()))
}

/// The body of a create or edit.
#[derive(Deserialize)]
pub struct PrinterForm {
    name: String,
    /// Required to create; ignored on edit (a Printer's Driver can't change).
    #[serde(default)]
    driver: Option<String>,
    #[serde(default)]
    settings: serde_json::Map<String, serde_json::Value>,
    /// Credential values. On edit, a missing or blank value keeps what is
    /// stored and `null` clears it.
    #[serde(default)]
    secrets: BTreeMap<String, Option<String>>,
}

impl std::fmt::Debug for PrinterForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrinterForm")
            .field("name", &self.name)
            .field("driver", &self.driver)
            .field("settings", &self.settings)
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Checks a form against its Driver and builds what gets stored.
fn validate(
    state: &AppState,
    driver_id: &str,
    form: PrinterForm,
    stored_secrets: Secrets,
) -> Result<PrinterInput, ApiError> {
    let mut errors = Vec::new();
    let name = form.name.trim().to_owned();
    if name.is_empty() {
        errors.push(field_error("name", "Required"));
    } else if name.chars().count() > 64 {
        errors.push(field_error("name", "Must be 64 characters or fewer"));
    }
    let Some(driver) = state.fleet().registry().get(driver_id) else {
        errors.push(field_error("driver", "Choose a Driver"));
        return Err(ApiError::Invalid(errors));
    };
    let fields = driver.fields();

    // Only the Driver's own non-secret fields are kept.
    let settings: serde_json::Map<_, _> = form
        .settings
        .into_iter()
        .filter(|(k, _)| {
            fields
                .iter()
                .any(|f| &f.key == k && f.kind != FieldKind::Secret)
        })
        .collect();

    let mut secrets = stored_secrets.into_inner();
    for field in fields.iter().filter(|f| f.kind == FieldKind::Secret) {
        match form.secrets.get(&field.key) {
            Some(Some(value)) if !value.is_empty() => {
                secrets.insert(field.key.clone(), value.clone());
            }
            Some(None) => {
                secrets.remove(&field.key);
            }
            _ => {}
        }
    }
    secrets.retain(|k, _| fields.iter().any(|f| &f.key == k));
    let secrets = Secrets::new(secrets);

    errors.extend(driver.validate(&settings, &secrets));
    if !errors.is_empty() {
        return Err(ApiError::Invalid(errors));
    }
    Ok(PrinterInput {
        name,
        driver: driver_id.to_owned(),
        settings,
        secrets,
    })
}

fn field_error(field: &str, message: &str) -> FieldError {
    FieldError {
        field: field.into(),
        message: message.into(),
    }
}

fn save_error(e: SavePrinterError) -> ApiError {
    match e {
        SavePrinterError::NameTaken(_) => {
            ApiError::field("name", "Another Printer already has this name")
        }
        SavePrinterError::NotFound => ApiError::NotFound("no such Printer".into()),
        SavePrinterError::Database(e) => ApiError::internal(e),
    }
}

/// What an audit entry says about a Printer form: names only, never a
/// credential's value.
fn form_detail(form: &PrinterForm) -> serde_json::Value {
    serde_json::json!({
        "name": form.name.trim(),
        "driver": form.driver,
        // Keys only: a careless client could put a credential in `settings`.
        "settings": form.settings.keys().collect::<Vec<_>>(),
        "credentials_changed": form.secrets.keys().collect::<Vec<_>>(),
    })
}

/// `POST /api/printers`: stores the Printer and starts it.
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<PrinterForm>,
) -> Result<Response, ApiError> {
    let detail = form_detail(&form);
    let result = async {
        let driver = form.driver.clone().unwrap_or_default();
        let input = validate(&state, &driver, form, Secrets::default())?;
        let id = uuid::Uuid::new_v4().to_string();
        let record = state
            .store()
            .insert_printer(state.key(), &id, &input)
            .await
            .map_err(save_error)?;
        start(&state, &id).await.map_err(ApiError::internal)?;
        Ok(record)
    }
    .await;
    let mut entry = audit::by(&user, Action::PrinterCreated)
        .detail(detail)
        .result(&result);
    if let Ok(record) = &result {
        entry = entry.printer(&record.id);
        entry.printer_name = Some(record.name.clone());
    }
    audit::record(&state, entry).await;
    let record = result?;
    tracing::info!(printer = %record.name, driver = %record.driver, "added a Printer");
    Ok((StatusCode::CREATED, Json(record)).into_response())
}

/// `PUT /api/printers/{id}`: saves the changes and restarts the Printer.
pub async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(form): Json<PrinterForm>,
) -> Result<Json<PrinterRecord>, ApiError> {
    let detail = form_detail(&form);
    let result = async {
        let existing = state
            .store()
            .printer(state.key(), &id)
            .await?
            .ok_or_else(|| ApiError::NotFound("no such Printer".into()))?;
        let stored = state.store().printer_secrets(state.key(), &id).await?;
        let input = validate(&state, &existing.driver, form, stored)?;
        let record = state
            .store()
            .update_printer(state.key(), &id, &input)
            .await
            .map_err(save_error)?;
        start(&state, &id).await.map_err(ApiError::internal)?;
        Ok(record)
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::PrinterUpdated)
            .printer(&id)
            .detail(detail)
            .result(&result),
    )
    .await;
    let record = result?;
    tracing::info!(printer = %record.name, "edited a Printer");
    Ok(Json(record))
}

/// `DELETE /api/printers/{id}`: stops the Printer's task and forgets it.
pub async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Named before it's gone, so the log can say which Printer it was.
    let name = state
        .store()
        .printer(state.key(), &id)
        .await?
        .map(|p| p.name);
    let result = async {
        if state.fleet().contains(&id) {
            state.fleet().stop(&id).await?;
        }
        if !state.store().delete_printer(&id).await? {
            return Err(ApiError::NotFound("no such Printer".into()));
        }
        Ok(())
    }
    .await;
    let mut entry = audit::by(&user, Action::PrinterRemoved)
        .printer(&id)
        .result(&result);
    entry.printer_name = name;
    audit::record(&state, entry).await;
    result?;
    tracing::info!(printer = %id, "removed a Printer");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
