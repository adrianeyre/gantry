//! Each role, against each group of endpoints (PRD §7.6, §9.2). The role is
//! enforced on the server: these call the API directly, as a script would,
//! with nothing hidden by a UI.
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::store::users::Role;
use crate::test_support::Client;

/// One request: method, path and body. `{id}` is replaced with a Printer's
/// id and `{user}` with a user's id.
type Call = (&'static str, &'static str, Option<Value>);

/// Reads: every role may make them.
fn reads() -> Vec<Call> {
    vec![
        ("GET", "/api/session", None),
        ("GET", "/api/drivers", None),
        ("GET", "/api/printers", None),
        ("GET", "/api/printers/{id}", None),
        ("GET", "/api/printers/{id}/jobs", None),
    ]
}

/// Printer control and Job management: Operators and Admins.
fn control() -> Vec<Call> {
    vec![
        (
            "POST",
            "/api/printers/{id}/commands",
            Some(json!({ "command": "pause" })),
        ),
        (
            "POST",
            "/api/printers/{id}/mark-clear",
            Some(json!({ "overrule_to_failed": false })),
        ),
    ]
}

/// Printers, users, settings and the simulation panel: Admins only.
fn admin() -> Vec<Call> {
    vec![
        (
            "POST",
            "/api/printers",
            Some(json!({ "name": "Made by {role}", "driver": "simulated" })),
        ),
        (
            "PUT",
            "/api/printers/{id}",
            Some(json!({ "name": "Target", "settings": {} })),
        ),
        ("GET", "/api/users", None),
        (
            "POST",
            "/api/users",
            Some(
                json!({ "username": "new-{role}", "password": "a long password", "role": "viewer" }),
            ),
        ),
        (
            "PUT",
            "/api/users/{user}",
            Some(json!({ "role": "viewer" })),
        ),
        ("GET", "/api/printers/{id}/simulation", None),
        (
            "POST",
            "/api/printers/{id}/simulation",
            Some(json!({ "action": "set_speed", "speed": 2 })),
        ),
        // Last, so the Printer exists for everything above.
        ("DELETE", "/api/users/{user}", None),
    ]
}

struct World {
    admin: Client,
    printer: String,
    target_user: i64,
}

async fn world() -> World {
    let admin = Client::new().await;
    let printer = admin.add_simulated("Target", json!({})).await;
    admin
        .state
        .fleet()
        .wait_for(&printer, Duration::from_secs(5), |v| {
            v.status.connection.is_connected()
        })
        .await
        .unwrap();
    let (status, user) = admin
        .post(
            "/api/users",
            json!({ "username": "target", "password": "a long password", "role": "viewer" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{user}");
    World {
        admin,
        printer,
        target_user: user["id"].as_i64().unwrap(),
    }
}

async fn call(
    client: &Client,
    world: &World,
    role: Role,
    (method, path, body): &Call,
) -> StatusCode {
    let path = path
        .replace("{id}", &world.printer)
        .replace("{user}", &world.target_user.to_string());
    let body = body
        .as_ref()
        .map(|b| serde_json::from_str(&b.to_string().replace("{role}", role.as_str())).unwrap());
    client.send(method, &path, body).await.0
}

/// Checks every call in `group` is refused (403) below `least` and not refused
/// at or above it.
async fn check(group: &str, calls: Vec<Call>, least: Role) {
    for role in Role::ALL {
        let world = world().await;
        let client = world.admin.as_role(role).await;
        for request in &calls {
            let status = call(&client, &world, role, request).await;
            if role < least {
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "{group}: {role:?} must be refused {} {}",
                    request.0,
                    request.1
                );
            } else {
                assert_ne!(
                    status,
                    StatusCode::FORBIDDEN,
                    "{group}: {role:?} must be allowed {} {}",
                    request.0,
                    request.1
                );
                assert_ne!(status, StatusCode::UNAUTHORIZED, "{group}: {role:?}");
            }
        }
    }
}

#[tokio::test]
async fn every_role_can_read() {
    check("reads", reads(), Role::Viewer).await;
}

#[tokio::test]
async fn only_operators_and_admins_control_printers_and_jobs() {
    check("control", control(), Role::Operator).await;
}

#[tokio::test]
async fn only_admins_manage_printers_users_and_the_simulation() {
    check("admin", admin(), Role::Admin).await;
}

#[tokio::test]
async fn only_admins_remove_printers() {
    for role in Role::ALL {
        let world = world().await;
        let client = world.admin.as_role(role).await;
        let status = client
            .delete(&format!("/api/printers/{}", world.printer))
            .await
            .0;
        if role == Role::Admin {
            assert_eq!(status, StatusCode::NO_CONTENT);
        } else {
            assert_eq!(status, StatusCode::FORBIDDEN, "{role:?}");
            assert!(world.admin.state.fleet().contains(&world.printer));
        }
    }
}

#[tokio::test]
async fn a_refusal_says_which_role_is_needed() {
    let world = world().await;
    let viewer = world.admin.as_role(Role::Viewer).await;
    let (status, body) = viewer
        .post(
            &format!("/api/printers/{}/commands", world.printer),
            json!({ "command": "pause" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["needed"], "operator");
    assert!(
        body["error"].as_str().unwrap().contains("Operator"),
        "{body}"
    );
}

#[tokio::test]
async fn the_last_admin_cannot_be_removed_or_demoted_through_the_api() {
    let admin = Client::new().await;
    let (_, me) = admin.get("/api/session").await;
    let id = me["id"].as_i64().unwrap();
    let (status, body) = admin
        .put(&format!("/api/users/{id}"), json!({ "role": "operator" }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("last Admin"));
    let (status, _) = admin.delete(&format!("/api/users/{id}")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Still an Admin, still logged in.
    assert_eq!(admin.get("/api/session").await.1["role"], "admin");
}

#[tokio::test]
async fn a_role_change_applies_to_live_sessions_at_once() {
    let world = world().await;
    let operator = world.admin.as_role(Role::Operator).await;
    let (_, me) = operator.get("/api/session").await;
    assert_eq!(me["role"], "operator");
    let id = me["id"].as_i64().unwrap();
    world
        .admin
        .put(&format!("/api/users/{id}"), json!({ "role": "viewer" }))
        .await;
    let (status, _) = operator
        .post(
            &format!("/api/printers/{}/commands", world.printer),
            json!({ "command": "pause" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
