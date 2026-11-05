//! Passkeys (PRD §9.2): logging in with WebAuthn as well as a password.
//!
//! A logged-in user registers passkeys under Account, and removes them; an
//! Admin can remove anyone's. On the login page, "Log in with a passkey"
//! asks the browser for any passkey for this site (a discoverable
//! credential, so no username is typed) and Gantry checks it. A passkey is
//! itself two factors (the device and its lock), so it isn't also asked
//! for a TOTP code. A locked account stays locked whichever way it logs in.
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::json;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::client::ClientInfo;
use crate::store::audit::{Action, Entry, Outcome};
use crate::store::users::User;
use crate::webauthn::{
    AssertionAnswer, EDDSA, ES256, Purpose, RS256, RegistrationResponse, RelyingParty,
};

/// How long the browser gives the user, in milliseconds.
const TIMEOUT_MS: u64 = 120_000;

/// The site passkeys are made for: the configured public URL, or else the
/// address this request came to.
pub fn relying_party(
    state: &AppState,
    headers: &HeaderMap,
    client: Option<&ClientInfo>,
) -> Result<RelyingParty, ApiError> {
    if let Some(url) = state.public_url() {
        return RelyingParty::from_origin(url)
            .ok_or_else(|| ApiError::Conflict("public_url isn't a URL".into()));
    }
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| ApiError::Conflict("the request has no Host".into()))?;
    let scheme = if client.is_some_and(|c| c.https) || state.secure_cookies() {
        "https"
    } else {
        "http"
    };
    RelyingParty::from_origin(&format!("{scheme}://{host}"))
        .ok_or_else(|| ApiError::Conflict("the request's Host isn't an address".into()))
}

/// `POST /api/account/passkeys/register/start`: the options for
/// `navigator.credentials.create()`.
pub async fn register_start(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    info: Option<Extension<ClientInfo>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    super::account::not_with_a_token(&user)?;
    let rp = relying_party(&state, &headers, info.as_ref().map(|i| &i.0))?;
    let handle = state.store().webauthn_handle(user.id).await?;
    let exclude: Vec<_> = state
        .store()
        .passkey_credential_ids(user.id)
        .await?
        .into_iter()
        .map(|id| json!({ "type": "public-key", "id": id }))
        .collect();
    let challenge = state
        .challenges()
        .issue(Purpose::Register { user_id: user.id }, rp.clone());
    let params: Vec<_> = [ES256, EDDSA, RS256]
        .iter()
        .map(|alg| json!({ "type": "public-key", "alg": alg }))
        .collect();
    Ok(Json(json!({ "publicKey": {
        "challenge": challenge,
        "rp": { "id": rp.id, "name": "Gantry" },
        "user": { "id": handle, "name": user.username, "displayName": user.username },
        "pubKeyCredParams": params,
        "timeout": TIMEOUT_MS,
        "attestation": "none",
        "excludeCredentials": exclude,
        "authenticatorSelection": { "residentKey": "required", "requireResidentKey": true, "userVerification": "preferred" },
    } })))
}

#[derive(Debug, Deserialize)]
pub struct RegisterForm {
    /// What the user calls it, such as "Phone".
    name: String,
    credential: RegistrationResponse,
}

/// `POST /api/account/passkeys/register/finish`.
pub async fn register_finish(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<RegisterForm>,
) -> Result<(StatusCode, Json<crate::store::passkeys::PasskeySummary>), ApiError> {
    super::account::not_with_a_token(&user)?;
    let name = form.name.trim().to_owned();
    let result = async {
        if name.is_empty() || name.chars().count() > 64 {
            return Err(ApiError::field("name", "Must be 1 to 64 characters"));
        }
        let credential =
            crate::webauthn::finish_registration(state.challenges(), user.id, &form.credential)
                .map_err(|e| ApiError::Conflict(e.0))?;
        state
            .store()
            .add_passkey(
                user.id,
                &name,
                &credential.id,
                &credential.public_key,
                credential.algorithm,
                credential.sign_count,
            )
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() => {
                    ApiError::Conflict("this passkey is already registered".into())
                }
                _ => ApiError::internal(e),
            })
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::PasskeyAdded)
            .target(user.username.clone())
            .detail(json!({ "name": name }))
            .result(&result),
    )
    .await;
    Ok((StatusCode::CREATED, Json(result?)))
}

async fn remove_for(
    state: &AppState,
    by: &User,
    owner_id: i64,
    owner_name: &str,
    id: i64,
) -> Result<StatusCode, ApiError> {
    let result = state
        .store()
        .remove_passkey(owner_id, id)
        .await
        .map_err(ApiError::from)
        .and_then(|name| name.ok_or_else(|| ApiError::NotFound("no such passkey".into())));
    audit::record(
        state,
        audit::by(by, Action::PasskeyRemoved)
            .target(owner_name.to_owned())
            .detail(json!({ "name": result.as_ref().ok(), "by_admin": by.id != owner_id }))
            .result(&result),
    )
    .await;
    result.map(|_| StatusCode::NO_CONTENT)
}

