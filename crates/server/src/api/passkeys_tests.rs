//! Passkeys end to end (#68), with a virtual authenticator: registering,
//! logging in, and removal by the owner and by an Admin.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::store::users::Role;
use crate::test_support::{Client, state_with};
use crate::webauthn::virtual_authenticator::Authenticator;
use crate::webauthn::{EDDSA, ES256, RS256};

const ORIGIN: &str = "http://gantry.test:8080";

async fn client() -> Client {
    let state = state_with(crate::Options {
        public_url: Some(ORIGIN.into()),
        ..Default::default()
    })
    .await;
    Client::with_state(state).await
}

/// A POST with no session, returning the cookie it sets, if any.
async fn open_post(
    client: &Client,
    path: &str,
    body: Value,
) -> (StatusCode, Value, Option<String>) {
    let response = crate::app(client.state.clone())
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::to_owned);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cookie,
    )
}

async fn register(client: &Client, device: &mut Authenticator, name: &str) -> Value {
    let (status, options) = client
        .post("/api/account/passkeys/register/start", json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let credential = device.register(&options, ORIGIN);
    let (status, added) = client
        .post(
            "/api/account/passkeys/register/finish",
            json!({ "name": name, "credential": credential }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    added
}

async fn log_in(
    client: &Client,
    device: &mut Authenticator,
) -> (StatusCode, Value, Option<String>) {
    let (status, options, _) = open_post(client, "/api/login/passkey/start", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let credential = device.login(&options, ORIGIN);
    open_post(
        client,
        "/api/login/passkey/finish",
        json!({ "credential": credential }),
    )
    .await
}

#[tokio::test]
async fn a_registered_passkey_logs_in_without_a_password() {
    for algorithm in [ES256, EDDSA, RS256] {
        let admin = client().await;
        let operator = admin.as_role(Role::Operator).await;
        let mut device = Authenticator::new(algorithm);
        let (_, options) = operator
            .post("/api/account/passkeys/register/start", json!({}))
            .await;
        let public_key = &options["publicKey"];
        assert_eq!(public_key["rp"]["id"], "gantry.test");
        assert_eq!(public_key["user"]["name"], "operator");
        assert_eq!(public_key["attestation"], "none");
        assert_eq!(
            public_key["authenticatorSelection"]["residentKey"],
            "required"
        );
        let credential = device.register(&options, ORIGIN);
        let (status, added) = operator
            .post(
                "/api/account/passkeys/register/finish",
                json!({ "name": "Phone", "credential": credential }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{added}");

        let (status, user, cookie) = log_in(&admin, &mut device).await;
        assert_eq!(status, StatusCode::OK, "{algorithm}: {user}");
        assert_eq!(user["username"], "operator");
        assert_eq!(user["role"], "operator");
        let session = Client::with_cookie(&admin.state, &cookie.unwrap());
        let (status, me) = session.get("/api/session").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["username"], "operator");

        let (_, account) = operator.get("/api/account").await;
        assert_eq!(account["passkeys"][0]["name"], "Phone");
        assert!(account["passkeys"][0]["last_used_at"].is_string());
        let (_, audit) = admin.get("/api/audit").await;
        let entries = audit["entries"].as_array().unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e["action"] == "passkey.added" && e["username"] == "operator")
        );
        assert!(
            entries
                .iter()
                .any(|e| e["action"] == "login.succeeded" && e["detail"]["method"] == "passkey")
        );
    }
}

#[tokio::test]
async fn a_bad_passkey_answer_is_refused_and_audited() {
    let admin = client().await;
    let mut device = Authenticator::new(ES256);
    register(&admin, &mut device, "Key").await;

    // An authenticator Gantry has never seen.
    let mut stranger = Authenticator::new(ES256);
    let (status, body, cookie) = log_in(&admin, &mut stranger).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(cookie.is_none());

    // A copied authenticator: its counter goes backwards.
    let (status, _, _) = log_in(&admin, &mut device).await;
    assert_eq!(status, StatusCode::OK);
    let (_, options, _) = open_post(&admin, "/api/login/passkey/start", json!({})).await;
    let replayed = device.sign_login(&options, ORIGIN, 1);
    let (status, body, _) = open_post(
        &admin,
        "/api/login/passkey/finish",
        json!({ "credential": replayed }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"].as_str().unwrap().contains("copied"), "{body}");

    // The same signed answer twice: its challenge is used up.
    let (_, options, _) = open_post(&admin, "/api/login/passkey/start", json!({})).await;
    let answer = device.login(&options, ORIGIN);
    let (status, _, _) = open_post(
        &admin,
        "/api/login/passkey/finish",
        json!({ "credential": answer.clone() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = open_post(
        &admin,
        "/api/login/passkey/finish",
        json!({ "credential": answer }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Made on another site.
    let (_, options, _) = open_post(&admin, "/api/login/passkey/start", json!({})).await;
    let phished = device.login(&options, "http://gantry.evil:8080");
    let (status, _, _) = open_post(
        &admin,
        "/api/login/passkey/finish",
        json!({ "credential": phished }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (_, audit) = admin.get("/api/audit").await;
    let failed = audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] == "login.failed" && e["detail"]["method"] == "passkey")
        .count();
    assert_eq!(failed, 4);
}

#[tokio::test]
async fn a_locked_account_stays_locked_with_a_passkey() {
    let admin = client().await;
    let mut device = Authenticator::new(ES256);
    register(&admin, &mut device, "Key").await;
    sqlx::query("UPDATE users SET locked_until = '2999-01-01T00:00:00Z' WHERE username = 'admin'")
        .execute(admin.state.store().pool())
        .await
        .unwrap();
    let (status, body, cookie) = log_in(&admin, &mut device).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"].as_str().unwrap().contains("locked"));
    assert!(cookie.is_none());
}

#[tokio::test]
async fn passkeys_are_removed_by_their_owner_or_an_admin() {
    let admin = client().await;
    let viewer = admin.as_role(Role::Viewer).await;
    let operator = admin.as_role(Role::Operator).await;
    let mut phone = Authenticator::new(ES256);
    let mut key = Authenticator::new(EDDSA);
    let first = register(&viewer, &mut phone, "Phone").await;
    let second = register(&viewer, &mut key, "Security key").await;

    // Registering the same authenticator again is refused.
    let (_, options) = viewer
        .post("/api/account/passkeys/register/start", json!({}))
        .await;
    assert_eq!(
        options["publicKey"]["excludeCredentials"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Someone else can't remove it through their own account.
    let (status, _) = operator
        .delete(&format!("/api/account/passkeys/{}", first["id"]))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The owner can.
    let (status, _) = viewer
        .delete(&format!("/api/account/passkeys/{}", first["id"]))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = log_in(&admin, &mut phone).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a removed passkey no longer logs in"
    );

    // An Admin sees and removes another user's.
    let (_, users) = admin.get("/api/users").await;
    let id = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "viewer")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let (status, _) = operator.get(&format!("/api/users/{id}/passkeys")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, listed) = admin.get(&format!("/api/users/{id}/passkeys")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed[0]["name"], "Security key");
    let (status, _) = operator
        .delete(&format!("/api/users/{id}/passkeys/{}", second["id"]))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = admin
        .delete(&format!("/api/users/{id}/passkeys/{}", second["id"]))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, account) = viewer.get("/api/account").await;
    assert_eq!(account["passkeys"], json!([]));

    let (_, audit) = admin.get("/api/audit").await;
    let removals: Vec<&Value> = audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] == "passkey.removed" && e["result"] == "ok")
        .collect();
    assert_eq!(removals.len(), 2);
    assert!(removals.iter().any(|e| e["username"] == "admin"
        && e["target"] == "viewer"
        && e["detail"]["by_admin"] == true));
}
