//! Alert channels and Alert Rules (PRD §7.4), for Admins. A channel's
//! credentials are encrypted at rest and never sent back.
use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use gantry_core::{FieldError, FieldKind, Secrets};
use serde::{Deserialize, Serialize};

use super::ApiError;
use crate::AppState;
use crate::alerts::{Alert, Event, EventKind, Level, channels};
use crate::audit::{self, EntryExt};
use crate::store::alerts::{ChannelSummary, Rule, RuleInput, StoredChannel};
use crate::store::audit::Action;
use crate::store::users::User;

/// `GET /api/alerts/channel-kinds`: what kinds of channel there are, with
/// the settings each needs.
pub async fn kinds() -> Json<Vec<channels::ChannelKind>> {
    Json(channels::kinds())
}

/// `GET /api/alerts/channels`.
pub async fn list_channels(
    State(state): State<AppState>,
) -> Result<Json<Vec<ChannelSummary>>, ApiError> {
    Ok(Json(
        state
            .store()
            .alert_channels(state.key())
            .await?
            .iter()
            .map(ChannelSummary::from)
            .collect(),
    ))
}

/// The body of a channel create or edit.
#[derive(Deserialize)]
pub struct ChannelForm {
    name: String,
    /// Required to create; a channel's kind can't change.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    settings: serde_json::Map<String, serde_json::Value>,
    /// On edit, a missing or blank value keeps what is stored, `null` clears it.
    #[serde(default)]
    secrets: BTreeMap<String, Option<String>>,
}

impl std::fmt::Debug for ChannelForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelForm")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("settings", &self.settings)
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}

fn field(field: &str, message: &str) -> FieldError {
    FieldError {
        field: field.into(),
        message: message.into(),
    }
}

fn validate_channel(
    id: i64,
    kind_id: &str,
    form: ChannelForm,
    stored: Secrets,
) -> Result<StoredChannel, ApiError> {
    let mut errors = Vec::new();
    let name = form.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 64 {
        errors.push(field("name", "Must be 1 to 64 characters"));
    }
    let Some(kind) = channels::kind(kind_id) else {
        errors.push(field("kind", "Choose a kind of channel"));
        return Err(ApiError::Invalid(errors));
    };
    let settings: serde_json::Map<_, _> = form
        .settings
        .into_iter()
        .filter(|(k, _)| {
            kind.fields
                .iter()
                .any(|f| &f.key == k && f.kind != FieldKind::Secret)
        })
        .collect();
    let mut secrets = stored.into_inner();
    for f in kind.fields.iter().filter(|f| f.kind == FieldKind::Secret) {
        match form.secrets.get(&f.key) {
            Some(Some(value)) if !value.trim().is_empty() => {
                secrets.insert(f.key.clone(), value.trim().to_owned());
            }
            Some(None) => {
                secrets.remove(&f.key);
            }
            _ => {}
        }
    }
    secrets.retain(|k, _| kind.fields.iter().any(|f| &f.key == k));
    let secrets = Secrets::new(secrets);
    errors.extend(gantry_core::driver::validate_fields(
        &kind.fields,
        &settings,
        &secrets,
    ));
    let url_fields = [
        ("server", settings.get("server").and_then(|v| v.as_str())),
        (
            "api_server",
            settings.get("api_server").and_then(|v| v.as_str()),
        ),
        ("webhook_url", secrets.get("webhook_url")),
    ];
    if kind.id == "email" {
        for (key, message) in channels::check_email(&settings) {
            if !errors.iter().any(|e| e.field == key) {
                errors.push(field(key, &message));
            }
        }
    }
    for (key, value) in url_fields {
        if let Some(value) = value
            && let Err(message) = channels::check_url(value)
            && !errors.iter().any(|e| e.field == key)
        {
            errors.push(field(key, &message));
        }
    }
    if !errors.is_empty() {
        return Err(ApiError::Invalid(errors));
    }
    Ok(StoredChannel {
        id,
        name,
        kind: kind.id.to_owned(),
        settings,
        secrets,
    })
}

