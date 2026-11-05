//! User management, for Admins (PRD §7.6, §9.2). The first account is still
//! created from the command line; after that, Admins manage accounts here.
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::users::{Account, ChangeUserError, CreateUserError, Role, User};

/// The body of a new account.
#[derive(Deserialize)]
pub struct NewUser {
    username: String,
    password: String,
    role: Role,
}

impl std::fmt::Debug for NewUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewUser")
            .field("username", &self.username)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

/// A change to an account: a new role, a new password, or both.
#[derive(Deserialize)]
pub struct UserChange {
    #[serde(default)]
    role: Option<Role>,
    #[serde(default)]
    password: Option<String>,
}

pub(crate) fn change_error(e: ChangeUserError) -> ApiError {
    match e {
        ChangeUserError::NotFound => ApiError::NotFound("no such user".into()),
        ChangeUserError::LastAdmin => ApiError::Conflict(e.to_string()),
        ChangeUserError::WeakPassword => ApiError::field("password", &e.to_string()),
        ChangeUserError::Database(e) => ApiError::internal(e),
    }
}

/// `GET /api/users`.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Account>>, ApiError> {
    Ok(Json(state.store().users().await?))
}

/// `POST /api/users`.
pub async fn create(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Json(form): Json<NewUser>,
) -> Result<Response, ApiError> {
    let result = async {
        let user = state
            .store()
            .create_user(&form.username, &form.password, form.role)
            .await
            .map_err(|e| match e {
                CreateUserError::Exists(_) => {
                    ApiError::field("username", "Another user already has this name")
                }
                CreateUserError::InvalidUsername => ApiError::field("username", &e.to_string()),
                CreateUserError::WeakPassword => ApiError::field("password", &e.to_string()),
                CreateUserError::Database(e) => ApiError::internal(e),
            })?;
        state
            .store()
            .user(user.id)
            .await?
            .ok_or_else(|| ApiError::internal("the new user vanished"))
    }
    .await;
    audit::record(
        &state,
        audit::by(&admin, Action::UserCreated)
            .target(form.username.trim())
            .detail(serde_json::json!({ "role": form.role }))
            .result(&result),
    )
    .await;
    let account = result?;
    tracing::info!(username = %account.username, role = account.role.as_str(), "created a user");
    Ok((StatusCode::CREATED, Json(account)).into_response())
}

/// `POST /api/users/{id}/unlock`: ends a lockout early (PRD §9.2).
pub async fn unlock(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(id): Path<i64>,
) -> Result<Json<Account>, ApiError> {
    let name = username(&state, id).await;
    let result = async {
        if !state.store().unlock(id).await? {
            return Err(ApiError::NotFound("no such user".into()));
        }
        state
            .store()
            .user(id)
            .await?
            .ok_or_else(|| ApiError::NotFound("no such user".into()))
    }
    .await;
    audit::record(
        &state,
        audit::by(&admin, Action::AccountUnlocked)
            .target(name)
            .result(&result),
    )
    .await;
    let account = result?;
    tracing::info!(username = %account.username, "unlocked an account");
    Ok(Json(account))
}

/// The name an account is known by in the log, even after it is gone.
async fn username(state: &AppState, id: i64) -> String {
    match state.store().user(id).await {
        Ok(Some(account)) => account.username,
        _ => format!("user #{id}"),
    }
}

/// `PUT /api/users/{id}`.
pub async fn update(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(id): Path<i64>,
    Json(change): Json<UserChange>,
) -> Result<Json<Account>, ApiError> {
    let name = username(&state, id).await;
    let result = apply(&state, id, &change).await;
    audit::record(
        &state,
        audit::by(&admin, Action::UserUpdated)
            .target(name)
            .detail(serde_json::json!({
                "role": change.role,
                "password_changed": change.password.is_some(),
            }))
            .result(&result),
    )
    .await;
    let account = result?;
    tracing::info!(username = %account.username, role = account.role.as_str(), "changed a user");
    Ok(Json(account))
}

async fn apply(state: &AppState, id: i64, change: &UserChange) -> Result<Account, ApiError> {
    if let Some(password) = &change.password {
        state
            .store()
            .set_password(id, password)
            .await
            .map_err(change_error)?;
    }
    if let Some(role) = change.role {
        state
            .store()
            .set_role(id, role)
            .await
            .map_err(change_error)?;
    }
    state
        .store()
        .user(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("no such user".into()))
}

/// `DELETE /api/users/{id}`.
pub async fn remove(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let name = username(&state, id).await;
    let result = state.store().delete_user(id).await.map_err(change_error);
    audit::record(
        &state,
        audit::by(&admin, Action::UserRemoved)
            .target(name)
            .result(&result),
    )
    .await;
    let account = result?;
    tracing::info!(username = %account.username, "removed a user");
    Ok(StatusCode::NO_CONTENT)
}
