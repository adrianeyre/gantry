//! The JSON API under `/api`. Every route here sits behind the session gate.
use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use gantry_core::{CommandError, FieldError, FleetError};
use serde_json::json;

use crate::AppState;

pub mod account;
pub mod alerts;
#[cfg(test)]
mod alerts_tests;
pub mod audit;
pub mod bulk;
#[cfg(test)]
mod capabilities_tests;
pub mod commands;
pub mod config;
#[cfg(test)]
mod config_tests;
pub mod discovery;
pub mod history;
#[cfg(test)]
mod jobs_tests;
#[cfg(test)]
mod login_tests;
pub mod mark_clear;
pub mod openapi;
#[cfg(test)]
mod openapi_tests;
pub mod passkeys;
#[cfg(test)]
mod passkeys_tests;
pub mod print_files;
#[cfg(test)]
mod print_files_tests;
pub mod printers;
pub mod queue;
#[cfg(test)]
mod queue_tests;
#[cfg(test)]
mod reattach_tests;
#[cfg(test)]
mod roles_tests;
pub mod settings;
#[cfg(feature = "simulated")]
pub mod simulation;
pub mod spools;
pub mod tags;
pub mod timelapses;
#[cfg(test)]
mod timelapses_tests;
pub mod tokens;
#[cfg(test)]
mod tokens_tests;
#[cfg(test)]
mod totp_tests;
pub mod users;
pub mod webhooks;
#[cfg(test)]
mod webhooks_tests;

pub fn routes() -> Router<AppState> {
    let routes = Router::new()
        .route("/drivers", get(printers::drivers))
        .route("/printers", get(printers::list).post(printers::create))
        .route(
            "/printers/{id}",
            get(printers::get)
                .put(printers::update)
                .delete(printers::remove),
        )
        .route("/printers/{id}/jobs", get(printers::jobs))
        .route("/printers/{id}/history", get(history::history))
        .route("/analytics/utilisation", get(history::utilisation))
        .route("/spools", get(spools::list).post(spools::create))
        .route(
            "/spools/{id}",
            axum::routing::put(spools::update).delete(spools::remove),
        )
        .route(
            "/printers/{id}/capabilities",
            axum::routing::put(printers::set_capabilities),
        )
        .route(
            "/printers/{id}/automation",
            axum::routing::put(printers::set_automation),
        )
        .route("/printers/{id}/commands", post(commands::send))
        .route("/printers/{id}/mark-clear", post(mark_clear::mark_clear))
        .route("/printers/{id}/camera", get(crate::camera::stream))
        .route("/printers/{id}/start", post(print_files::start))
        .route(
            "/printers/{id}/tags",
            axum::routing::put(tags::set_for_printer),
        )
        .route("/tags", get(tags::list).post(tags::create))
        .route("/settings", get(settings::get).put(settings::update))
        .route("/alerts/channel-kinds", get(alerts::kinds))
        .route(
            "/alerts/channels",
            get(alerts::list_channels).post(alerts::create_channel),
        )
        .route(
            "/alerts/channels/{id}",
            axum::routing::put(alerts::update_channel).delete(alerts::remove_channel),
        )
        .route("/alerts/channels/{id}/test", post(alerts::test_channel))
        .route(
            "/alerts/rules",
            get(alerts::list_rules).post(alerts::create_rule),
        )
        .route(
            "/alerts/rules/{id}",
            axum::routing::put(alerts::update_rule).delete(alerts::remove_rule),
        )
        .route("/queue", get(queue::list).post(queue::add))
        .route("/queue/order", axum::routing::put(queue::reorder))
        .route("/queue/{id}", axum::routing::delete(queue::remove))
        .route("/queue/{id}/start", post(queue::start))
        .route("/tags/{id}", axum::routing::delete(tags::remove))
        .route(
            "/print-files",
            get(print_files::list).post(print_files::upload).layer(
                axum::extract::DefaultBodyLimit::max(print_files::MAX_UPLOAD_BYTES),
            ),
        )
        .route(
            "/print-files/{id}",
            get(print_files::get)
                .put(print_files::update)
                .delete(print_files::remove),
        )
        .route(
            "/print-files/{id}/plates/{plate}/thumbnail",
            get(print_files::thumbnail),
        )
        .route("/print-files/{id}/model", get(print_files::model))
        .route(
            "/print-files/{id}/plates/{plate}/toolpath",
            get(print_files::toolpath),
        )
        .route("/audit", get(audit::list))
        .route("/bulk/commands", post(bulk::send))
        .route("/config/export", get(config::export))
        .route("/config/import/preview", post(config::preview))
        .route("/config/import", post(config::import))
        .route("/users", get(users::list).post(users::create))
        .route(
            "/users/{id}",
            axum::routing::put(users::update).delete(users::remove),
        )
        .route("/users/{id}/unlock", post(users::unlock))
        .route("/tokens", get(tokens::list).post(tokens::create))
        .route("/tokens/{id}", axum::routing::delete(tokens::revoke))
        .route("/webhooks", get(webhooks::list).post(webhooks::create))
        .route(
            "/webhooks/{id}",
            axum::routing::put(webhooks::update).delete(webhooks::remove),
        )
        .route("/webhooks/{id}/test", post(webhooks::test))
        .route("/account", get(account::get))
        .route("/account/totp/setup", post(account::setup))
        .route("/account/totp/enable", post(account::enable))
        .route("/account/totp/disable", post(account::disable))
        .route(
            "/account/totp/recovery-codes",
            post(account::new_recovery_codes),
        )
        .route("/users/{id}/totp", axum::routing::delete(account::reset))
        .route(
            "/account/passkeys/register/start",
            post(passkeys::register_start),
        )
        .route(
            "/account/passkeys/register/finish",
            post(passkeys::register_finish),
        )
        .route(
            "/account/passkeys/{id}",
            axum::routing::delete(passkeys::remove_own),
        )
        .route("/users/{id}/passkeys", get(passkeys::list_for_user))
        .route("/discovery", get(discovery::list))
        .route(
            "/printers/{id}/timelapse",
            get(timelapses::setting).put(timelapses::set),
        )
        .route(
            "/jobs/{id}/timelapse",
            get(timelapses::get).delete(timelapses::remove),
        )
        .route(
            "/jobs/{id}/timelapse/frames/{frame}",
            get(timelapses::frame),
        )
        .route(
            "/users/{id}/passkeys/{passkey}",
            axum::routing::delete(passkeys::remove_for_user),
        );
    #[cfg(feature = "simulated")]
    let routes = routes.route(
        "/printers/{id}/simulation",
        get(simulation::show).post(simulation::act),
    );
    routes
}

