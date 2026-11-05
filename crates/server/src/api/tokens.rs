//! API tokens (PRD §7.7): every user can make tokens for the REST API, which
//! act with that user's role. A token can't manage tokens, so a leaked one
//! can't be used to make more.
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::tokens::{CreateTokenError, TokenSummary};
use crate::store::users::{Role, User};

/// The longest a token may live, in days (10 years).
pub const MAX_DAYS: u32 = 3650;

fn not_with_a_token(user: &User) -> Result<(), ApiError> {
    if user.api_token.is_some() {
        return Err(ApiError::Forbidden(
            "API tokens can't be managed with an API token; log in to Gantry instead".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// Every user's tokens, for an Admin.
    #[serde(default)]
    all: bool,
}

/// `GET /api/tokens`: the caller's tokens, or with `?all=true` (Admins)
/// everyone's. Never the tokens themselves.
pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<TokenSummary>>, ApiError> {
    not_with_a_token(&user)?;
    if query.all && user.role < Role::Admin {
        return Err(ApiError::Forbidden(
            "only an Admin can see everyone's tokens".into(),
        ));
    }
    let owner = (!query.all).then_some(user.id);
    Ok(Json(state.store().api_tokens(owner).await?))
}

#[derive(Debug, Deserialize)]
pub struct NewTokenForm {
    name: String,
    /// Days until it expires; none or null for never.
    #[serde(default)]
    expires_in_days: Option<u32>,
}

/// A new token: shown this once.
#[derive(Debug, Serialize)]
pub struct Created {
    #[serde(flatten)]
    summary: TokenSummary,
    token: String,
}

/// `POST /api/tokens`.
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<NewTokenForm>,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    not_with_a_token(&user)?;
    let name = form.name.trim().to_owned();
    let result = async {
        if name.is_empty() || name.chars().count() > 64 {
            return Err(ApiError::field("name", "Must be 1 to 64 characters"));
        }
        let expires_at = match form.expires_in_days {
            None => None,
            Some(days @ 1..=MAX_DAYS) => {
                Some(crate::store::now() + time::Duration::days(i64::from(days)))
            }
            Some(_) => {
                return Err(ApiError::field(
                    "expires_in_days",
                    &format!("Must be 1 to {MAX_DAYS} days, or empty for never"),
                ));
            }
        };
        state
            .store()
            .create_api_token(&user, &name, expires_at)
            .await
            .map_err(|e| match e {
                CreateTokenError::Exists(_) => {
                    ApiError::field("name", "You already have a token with this name")
                }
                CreateTokenError::TooMany => ApiError::Conflict(e.to_string()),
                CreateTokenError::Database(e) => ApiError::internal(e),
            })
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::ApiTokenCreated)
            .target(name.clone())
            .detail(serde_json::json!({ "expires_in_days": form.expires_in_days }))
            .result(&result),
    )
    .await;
    let new = result?;
    Ok((
        StatusCode::CREATED,
        Json(Created {
            summary: new.summary,
            token: new.token,
        }),
    ))
}

/// `DELETE /api/tokens/{id}`: revokes a token. Its owner may, and so may an
/// Admin.
pub async fn revoke(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    not_with_a_token(&user)?;
    let token = state.store().api_token(id).await?;
    let result = async {
        let token = token
            .as_ref()
            .ok_or_else(|| ApiError::NotFound("no such token".into()))?;
        if token.user_id != user.id && user.role < Role::Admin {
            return Err(ApiError::NotFound("no such token".into()));
        }
        state.store().revoke_api_token(id).await?;
        Ok(StatusCode::NO_CONTENT)
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::ApiTokenRevoked)
            .target(
                token
                    .as_ref()
                    .map_or_else(|| id.to_string(), |t| t.name.clone()),
            )
            .detail(serde_json::json!({ "owner": token.as_ref().map(|t| &t.username) }))
            .result(&result),
    )
    .await;
    result
}