/// `DELETE /api/account/passkeys/{id}`: the owner removes one.
pub async fn remove_own(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    super::account::not_with_a_token(&user)?;
    remove_for(&state, &user, user.id, &user.username, id).await
}

/// `GET /api/users/{id}/passkeys`: an Admin sees a user's passkeys.
pub async fn list_for_user(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<crate::store::passkeys::PasskeySummary>>, ApiError> {
    state
        .store()
        .user(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("no such user".into()))?;
    Ok(Json(state.store().passkeys(id).await?))
}

/// `DELETE /api/users/{id}/passkeys/{passkey}`: an Admin removes one of a
/// user's passkeys.
pub async fn remove_for_user(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path((user_id, id)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    super::account::not_with_a_token(&admin)?;
    let account = state
        .store()
        .user(user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("no such user".into()))?;
    remove_for(&state, &admin, user_id, &account.username, id).await
}

/// `POST /api/login/passkey/start`: open to anyone, like the login form.
pub async fn login_start(
    State(state): State<AppState>,
    info: Option<Extension<ClientInfo>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rp = relying_party(&state, &headers, info.as_ref().map(|i| &i.0))?;
    let challenge = state.challenges().issue(Purpose::Login, rp.clone());
    Ok(Json(json!({ "publicKey": {
        "challenge": challenge,
        "rpId": rp.id,
        "timeout": TIMEOUT_MS,
        "userVerification": "preferred",
        "allowCredentials": [],
    } })))
}

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    credential: AssertionAnswer,
}

/// `POST /api/login/passkey/finish`: checks the passkey and starts a
/// session, as `POST /api/login` does.
pub async fn login_finish(
    State(state): State<AppState>,
    info: Option<Extension<ClientInfo>>,
    Json(form): Json<LoginForm>,
) -> Response {
    let info = info.map(|i| i.0);
    let client = info
        .as_ref()
        .map_or_else(|| "unknown".to_owned(), ClientInfo::label);
    let refuse = |message: &str| {
        (StatusCode::UNAUTHORIZED, Json(json!({ "error": message }))).into_response()
    };
    if state
        .clients()
        .check(
            &client,
            state.login_limits().client_attempts_per_minute,
            std::time::Instant::now(),
        )
        .is_err()
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "Too many failed logins from here. Wait a minute and try again." })),
        )
            .into_response();
    }
    let found = match state
        .store()
        .passkey_by_credential(form.credential.id.trim_end_matches('='))
        .await
    {
        Ok(found) => found,
        Err(e) => {
            tracing::error!(error = %e, "could not look up a passkey");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let failed = |user: Option<&User>, message: &str| {
        let mut entry = match user {
            Some(user) => crate::audit::by(user, Action::LoginFailed),
            None => Entry::new(Action::LoginFailed),
        };
        entry.outcome = Outcome::Refused;
        entry.message = Some(message.to_owned());
        entry.detail = Some(json!({ "client": client, "method": "passkey" }));
        entry
    };
    let Some((passkey, user)) = found else {
        state.clients().failed(&client, std::time::Instant::now());
        crate::audit::record(&state, failed(None, "an unknown passkey")).await;
        return refuse(
            "Gantry doesn't know this passkey. Log in with your password and add it under Account.",
        );
    };
    match crate::webauthn::finish_login(
        state.challenges(),
        &form.credential,
        &passkey.public_key,
        passkey.sign_count,
    ) {
        Ok(count) => {
            if let Err(e) = state.store().passkey_used(passkey.id, count).await {
                tracing::error!(error = %e, "could not record a passkey's use");
            }
        }
        Err(e) => {
            state.clients().failed(&client, std::time::Instant::now());
            crate::audit::record(&state, failed(Some(&user), &e.0)).await;
            return refuse(&format!("The passkey wasn't accepted: {}.", e.0));
        }
    }
    match state.store().locked_until(user.id).await {
        Ok(Some(_)) => {
            crate::audit::record(&state, failed(Some(&user), "account locked")).await;
            return refuse(
                "This account is locked after too many failed logins. Try again later, or ask an Admin to unlock it.",
            );
        }
        Ok(None) => {}
        Err(e) => {
            tracing::error!(error = %e, "could not check a lockout");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    crate::auth::start_session(
        &state,
        info.as_ref(),
        user,
        json!({ "client": client, "method": "passkey" }),
    )
    .await
}