fn duplicate(e: sqlx::Error) -> ApiError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            ApiError::Invalid(vec![field("name", "Another channel has this name")])
        }
        _ => e.into(),
    }
}

/// `POST /api/alerts/channels`.
pub async fn create_channel(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<ChannelForm>,
) -> Result<(StatusCode, Json<ChannelSummary>), ApiError> {
    let name = form.name.trim().to_owned();
    let kind = form.kind.clone().unwrap_or_default();
    let result = async {
        let channel = validate_channel(0, &kind, form, Secrets::default())?;
        let id = state
            .store()
            .save_alert_channel(state.key(), &channel)
            .await
            .map_err(duplicate)?;
        Ok(ChannelSummary::from(&StoredChannel { id, ..channel }))
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::AlertChannelCreated)
            .target(name)
            .detail(serde_json::json!({ "kind": kind }))
            .result(&result),
    )
    .await;
    result.map(|c| (StatusCode::CREATED, Json(c)))
}

async fn find_channel(state: &AppState, id: i64) -> Result<StoredChannel, ApiError> {
    state
        .store()
        .alert_channel(state.key(), id)
        .await?
        .ok_or_else(|| ApiError::NotFound("no such Alert channel".into()))
}

/// `PUT /api/alerts/channels/{id}`.
pub async fn update_channel(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
    Json(form): Json<ChannelForm>,
) -> Result<Json<ChannelSummary>, ApiError> {
    let name = form.name.trim().to_owned();
    let result = async {
        let current = find_channel(&state, id).await?;
        let channel = validate_channel(id, &current.kind, form, current.secrets)?;
        state
            .store()
            .save_alert_channel(state.key(), &channel)
            .await
            .map_err(duplicate)?;
        Ok(ChannelSummary::from(&channel))
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::AlertChannelUpdated)
            .target(name)
            .result(&result),
    )
    .await;
    result.map(Json)
}

/// `DELETE /api/alerts/channels/{id}`: also removes the Rules that use it.
pub async fn remove_channel(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let result = async {
        let channel = find_channel(&state, id).await?;
        state.store().delete_alert_channel(id).await?;
        Ok(channel.name)
    }
    .await;
    let target = result
        .as_ref()
        .map_or_else(|_| id.to_string(), Clone::clone);
    audit::record(
        &state,
        audit::by(&user, Action::AlertChannelRemoved)
            .target(target)
            .result(&result),
    )
    .await;
    result.map(|_| StatusCode::NO_CONTENT)
}

