use std::time::Duration;

use axum::http::StatusCode;
use gantry_core::DriverRegistry;
use gantry_driver_simulated::SimulatedPrinter;
use serde_json::json;

use crate::AppState;
use crate::config::Config;
use crate::test_support::{Client, drivers};

const CODE: &str = "SECRET-ACCESS-CODE-42";

#[tokio::test]
async fn lists_the_drivers_and_their_fields() {
    let client = Client::new().await;
    let (status, body) = client.get("/api/drivers").await;
    assert_eq!(status, StatusCode::OK);
    let simulated = body
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "simulated")
        .unwrap();
    let secret = simulated["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == "access_code")
        .unwrap();
    assert_eq!(secret["kind"], "secret");
}

#[tokio::test]
async fn adding_a_printer_starts_it() {
    let client = Client::new().await;
    let id = client.add_simulated("Bench", json!({})).await;
    let view = client
        .state
        .fleet()
        .wait_for(&id, Duration::from_secs(3), |v| {
            v.status.connection.is_connected()
        })
        .await
        .unwrap();
    assert_eq!(view.name, "Bench");
}

#[tokio::test]
async fn credentials_are_encrypted_and_never_returned() {
    let client = Client::new().await;
    let (status, created) = client
        .post(
            "/api/printers",
            json!({
                "name": "Secretive", "driver": "simulated",
                "settings": {}, "secrets": { "access_code": CODE },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["secrets_set"], json!(["access_code"]));
    let id = created["id"].as_str().unwrap();

    let raw: String = sqlx::query_scalar("SELECT secrets FROM printers")
        .fetch_one(client.state.store().pool())
        .await
        .unwrap();
    assert!(!raw.contains(CODE));

    for path in ["/api/printers".to_owned(), format!("/api/printers/{id}")] {
        let (_, body) = client.get(&path).await;
        assert!(!body.to_string().contains(CODE), "{body}");
    }
    assert!(!created.to_string().contains(CODE));
}

#[tokio::test]
async fn editing_keeps_a_blank_credential_and_replaces_a_new_one() {
    let client = Client::new().await;
    let (_, created) = client
        .post(
            "/api/printers",
            json!({ "name": "P", "driver": "simulated", "secrets": { "access_code": CODE } }),
        )
        .await;
    let id = created["id"].as_str().unwrap();
    let secret = || async {
        client
            .state
            .store()
            .printer_secrets(client.state.key(), id)
            .await
            .unwrap()
            .get("access_code")
            .map(str::to_owned)
    };

    let (status, _) = client
        .put(
            &format!("/api/printers/{id}"),
            json!({ "name": "Renamed", "settings": { "model": "Z" }, "secrets": { "access_code": "" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(secret().await.as_deref(), Some(CODE));

    client
        .put(
            &format!("/api/printers/{id}"),
            json!({ "name": "Renamed", "secrets": { "access_code": "NEW-CODE" } }),
        )
        .await;
    assert_eq!(secret().await.as_deref(), Some("NEW-CODE"));

    client
        .put(
            &format!("/api/printers/{id}"),
            json!({ "name": "Renamed", "secrets": { "access_code": null } }),
        )
        .await;
    assert_eq!(secret().await, None);

    // The running Printer was restarted with the new name.
    let view = client
        .state
        .fleet()
        .wait_for(id, Duration::from_secs(2), |v| v.name == "Renamed")
        .await
        .unwrap();
    assert_eq!(view.name, "Renamed");
}

#[tokio::test]
async fn validation_errors_name_their_field() {
    let client = Client::new().await;
    let (status, body) = client
        .post(
            "/api/printers",
            json!({ "name": " ", "driver": "simulated", "settings": { "filament_slots": 99 } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let fields: Vec<&str> = body["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["name", "filament_slots"]);

    let (status, body) = client
        .post(
            "/api/printers",
            json!({ "name": "X", "driver": "nonsense" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["fields"][0]["field"], "driver");

    client.add_simulated("Taken", json!({})).await;
    let (status, body) = client
        .post(
            "/api/printers",
            json!({ "name": "taken", "driver": "simulated" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["fields"][0]["field"], "name");
}

#[tokio::test]
async fn removing_a_printer_stops_its_task() {
    let client = Client::new().await;
    let id = client.add_simulated("Doomed", json!({})).await;
    let handle = client.state.fleet().handle(&id).unwrap();
    let sim = handle
        .as_any()
        .downcast_ref::<SimulatedPrinter>()
        .unwrap()
        .clone();
    drop(handle);
    assert!(sim.is_running());

    let (status, _) = client.delete(&format!("/api/printers/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!client.state.fleet().contains(&id));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!sim.is_running(), "the Printer's task has stopped");
    let (status, _) = client.get(&format!("/api/printers/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = client.delete(&format!("/api/printers/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_printer_survives_a_restart_and_reconnects() {
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
    let (_, created) = client
        .post(
            "/api/printers",
            json!({ "name": "Survivor", "driver": "simulated", "secrets": { "access_code": CODE } }),
        )
        .await;
    let id = created["id"].as_str().unwrap().to_owned();
    first.fleet().stop_all().await;
    first.store().pool().close().await;
    drop((client, first));

    let second = AppState::start(&config, drivers()).await.unwrap();
    let view = second
        .fleet()
        .wait_for(&id, Duration::from_secs(3), |v| {
            v.status.connection.is_connected()
        })
        .await
        .unwrap();
    assert_eq!(view.name, "Survivor");
    let (_, printer, error) = second
        .store()
        .printer_config(second.key(), &id)
        .await
        .unwrap()
        .unwrap();
    assert!(error.is_none());
    assert_eq!(printer.secrets.get("access_code"), Some(CODE));
    second.fleet().stop_all().await;
}

#[tokio::test]
async fn a_printer_whose_driver_is_missing_does_not_stop_startup() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_str().unwrap().to_owned();
    let config = Config::load(None, move |k| match k {
        "GANTRY_DATA_DIR" => Some(data.clone()),
        "GANTRY_SECRET_KEY" => Some("a secret key for tests only".into()),
        _ => None,
    })
    .unwrap();
    let first = AppState::start(&config, drivers()).await.unwrap();
    Client::with_state(first.clone())
        .await
        .add_simulated("Orphan", json!({}))
        .await;
    first.fleet().stop_all().await;
    first.store().pool().close().await;

    let second = AppState::start(&config, DriverRegistry::new())
        .await
        .unwrap();
    assert!(second.fleet().ids().is_empty());
}
