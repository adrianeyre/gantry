//! An Admin adds a Bambu Printer through the API: it connects to the (mock)
//! printer, and its serial number and access code are encrypted at rest and
//! never sent back.
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gantry_core::{DriverRegistry, Fleet, PrinterState};
use gantry_mock_bambu::MockBambu;
use gantry_server::AppState;
use gantry_server::crypto::SecretKey;
use gantry_server::store::Store;
use gantry_server::store::jobs::SqlJobLog;
use gantry_server::store::users::Role;
use http_body_util::BodyExt;
use tower::ServiceExt;

const SERIAL: &str = "APISERIAL000777";
const CODE: &str = "24681357";

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn an_admin_adds_a_bambu_printer() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("printing");

    let store = Store::in_memory().await.unwrap();
    store
        .create_user("admin", "an admin password", Role::Admin)
        .await
        .unwrap();
    let mut drivers = DriverRegistry::new();
    drivers.register(gantry_driver_bambu::BambuDriver);
    let fleet = Fleet::new(drivers, Arc::new(SqlJobLog(store.clone())));
    let key = SecretKey::from_config("a secret key for tests only").unwrap();
    let state = AppState::new(store.clone(), key, false, fleet);
    let app = gantry_server::app(state.clone());

    let login = app
        .clone()
        .oneshot(
            Request::post("/api/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"username":"admin","password":"an admin password"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    // The form the UI builds from the Driver's fields.
    let (status, drivers) = send(
        &app,
        Request::get("/api/drivers")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(drivers.contains("\"id\":\"bambu\""));

    let body = serde_json::json!({
        "name": "X1 in the garage",
        "driver": "bambu",
        "settings": { "host": mock.address().to_string() },
        "secrets": { "serial": SERIAL, "access_code": CODE },
    });
    let (status, created) = send(
        &app,
        Request::post("/api/printers")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert!(!created.contains(SERIAL) && !created.contains(CODE));
    let id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // It connects and shows live status.
    let view = state
        .fleet()
        .wait_for(&id, Duration::from_secs(10), |v| {
            v.status.state == Some(PrinterState::Printing)
        })
        .await
        .unwrap();
    assert_eq!(view.status.job.as_ref().unwrap().progress, 43.0);

    // Encrypted at rest, and never sent back.
    let raw: String = sqlx::query_scalar("SELECT secrets FROM printers")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert!(!raw.contains(SERIAL) && !raw.contains(CODE));
    let settings: String = sqlx::query_scalar("SELECT settings FROM printers")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert!(!settings.contains(SERIAL));
    let (_, listed) = send(
        &app,
        Request::get("/api/printers")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(
        !listed.contains(SERIAL) && !listed.contains(CODE),
        "{listed}"
    );

    // A missing access code is a validation error, inline.
    let (status, refused) = send(
        &app,
        Request::post("/api/printers")
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "name": "No code", "driver": "bambu", "settings": { "host": "10.0.0.9" }, "secrets": { "serial": SERIAL } }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(refused.contains("access_code"), "{refused}");
    state.fleet().stop_all().await;
}