/// The answer to a test send.
#[derive(Debug, Serialize)]
pub struct TestResult {
    pub sent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /api/alerts/channels/{id}/test`: sends one test Alert, once, and
/// says whether it got through.
pub async fn test_channel(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<Json<TestResult>, ApiError> {
    let channel = find_channel(&state, id).await?;
    let alert = Alert {
        event: Event {
            kind: EventKind::JobEnded,
            printer_id: String::new(),
            printer_name: "Gantry".into(),
            outcome: None,
        },
        title: "Gantry: test Alert".into(),
        message: format!(
            "This is a test from Gantry, sent by {} to the channel {}.",
            user.username, channel.name
        ),
        level: Level::Info,
    };
    let outcome = match channels::build(&channel, state.desktop_alerts()) {
        Ok(sender) => sender.send(&alert).await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let result: Result<(), ApiError> = outcome
        .clone()
        .map_err(|e| ApiError::Conflict(format!("the test Alert wasn't sent: {e}")));
    audit::record(
        &state,
        audit::by(&user, Action::AlertChannelTested)
            .target(channel.name.clone())
            .result(&result),
    )
    .await;
    Ok(Json(TestResult {
        sent: outcome.is_ok(),
        error: outcome.err(),
    }))
}

/// `GET /api/alerts/rules`.
pub async fn list_rules(State(state): State<AppState>) -> Result<Json<Vec<Rule>>, ApiError> {
    Ok(Json(state.store().alert_rules().await?))
}

async fn validate_rule(state: &AppState, mut input: RuleInput) -> Result<RuleInput, ApiError> {
    let mut errors = Vec::new();
    input.name = input.name.trim().to_owned();
    if input.name.is_empty() || input.name.chars().count() > 64 {
        errors.push(field("name", "Must be 1 to 64 characters"));
    }
    input
        .events
        .sort_by_key(|e| EventKind::ALL.iter().position(|k| k == e));
    input.events.dedup();
    if input.events.is_empty() {
        errors.push(field("events", "Choose at least one event"));
    }
    if !input.events.contains(&EventKind::JobEnded) {
        input.outcomes.clear();
    }
    input.outcomes.dedup();
    if state
        .store()
        .alert_channel(state.key(), input.channel_id)
        .await?
        .is_none()
    {
        errors.push(field("channel_id", "Choose a channel"));
    }
    let printers = state.store().printer_ids().await?;
    if let Some(unknown) = input.printers.iter().find(|p| !printers.contains(p)) {
        errors.push(field("printers", &format!("there is no Printer {unknown}")));
    }
    let tags = state.store().tags().await?;
    if let Some(unknown) = input
        .tags
        .iter()
        .find(|t| !tags.iter().any(|k| k.id == **t))
    {
        errors.push(field("tags", &format!("there is no Tag {unknown}")));
    }
    if errors.is_empty() {
        Ok(input)
    } else {
        Err(ApiError::Invalid(errors))
    }
}

async fn save_rule(
    state: &AppState,
    user: &User,
    id: Option<i64>,
    input: RuleInput,
) -> Result<Rule, ApiError> {
    let name = input.name.trim().to_owned();
    let result = async {
        if let Some(id) = id
            && state.store().alert_rule(id).await?.is_none()
        {
            return Err(ApiError::NotFound("no such Alert Rule".into()));
        }
        let input = validate_rule(state, input).await?;
        let id = state.store().save_alert_rule(id, &input).await?;
        state
            .store()
            .alert_rule(id)
            .await?
            .ok_or_else(|| ApiError::internal("a saved Alert Rule vanished"))
    }
    .await;
    let action = if id.is_some() {
        Action::AlertRuleUpdated
    } else {
        Action::AlertRuleCreated
    };
    let mut entry = audit::by(user, action).target(name).result(&result);
    if let Ok(rule) = &result {
        entry = entry.detail(serde_json::json!({
            "events": rule.events,
            "outcomes": rule.outcomes,
            "printers": rule.printers,
            "tags": rule.tags,
            "channel_id": rule.channel_id,
            "enabled": rule.enabled,
        }));
    }
    audit::record(state, entry).await;
    result
}

/// `POST /api/alerts/rules`.
pub async fn create_rule(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(input): Json<RuleInput>,
) -> Result<(StatusCode, Json<Rule>), ApiError> {
    save_rule(&state, &user, None, input)
        .await
        .map(|r| (StatusCode::CREATED, Json(r)))
}

/// `PUT /api/alerts/rules/{id}`.
pub async fn update_rule(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
    Json(input): Json<RuleInput>,
) -> Result<Json<Rule>, ApiError> {
    save_rule(&state, &user, Some(id), input).await.map(Json)
}

/// `DELETE /api/alerts/rules/{id}`.
pub async fn remove_rule(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let result = async {
        let rule = state
            .store()
            .alert_rule(id)
            .await?
            .ok_or_else(|| ApiError::NotFound("no such Alert Rule".into()))?;
        state.store().delete_alert_rule(id).await?;
        Ok(rule.name)
    }
    .await;
    let target = result
        .as_ref()
        .map_or_else(|_| id.to_string(), Clone::clone);
    audit::record(
        &state,
        audit::by(&user, Action::AlertRuleRemoved)
            .target(target)
            .result(&result),
    )
    .await;
    result.map(|_| StatusCode::NO_CONTENT)
}