/// An API failure, as JSON: `{ "error": message }`, plus `fields` for
/// validation errors so the UI can show each one inline.
#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    Forbidden(String),
    Invalid(Vec<FieldError>),
    Conflict(String),
    Command(CommandError),
    Internal(String),
}

impl ApiError {
    pub fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!(error = %e, "request failed");
        Self::Internal("something went wrong; see the server log".into())
    }

    /// The message the client is shown.
    pub fn message(&self) -> String {
        match self {
            Self::NotFound(m) | Self::Forbidden(m) | Self::Conflict(m) | Self::Internal(m) => {
                m.clone()
            }
            Self::Invalid(fields) => fields
                .iter()
                .map(|f| format!("{}: {}", f.field, f.message))
                .collect::<Vec<_>>()
                .join("; "),
            Self::Command(c) => c.to_string(),
        }
    }

    pub fn field(field: &str, message: &str) -> Self {
        Self::Invalid(vec![FieldError {
            field: field.into(),
            message: message.into(),
        }])
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::internal(e)
    }
}

impl From<FleetError> for ApiError {
    fn from(e: FleetError) -> Self {
        match e {
            FleetError::UnknownPrinter(_) => Self::NotFound("no such Printer".into()),
            FleetError::Command(c) => Self::Command(c),
            other => Self::internal(other),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound(m) => {
                (StatusCode::NOT_FOUND, Json(json!({ "error": m }))).into_response()
            }
            Self::Forbidden(m) => {
                (StatusCode::FORBIDDEN, Json(json!({ "error": m }))).into_response()
            }
            Self::Conflict(m) => {
                (StatusCode::CONFLICT, Json(json!({ "error": m }))).into_response()
            }
            Self::Invalid(fields) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "Some fields need attention", "fields": fields })),
            )
                .into_response(),
            Self::Command(c) => {
                let status = match c {
                    CommandError::TimedOut => StatusCode::GATEWAY_TIMEOUT,
                    CommandError::Invalid { .. } => StatusCode::UNPROCESSABLE_ENTITY,
                    _ => StatusCode::CONFLICT,
                };
                (status, Json(json!({ "error": c.to_string(), "reason": c }))).into_response()
            }
            Self::Internal(m) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": m })),
            )
                .into_response(),
        }
    }
}
