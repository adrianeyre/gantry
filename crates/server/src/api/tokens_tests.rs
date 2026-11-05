//! API tokens end to end (#70): made and revoked by their owner, stored
//! hashed, acting with the owner's role, and audited like UI actions.
use axum::http::StatusCode;
use serde_json::json;

use crate::store::users::Role;
use crate::test_support::{Client, anonymous, with_token};

async fn token(client: &Client, name: &str) -> (i64, String) {
    let (status, body) = client.post("/api/tokens", json!({ "name": name })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (
        body["id"].as_i64().unwrap(),
        body["token"].as_str().unwrap().to_owned(),
    )
}

#[tokio::test]
async fn a_token_acts_with_its_owners_role_and_can_be_revoked() {
    let admin = Client::new().await;
    let operator = admin.as_role(Role::Operator).await;
    let printer = admin.connected_simulated("X1", json!({})).await;
    let (id, secret) = token(&operator, "home assistant").await;
    let api = with_token(&admin.state, &secret);

    // Reads and Operator actions work; Admin actions don't.
    let (status, printers) = api.get("/api/printers").await;
    assert_eq!(status, StatusCode::OK, "{printers}");
    assert_eq!(printers.as_array().unwrap().len(), 1);
    let (status, body) = api
        .post(
            &format!("/api/printers/{printer}/simulation"),
            json!({ "action": "start_print", "print_file": "cube.gcode", "duration_secs": 600, "layers": 10 }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "simulation is Admin-only: {body}"
    );
    let (status, body) = api.post("/api/tags", json!({ "name": "Room" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, session) = api.get("/api/session").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["username"], "operator");
    assert_eq!(session["role"], "operator");
    assert_eq!(session["api_token"], "home assistant");

    // Its owner's role changes, and so does the token's.
    let (_, users) = admin.get("/api/users").await;
    let operator_id = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "operator")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let (status, _) = admin
        .put(
            &format!("/api/users/{operator_id}"),
            json!({ "role": "admin" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = api.post("/api/tags", json!({ "name": "Room" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Listed without the token itself.
    let (_, listed) = operator.get("/api/tokens").await;
    assert_eq!(listed[0]["name"], "home assistant");
    assert!(listed[0]["last_used_at"].is_string());
    assert!(!listed.to_string().contains(&secret[7..]));

    // A token can't manage tokens.
    let (status, _) = api.post("/api/tokens", json!({ "name": "another" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = api.delete(&format!("/api/tokens/{id}")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = operator.delete(&format!("/api/tokens/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = api.get("/api/printers").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body["error"].as_str().unwrap().contains("revoked"));
}

#[tokio::test]
async fn tokens_are_stored_hashed() {
    let admin = Client::new().await;
    let (_, secret) = token(&admin, "ci").await;
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT name, token_hash, prefix FROM api_tokens")
            .fetch_all(admin.state.store().pool())
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    let (_, hash, prefix) = &rows[0];
    assert!(!hash.contains(&secret[7..]) && hash != &secret);
    assert!(secret.starts_with(prefix.as_str()) && prefix.len() < 16);
}

#[tokio::test]
async fn token_calls_are_audited_with_the_tokens_name() {
    let admin = Client::new().await;
    let printer = admin.connected_simulated("X1", json!({})).await;
    let (_, secret) = token(&admin, "octoslack").await;
    let api = with_token(&admin.state, &secret);
    let (status, body) = api
        .put(
            &format!("/api/printers/{printer}/automation"),
            json!({ "auto_start": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // A refused call is audited too.
    let (status, _) = api
        .post(
            &format!("/api/printers/{printer}/commands"),
            json!({ "command": "pause" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (_, entries) = admin.get("/api/audit").await;
    let entries = entries["entries"].as_array().unwrap();
    let automation = entries
        .iter()
        .find(|e| e["action"] == "printer.automation")
        .unwrap();
    assert_eq!(automation["username"], "admin");
    assert_eq!(automation["api_token"], "octoslack");
    let pause = entries
        .iter()
        .find(|e| e["action"] == "printer.command")
        .unwrap();
    assert_eq!(pause["api_token"], "octoslack");
    assert_eq!(pause["result"], "refused");
    let created = entries
        .iter()
        .find(|e| e["action"] == "api_token.created")
        .unwrap();
    assert_eq!(created["target"], "octoslack");
    assert!(
        created["api_token"].is_null(),
        "made in the UI, not with a token"
    );
    // Made in the UI: nothing says a token did it.
    let login = entries
        .iter()
        .find(|e| e["action"] == "login.succeeded")
        .unwrap();
    assert!(login["api_token"].is_null());
}

#[tokio::test]
async fn only_the_owner_or_an_admin_revokes_and_bad_input_is_refused() {
    let admin = Client::new().await;
    let viewer = admin.as_role(Role::Viewer).await;
    let operator = admin.as_role(Role::Operator).await;
    let (id, _) = token(&viewer, "mine").await;
    let (status, _) = operator.delete(&format!("/api/tokens/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = viewer.get("/api/tokens?all=true").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, all) = admin.get("/api/tokens?all=true").await;
    assert_eq!(all[0]["username"], "viewer");
    let (status, _) = admin.delete(&format!("/api/tokens/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    for (body, field) in [
        (json!({ "name": " " }), "name"),
        (
            json!({ "name": "x", "expires_in_days": 0 }),
            "expires_in_days",
        ),
        (
            json!({ "name": "x", "expires_in_days": 4000 }),
            "expires_in_days",
        ),
    ] {
        let (status, error) = viewer.post("/api/tokens", body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error["fields"][0]["field"], field);
    }
    token(&viewer, "dup").await;
    let (status, _) = viewer.post("/api/tokens", json!({ "name": "dup" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = viewer
        .post(
            "/api/tokens",
            json!({ "name": "soon", "expires_in_days": 30 }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body["expires_at"].is_string());

    // No token, or a made-up one, is no way in.
    let (status, _) = anonymous(&admin.state).get("/api/printers").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = with_token(&admin.state, "gantry_made-up")
        .get("/api/printers")
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
