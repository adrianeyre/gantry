//! A user's own account security (PRD §9.2): two-factor authentication with
//! TOTP and its recovery codes, and passkeys. None of it can be done with an
//! API token, and changing it asks for the password (or a code) again.
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::two_factor::{CodeCheck, TwoFactor};
use crate::store::users::User;

pub(crate) fn not_with_a_token(user: &User) -> Result<(), ApiError> {
    if user.api_token.is_some() {
        return Err(ApiError::Forbidden(
            "account security can't be changed with an API token; log in to Gantry instead".into(),
        ));
    }
    Ok(())
}

async fn check_password(state: &AppState, user: &User, password: &str) -> Result<(), ApiError> {
    match state.store().authenticate(&user.username, password).await? {
        Some(_) => Ok(()),
        None => Err(ApiError::field("password", "That isn't your password")),
    }
}

fn unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// The account page's view of the caller.
#[derive(Debug, Serialize)]
pub struct Account {
    #[serde(flatten)]
    user: User,
    two_factor: TwoFactor,
    /// An Admin requires TOTP of every user.
    require_totp: bool,
    passkeys: Vec<crate::store::passkeys::PasskeySummary>,
}

/// `GET /api/account`.
pub async fn get(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Account>, ApiError> {
    Ok(Json(Account {
        two_factor: state.store().two_factor(user.id).await?,
        require_totp: state.store().totp_required().await?,
        passkeys: state.store().passkeys(user.id).await?,
        user,
    }))
}

#[derive(Deserialize)]
pub struct PasswordForm {
    password: String,
}

/// A secret being set up: shown so the user can add it to their app.
#[derive(Serialize)]
pub struct Setup {
    secret: String,
    uri: String,
    qr_svg: String,
}

/// `POST /api/account/totp/setup`: a new secret, pending until a code from
/// it turns TOTP on. Asks for the password again.
pub async fn setup(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<PasswordForm>,
) -> Result<Json<Setup>, ApiError> {
    not_with_a_token(&user)?;
    check_password(&state, &user, &form.password).await?;
    if state.store().totp_enabled(user.id).await? {
        return Err(ApiError::Conflict(
            "two-factor authentication is already on; turn it off first to change apps".into(),
        ));
    }
    let secret = state.store().start_totp(state.key(), user.id).await?;
    let uri = crate::totp::uri(&secret, &user.username, "Gantry");
    Ok(Json(Setup {
        qr_svg: crate::totp::qr_svg(&uri),
        secret,
        uri,
    }))
}

#[derive(Deserialize)]
pub struct CodeForm {
    code: String,
}

#[derive(Serialize)]
pub struct RecoveryCodes {
    /// Shown this once: each works once, in place of a code.
    recovery_codes: Vec<String>,
}

/// `POST /api/account/totp/enable`: turns TOTP on with a code from the
/// pending secret, and gives recovery codes.
pub async fn enable(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<CodeForm>,
) -> Result<Json<RecoveryCodes>, ApiError> {
    not_with_a_token(&user)?;
    let result = async {
        state
            .store()
            .enable_totp(state.key(), user.id, &form.code, unix())
            .await?
            .ok_or_else(|| {
                ApiError::field(
                    "code",
                    "That code isn't right. Check your device's clock, and that you scanned the newest QR code",
                )
            })
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::TotpEnabled)
            .target(user.username.clone())
            .result(&result),
    )
    .await;
    Ok(Json(RecoveryCodes {
        recovery_codes: result?,
    }))
}

#[derive(Deserialize)]
pub struct DisableForm {
    password: String,
    code: String,
}

/// `POST /api/account/totp/disable`: needs the password and a current code
/// (or a recovery code). Refused while an Admin requires TOTP.
pub async fn disable(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<DisableForm>,
) -> Result<StatusCode, ApiError> {
    not_with_a_token(&user)?;
    let result = async {
        check_password(&state, &user, &form.password).await?;
        if state.store().totp_required().await? {
            return Err(ApiError::Conflict(
                "an Admin requires two-factor authentication, so it can't be turned off".into(),
            ));
        }
        match state
            .store()
            .check_second_factor(state.key(), user.id, &form.code, unix())
            .await?
        {
            CodeCheck::Wrong => return Err(ApiError::field("code", "That code isn't right")),
            CodeCheck::Totp | CodeCheck::RecoveryCode => {}
        }
        state.store().disable_totp(user.id).await?;
        Ok(StatusCode::NO_CONTENT)
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::TotpDisabled)
            .target(user.username.clone())
            .result(&result),
    )
    .await;
    result
}

/// `POST /api/account/totp/recovery-codes`: replaces the recovery codes,
/// given a current code.
pub async fn new_recovery_codes(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<CodeForm>,
) -> Result<Json<RecoveryCodes>, ApiError> {
    not_with_a_token(&user)?;
    if !state.store().totp_enabled(user.id).await? {
        return Err(ApiError::Conflict(
            "two-factor authentication isn't on".into(),
        ));
    }
    if state
        .store()
        .check_second_factor(state.key(), user.id, &form.code, unix())
        .await?
        != CodeCheck::Totp
    {
        return Err(ApiError::field(
            "code",
            "Enter a current code from your authenticator app",
        ));
    }
    Ok(Json(RecoveryCodes {
        recovery_codes: state.store().new_recovery_codes(user.id).await?,
    }))
}

/// `DELETE /api/users/{id}/totp`: an Admin turns off a user's TOTP, such as
/// after they lose their phone. If TOTP is required they must set it up
/// again at their next login.
pub async fn reset(
    State(state): State<AppState>,
    Extension(admin): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    not_with_a_token(&admin)?;
    let account = state
        .store()
        .user(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("no such user".into()))?;
    let result = async {
        if !state.store().disable_totp(id).await? {
            return Err(ApiError::Conflict(format!(
                "{} doesn't have two-factor authentication on",
                account.username
            )));
        }
        Ok(StatusCode::NO_CONTENT)
    }
    .await;
    audit::record(
        &state,
        audit::by(&admin, Action::TotpDisabled)
            .target(account.username.clone())
            .detail(serde_json::json!({ "by_admin": true }))
            .result(&result),
    )
    .await;
    result
}
