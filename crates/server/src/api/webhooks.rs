//! Webhooks (PRD §7.7), managed by Admins. The signing secret is made by
//! Gantry and shown once, when the webhook is made or its secret replaced.
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use gantry_core::{FieldError, JobOutcome};
use serde::{Deserialize, Serialize};

use super::ApiError;
use crate::AppState;
use crate::alerts::EventKind;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::users::User;
use crate::store::webhooks::{Webhook, WebhookSummary, new_secret};
use crate::webhooks::{Backoff, Payload, TryResult};

/// What an Admin sends to make or change a webhook. Every field is optional
/// when changing one.
#[derive(Debug, Default, Deserialize)]
pub struct WebhookForm {
    name: Option<String>,
    url: Option<String>,
    enabled: Option<bool>,
    events: Option<Vec<EventKind>>,
    outcomes: Option<Vec<JobOutcome>>,
    printers: Option<Vec<String>>,
    tags: Option<Vec<i64>>,
    /// Makes a new signing secret, which the answer shows once.
    #[serde(default)]
    new_secret: bool,
}

/// A webhook, with its secret when it was just made.
#[derive(Debug, Serialize)]
pub struct WebhookAnswer {
    #[serde(flatten)]
    webhook: WebhookSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<String>,
}

fn field(field: &str, message: &str) -> FieldError {
    FieldError {
        field: field.into(),
        message: message.into(),
    }
}

async fn validate(state: &AppState, hook: &Webhook) -> Result<(), ApiError> {
    let mut errors = Vec::new();
    if hook.name.is_empty() || hook.name.chars().count() > 64 {
        errors.push(field("name", "Must be 1 to 64 characters"));
    }
    if let Err(message) = crate::alerts::channels::check_url(&hook.url) {
        errors.push(field("url", &message));
    }
    if hook.events.is_empty() {
        errors.push(field("events", "Choose at least one event"));
    }
    let known = state.store().printer_ids().await?;
    if hook.printers.iter().any(|p| !known.contains(p)) {
        errors.push(field("printers", "No such Printer"));
    }
    let tags: Vec<i64> = state.store().tags().await?.iter().map(|t| t.id).collect();
    if hook.tags.iter().any(|t| !tags.contains(t)) {
        errors.push(field("tags", "No such Tag"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError::Invalid(errors))
    }
}

fn duplicate(e: sqlx::Error) -> ApiError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            ApiError::field("name", "Another webhook has this name")
        }
        _ => ApiError::internal(e),
    }
}

fn apply(hook: &mut Webhook, form: &WebhookForm) {
    if let Some(name) = &form.name {
        hook.name = name.trim().to_owned();
    }
    if let Some(url) = &form.url {
        hook.url = url.trim().to_owned();
    }
    if let Some(enabled) = form.enabled {
        hook.enabled = enabled;
    }
    if let Some(events) = &form.events {
        let mut events = events.clone();
        events.sort_by_key(|e| EventKind::ALL.iter().position(|k| k == e));
        events.dedup();
        hook.events = events;
    }
    if let Some(outcomes) = &form.outcomes {
        hook.outcomes = outcomes.clone();
    }
    if let Some(printers) = &form.printers {
        hook.printers = printers.clone();
    }
    if let Some(tags) = &form.tags {
        hook.tags = tags.clone();
    }
}

/// `GET /api/webhooks`.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<WebhookSummary>>, ApiError> {
    let hooks = state.store().webhooks(state.key()).await?;
    Ok(Json(hooks.iter().map(WebhookSummary::from).collect()))
}

/// `POST /api/webhooks`.
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<WebhookForm>,
) -> Result<(StatusCode, Json<WebhookAnswer>), ApiError> {
    let mut hook = Webhook {
        id: 0,
        name: String::new(),
        url: String::new(),
        secret: new_secret(),
        enabled: true,
        events: Vec::new(),
        outcomes: Vec::new(),
        printers: Vec::new(),
        tags: Vec::new(),
    };
    apply(&mut hook, &form);
    let result = async {
        validate(&state, &hook).await?;
        hook.id = state
            .store()
            .save_webhook(state.key(), &hook)
            .await
            .map_err(duplicate)?;
        Ok(())
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::WebhookCreated)
            .target(hook.name.clone())
            .detail(serde_json::json!({ "events": hook.events }))
            .result(&result),
    )
    .await;
    result?;
    Ok((
        StatusCode::CREATED,
        Json(WebhookAnswer {
            webhook: WebhookSummary::from(&hook),
            secret: Some(hook.secret),
        }),
    ))
}

async fn find(state: &AppState, id: i64) -> Result<Webhook, ApiError> {
    state
        .store()
        .webhook(state.key(), id)
        .await?
        .ok_or_else(|| ApiError::NotFound("no such webhook".into()))
}

/// `PUT /api/webhooks/{id}`.
pub async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
    Json(form): Json<WebhookForm>,
) -> Result<Json<WebhookAnswer>, ApiError> {
    let mut hook = find(&state, id).await?;
    let before = hook.name.clone();
    apply(&mut hook, &form);
    if form.new_secret {
        hook.secret = new_secret();
    }
    let result = async {
        validate(&state, &hook).await?;
        state
            .store()
            .save_webhook(state.key(), &hook)
            .await
            .map_err(duplicate)?;
        Ok(())
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::WebhookUpdated)
            .target(before)
            .detail(serde_json::json!({ "new_secret": form.new_secret, "enabled": hook.enabled }))
            .result(&result),
    )
    .await;
    result?;
    Ok(Json(WebhookAnswer {
        webhook: WebhookSummary::from(&hook),
        secret: form.new_secret.then_some(hook.secret),
    }))
}

/// `DELETE /api/webhooks/{id}`.
pub async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let hook = find(&state, id).await?;
    let result = state
        .store()
        .delete_webhook(id)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from);
    audit::record(
        &state,
        audit::by(&user, Action::WebhookRemoved)
            .target(hook.name)
            .result(&result),
    )
    .await;
    result
}

#[derive(Debug, Serialize)]
pub struct TestAnswer {
    delivered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// `POST /api/webhooks/{id}/test`: one signed `test` delivery, tried once.
pub async fn test(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<Json<TestAnswer>, ApiError> {
    let hook = find(&state, id).await?;
    let payload = Payload::test(&user.username);
    let outcome = crate::webhooks::deliver(&hook, &payload, &Backoff(Vec::new())).await;
    let answer = match outcome {
        TryResult::Delivered(status) => TestAnswer {
            delivered: true,
            status: Some(status),
            error: None,
        },
        TryResult::Retry(e) | TryResult::GiveUp(e) => TestAnswer {
            delivered: false,
            status: None,
            error: Some(e),
        },
    };
    let result: Result<(), ApiError> = match &answer.error {
        Some(e) => Err(ApiError::Conflict(format!(
            "the test delivery didn't arrive: {e}"
        ))),
        None => Ok(()),
    };
    audit::record(
        &state,
        audit::by(&user, Action::WebhookTested)
            .target(hook.name)
            .result(&result),
    )
    .await;
    Ok(Json(answer))
}
