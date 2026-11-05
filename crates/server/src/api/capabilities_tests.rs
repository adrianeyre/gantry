//! An Admin's capability overrides (PRD §6.1).
use std::time::Duration;

use axum::http::StatusCode;
use gantry_core::{Capability, PrinterView};
use serde_json::json;

use crate::AppState;
use crate::config::Config;
use crate::store::users::Role;
use crate::test_support::{Client, INERT, drivers};

async fn until(state: &AppState, id: &str, pred: impl Fn(&PrinterView) -> bool) -> PrinterView {
    (*state
        .fleet()
        .wait_for(id, Duration::from_secs(5), pred)
        .await
        .unwrap())
    .clone()
}

#[tokio::test]
async fn an_override_survives_a_restart_and_a_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_str().unwrap().to_owned();
    let config = Config::load(None, move |k| match k {
        "GANTRY_DATA_DIR" => Some(data.clone()),
        "GANTRY_SECRET_KEY" => Some("a secret key for tests only".into()),
        _ => None,
    })
    .unwrap();

    let first = AppState::start(&config, drivers()).await.unwrap();
    let client = Client::with_state(first.clone()).await;
    let id = client
        .add_simulated("Overridden", json!({ "camera": false }))
        .await;
    until(&first, &id, |v| v.status.connection.is_connected()).await;
    // Add the camera the Printer doesn't report; remove homing it does.
    let (status, view) = client
        .put(
            &format!("/api/printers/{id}/capabilities"),
            json!({ "overrides": { "camera": true, "homing": false } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let caps = view["status"]["capabilities"].as_array().unwrap();
    assert!(caps.contains(&json!("camera")));
    assert!(!caps.contains(&json!("homing")));
    assert!(
        !view["reported_capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("camera"))
    );
    assert_eq!(
        view["capability_overrides"],
        json!({ "camera": true, "homing": false })
    );

    // A reconnect: the Driver reports again, and the override still holds.
    client
        .post(
            &format!("/api/printers/{id}/simulation"),
            json!({ "action": "drop_connection" }),
        )
        .await;
    until(&first, &id, |v| !v.status.connection.is_connected()).await;
    client
        .post(
            &format!("/api/printers/{id}/simulation"),
            json!({ "action": "restore_connection" }),
        )
        .await;
    let view = until(&first, &id, |v| v.status.connection.is_connected()).await;
    assert!(view.status.has(Capability::Camera));
    assert!(!view.status.has(Capability::Homing));

    // A restart.
    first.fleet().stop_all().await;
    first.store().pool().close().await;
    drop((client, first));
    let second = AppState::start(&config, drivers()).await.unwrap();
    let view = until(&second, &id, |v| v.status.connection.is_connected()).await;
    assert!(view.status.has(Capability::Camera));
    assert!(!view.status.has(Capability::Homing));
    assert_eq!(view.capability_overrides.len(), 2);

    // Resetting puts back what the Driver reports.
    let client = Client::with_state(second.clone()).await;
    let (status, view) = client
        .put(
            &format!("/api/printers/{id}/capabilities"),
            json!({ "overrides": {} }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let caps = view["status"]["capabilities"].as_array().unwrap();
    assert!(!caps.contains(&json!("camera")));
    assert!(caps.contains(&json!("homing")));
    second.fleet().stop_all().await;
}

#[tokio::test]
async fn a_command_the_driver_cannot_really_perform_still_fails_cleanly() {
    let client = Client::new().await;
    // The Inert Driver reports no capabilities and can do nothing.
    let (status, created) = client
        .post("/api/printers", json!({ "name": "Inert", "driver": INERT }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap();
    until(&client.state, id, |v| v.status.connection.is_connected()).await;
    let (status, _) = client
        .put(
            &format!("/api/printers/{id}/capabilities"),
            json!({ "overrides": { "pause": true, "homing": true, "camera": true } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    for body in [json!({ "command": "pause" }), json!({ "command": "home" })] {
        let (status, body) = client
            .post(&format!("/api/printers/{id}/commands"), body)
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["reason"]["kind"], "unsupported");
    }
    // No camera to relay, so a clean 404 rather than a hang.
    let response = client.raw_get(&format!("/api/printers/{id}/camera")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_admins_override_and_each_change_is_audited() {
    let client = Client::new().await;
    let id = client.add_simulated("Audited caps", json!({})).await;
    until(&client.state, &id, |v| v.status.connection.is_connected()).await;
    let operator = client.as_role(Role::Operator).await;
    let (status, _) = operator
        .put(
            &format!("/api/printers/{id}/capabilities"),
            json!({ "overrides": { "pause": false } }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = client
        .put(
            &format!("/api/printers/{id}/capabilities"),
            json!({ "overrides": { "pause": false } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let entries = client
        .state
        .store()
        .audit_entries(&crate::store::audit::Filter {
            action: Some("printer.capabilities".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].detail,
        Some(json!({ "overrides": { "pause": false } }))
    );
    assert_eq!(
        client
            .put(
                "/api/printers/nope/capabilities",
                json!({ "overrides": {} })
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
