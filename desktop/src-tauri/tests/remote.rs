//! Remote mode (#74): the desktop app's connect logic against a real Gantry
//! server started in-process, and against servers that are not Gantry.
mod common;

use std::net::SocketAddr;

use axum::routing::get;
use gantry_desktop::probe::{self, ProbeError};
use gantry_desktop::server_url::ServerUrl;
use gantry_server::AppState;
use gantry_server::config::Config;
use gantry_server::store::users::Role;

/// A real Gantry server, with the Simulated Driver, a temporary data folder,
/// a secret key and one Admin, as `gantry` would serve it.
async fn gantry() -> (SocketAddr, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_string_lossy().into_owned();
    let config = Config::load(None, move |name| match name {
        "GANTRY_DATA_DIR" => Some(data.clone()),
        "GANTRY_SECRET_KEY" => Some(common::SECRET_KEY.to_owned()),
        _ => None,
    })
    .unwrap();
    let mut drivers = gantry_core::DriverRegistry::new();
    drivers.register(gantry_driver_simulated::SimulatedDriver);
    let state = AppState::start(&config, drivers).await.unwrap();
    state
        .store()
        .create_user(common::USERNAME, common::PASSWORD, Role::Admin)
        .await
        .unwrap();
    (common::serve(gantry_server::app(state)).await, dir)
}

#[tokio::test]
async fn the_connect_check_accepts_a_gantry_server_typed_as_host_and_port() {
    let (address, _dir) = gantry().await;
    // As someone types it: no scheme, so http:// is assumed.
    let server = ServerUrl::parse(&address.to_string()).unwrap();
    assert_eq!(server.to_string(), format!("http://{address}"));
    probe::check_gantry(&probe::client(), &server)
        .await
        .unwrap();
}

#[tokio::test]
async fn logging_in_at_the_chosen_address_gives_a_session_and_the_dashboard() {
    let (address, _dir) = gantry().await;
    let server = ServerUrl::parse(&format!("http://{address}/")).unwrap();
    let http = probe::client();
    probe::check_gantry(&http, &server).await.unwrap();

    // The window goes to the server's own page, which sends a visitor with
    // no session to its login page: the embedded React app.
    let home = http.get(server.home()).send().await.unwrap();
    assert!(home.status().is_redirection(), "{}", home.status());
    let location = home.headers()["location"].to_str().unwrap();
    assert!(location.contains("/login"), "{location}");
    let page = http.get(server.join("/login")).send().await.unwrap();
    assert_eq!(page.status(), reqwest::StatusCode::OK);
    let html = page.text().await.unwrap();
    assert!(html.contains("<title>Gantry"), "{html}");

    let cookie = common::log_in(&http, &server).await;
    let session = http
        .get(server.join("/api/session"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(session.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = session.json().await.unwrap();
    assert_eq!(body["username"], common::USERNAME, "{body}");

    // And the desktop app's WebSocket opens with that session.
    let mut socket = common::fleet_socket(&server, &cookie).await;
    common::until_message(&mut socket, |m| m["type"] == "snapshot").await;
}

#[tokio::test]
async fn the_connect_check_refuses_a_web_server_that_is_not_gantry() {
    let address =
        common::serve(axum::Router::new().fallback(|| async { "Hello from a router" })).await;
    let server = ServerUrl::parse(&format!("http://{address}")).unwrap();
    let err = probe::check_gantry(&probe::client(), &server)
        .await
        .unwrap_err();
    assert!(matches!(err, ProbeError::NotGantry { .. }), "{err}");
    assert!(err.to_string().contains("not a Gantry server"), "{err}");
}

#[tokio::test]
async fn the_connect_check_refuses_a_server_whose_session_needs_no_login() {
    // Another app with a /healthz of its own.
    let router = axum::Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/session", get(|| async { "{}" }));
    let address = common::serve(router).await;
    let server = ServerUrl::parse(&format!("http://{address}")).unwrap();
    let err = probe::check_gantry(&probe::client(), &server)
        .await
        .unwrap_err();
    assert!(matches!(err, ProbeError::NotGantry { .. }), "{err}");
    assert!(err.to_string().contains("/api/session"), "{err}");
}

#[tokio::test]
async fn the_connect_check_says_when_nothing_answers() {
    // A port that was free a moment ago.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let server = ServerUrl::parse(&format!("127.0.0.1:{port}")).unwrap();
    let err = probe::check_gantry(&probe::client(), &server)
        .await
        .unwrap_err();
    assert!(matches!(err, ProbeError::Unreachable { .. }), "{err}");
}
