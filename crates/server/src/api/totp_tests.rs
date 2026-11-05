//! TOTP two-factor authentication end to end (#67): enrolment, logging in
//! with a code or a recovery code, encryption at rest, auditing, and Admins
//! requiring it.
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::store::users::Role;
use crate::test_support::{Client, PASSWORD};
use crate::totp;

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// The code for `secret` `offset` steps from now.
fn code(secret: &str, offset: i64) -> String {
    let key = totp::from_base32(secret).unwrap();
    totp::code_at_step(&key, totp::step_at(now()) + offset, totp::DIGITS)
}

/// As if time had moved on: lets the next step's code be used again.
async fn forget_last_step(client: &Client, username: &str) {
    sqlx::query("UPDATE users SET totp_last_step = NULL WHERE username = ?")
        .bind(username)
        .execute(client.state.store().pool())
        .await
        .unwrap();
}

/// Turns TOTP on for the client's user; returns the secret and recovery codes.
async fn enrol(client: &Client) -> (String, Vec<String>) {
    let (status, setup) = client
        .post("/api/account/totp/setup", json!({ "password": PASSWORD }))
        .await;
    assert_eq!(status, StatusCode::OK, "{setup}");
    let secret = setup["secret"].as_str().unwrap().to_owned();
    let (status, enabled) = client
        .post(
            "/api/account/totp/enable",
            json!({ "code": code(&secret, 0) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{enabled}");
    let codes = enabled["recovery_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_owned())
        .collect();
    (secret, codes)
}

fn login(username: &str, code: Option<&str>) -> Value {
    match code {
        Some(code) => json!({ "username": username, "password": PASSWORD, "code": code }),
        None => json!({ "username": username, "password": PASSWORD }),
    }
}

async fn actions(admin: &Client) -> Vec<(String, Option<String>, Option<String>)> {
    let (_, audit) = admin.get("/api/audit").await;
    audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["action"].as_str().unwrap().to_owned(),
                e["target"].as_str().map(str::to_owned),
                e["message"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

#[tokio::test]
async fn enrolment_shows_a_qr_code_needs_the_password_and_a_working_code() {
    let admin = Client::new().await;
    let (status, error) = admin
        .post("/api/account/totp/setup", json!({ "password": "not it" }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    let (status, setup) = admin
        .post("/api/account/totp/setup", json!({ "password": PASSWORD }))
        .await;
    assert_eq!(status, StatusCode::OK);
    let secret = setup["secret"].as_str().unwrap();
    assert_eq!(secret.len(), 32);
    assert!(setup["uri"].as_str().unwrap().starts_with(&format!(
        "otpauth://totp/Gantry:admin?secret={secret}&issuer=Gantry"
    )));
    assert!(setup["qr_svg"].as_str().unwrap().starts_with("<?xml"));
    // Not on until a code proves the app has it.
    let (_, account) = admin.get("/api/account").await;
    assert_eq!(account["two_factor"]["totp_enabled"], false);
    let (status, _) = admin
        .post("/api/account/totp/enable", json!({ "code": "000000" }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, enabled) = admin
        .post(
            "/api/account/totp/enable",
            json!({ "code": code(secret, 0) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{enabled}");
    assert_eq!(enabled["recovery_codes"].as_array().unwrap().len(), 10);
    let (_, account) = admin.get("/api/account").await;
    assert_eq!(account["two_factor"]["totp_enabled"], true);
    assert_eq!(account["two_factor"]["recovery_codes_left"], 10);
    // Setting it up again is refused while it is on.
    let (status, _) = admin
        .post("/api/account/totp/setup", json!({ "password": PASSWORD }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_secret_is_encrypted_at_rest_and_recovery_codes_are_hashed() {
    let admin = Client::new().await;
    let (secret, codes) = enrol(&admin).await;
    let (stored, pending): (String, Option<String>) =
        sqlx::query_as("SELECT totp_secret, totp_pending FROM users WHERE username = 'admin'")
            .fetch_one(admin.state.store().pool())
            .await
            .unwrap();
    assert!(stored.starts_with("v1:"), "{stored}");
    assert!(!stored.contains(&secret));
    assert!(pending.is_none());
    assert_eq!(
        admin.state.key().decrypt(&stored).unwrap(),
        secret,
        "decrypts with the secret key"
    );
    let hashes: Vec<String> = sqlx::query_scalar("SELECT code_hash FROM recovery_codes")
        .fetch_all(admin.state.store().pool())
        .await
        .unwrap();
    assert_eq!(hashes.len(), 10);
    for code in &codes {
        assert!(!hashes.contains(code));
        assert!(!hashes.iter().any(|h| h.contains(&code.replace('-', ""))));
    }
}

#[tokio::test]
async fn logging_in_needs_a_code_and_a_wrong_one_counts_as_a_failure() {
    let admin = Client::new().await;
    let (secret, _) = enrol(&admin).await;
    let state = admin.state.clone();

    // The password alone: asked for the code, no session, nothing counted.
    let (status, body, cookie) = Client::login_with(&state, login("admin", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["second_factor"], "totp");
    assert!(cookie.is_none());
    let failures = |state: crate::AppState| async move {
        sqlx::query_scalar::<_, i64>("SELECT failed_logins FROM users WHERE username = 'admin'")
            .fetch_one(state.store().pool())
            .await
            .unwrap()
    };
    assert_eq!(failures(state.clone()).await, 0);

    // A wrong code is a failed login.
    let (status, body, cookie) = Client::login_with(&state, login("admin", Some("000000"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"].as_str().unwrap().contains("code"));
    assert!(cookie.is_none());
    assert_eq!(failures(state.clone()).await, 1);

    // The wrong password with a right code is still the wrong password.
    let (status, _, _) = Client::login_with(
        &state,
        json!({ "username": "admin", "password": "nope", "code": code(&secret, 1) }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Waits after failures are part of the lockout rules; skip them here.
    sqlx::query("UPDATE users SET failed_logins = 0, last_failure = NULL")
        .execute(state.store().pool())
        .await
        .unwrap();
    let next = code(&secret, 1);
    let (status, user, cookie) = Client::login_with(&state, login("admin", Some(&next))).await;
    assert_eq!(status, StatusCode::OK, "{user}");
    let session = Client::with_cookie(&state, &cookie.unwrap());
    let (status, _) = session.get("/api/printers").await;
    assert_eq!(status, StatusCode::OK);
    // The same code again: refused, it has been used.
    let (status, _, _) = Client::login_with(&state, login("admin", Some(&next))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let audit = actions(&admin).await;
    assert!(
        audit
            .iter()
            .any(|(a, _, m)| a == "login.failed" && m.as_deref() == Some("wrong two-factor code"))
    );
    assert!(audit.iter().any(|(a, _, _)| a == "login.succeeded"));
}

#[tokio::test]
async fn wrong_codes_lock_the_account_like_wrong_passwords() {
    let state = crate::test_support::state_with(crate::Options {
        login: crate::store::users::LoginLimits {
            max_failures: 3,
            slow_after: 10,
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let admin = Client::with_state(state.clone()).await;
    let (secret, _) = enrol(&admin).await;
    for _ in 0..3 {
        let (status, _, _) = Client::login_with(&state, login("admin", Some("111111"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, body, _) =
        Client::login_with(&state, login("admin", Some(&code(&secret, 1)))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(
        actions(&admin)
            .await
            .iter()
            .any(|(a, t, _)| a == "account.locked" && t.as_deref() == Some("admin"))
    );
}

#[tokio::test]
async fn a_recovery_code_works_once_and_is_audited() {
    let admin = Client::new().await;
    let (_, codes) = enrol(&admin).await;
    let state = admin.state.clone();
    let typed = codes[0].to_uppercase().replace('-', " ");
    let (status, user, cookie) = Client::login_with(&state, login("admin", Some(&typed))).await;
    assert_eq!(status, StatusCode::OK, "{user}");
    assert!(cookie.is_some());
    let (status, _, _) = Client::login_with(&state, login("admin", Some(&codes[0]))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "used up");
    let (_, account) = admin.get("/api/account").await;
    assert_eq!(account["two_factor"]["recovery_codes_left"], 9);
    assert!(
        actions(&admin)
            .await
            .iter()
            .any(|(a, _, _)| a == "totp.recovery_code_used")
    );
}

#[tokio::test]
async fn turning_it_on_and_off_is_audited_and_off_needs_the_password_and_a_code() {
    let admin = Client::new().await;
    let (secret, _) = enrol(&admin).await;
    let (status, _) = admin
        .post(
            "/api/account/totp/disable",
            json!({ "password": PASSWORD, "code": "000000" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = admin
        .post(
            "/api/account/totp/disable",
            json!({ "password": "nope", "code": code(&secret, 1) }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    forget_last_step(&admin, "admin").await;
    let (status, body) = admin
        .post(
            "/api/account/totp/disable",
            json!({ "password": PASSWORD, "code": code(&secret, 0) }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, _, cookie) = Client::login_with(&admin.state, login("admin", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cookie.is_some());

    let audit = actions(&admin).await;
    let mut kinds: Vec<(&str, &str)> = audit
        .iter()
        .filter(|(a, _, _)| a.starts_with("totp."))
        .map(|(a, _, m)| (a.as_str(), if m.is_some() { "refused" } else { "ok" }))
        .collect();
    kinds.reverse();
    assert_eq!(
        kinds,
        [
            ("totp.enabled", "ok"),
            ("totp.disabled", "refused"),
            ("totp.disabled", "refused"),
            ("totp.disabled", "ok"),
        ]
    );
}

#[tokio::test]
async fn admins_can_require_it_and_reset_it() {
    let admin = Client::new().await;
    enrol(&admin).await;
    let operator = admin.as_role(Role::Operator).await;
    let (status, settings) = admin
        .put("/api/settings", json!({ "require_totp": true }))
        .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["require_totp"], true);

    // The Operator, without TOTP, may now only see their session and enrol.
    let (status, body) = operator.get("/api/printers").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["enrol_totp"], true);
    let (status, session) = operator.get("/api/session").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["must_enrol_totp"], true);
    // A fresh login says so too.
    let (status, user, _) = Client::login_with(&admin.state, login("operator", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(user["must_enrol_totp"], true);

    let (secret, _) = enrol(&operator).await;
    let (status, _) = operator.get("/api/printers").await;
    assert_eq!(status, StatusCode::OK);
    // And it can't be turned off while required.
    let (status, _) = operator
        .post(
            "/api/account/totp/disable",
            json!({ "password": PASSWORD, "code": code(&secret, 1) }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // An Admin resets it (a lost phone): the Operator must enrol again.
    let (_, users) = admin.get("/api/users").await;
    let id = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "operator")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let (status, _) = operator.delete(&format!("/api/users/{id}/totp")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "only an Admin");
    let (status, _) = admin.delete(&format!("/api/users/{id}/totp")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = operator.get("/api/printers").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        actions(&admin)
            .await
            .iter()
            .any(|(a, t, _)| a == "totp.disabled" && t.as_deref() == Some("operator"))
    );
}

#[tokio::test]
async fn an_api_token_cant_change_two_factor_settings() {
    let admin = Client::new().await;
    let (_, token) = admin.post("/api/tokens", json!({ "name": "t" })).await;
    let api = crate::test_support::with_token(&admin.state, token["token"].as_str().unwrap());
    let (status, _) = api
        .post("/api/account/totp/setup", json!({ "password": PASSWORD }))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
