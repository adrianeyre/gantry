//! Embedded mode (#75): the core and server inside the app, with no separate
//! server. Starts it as the app does, creates the first Admin, adds a
//! Simulated Printer through the API and sees it connect.
mod common;

use gantry_desktop::embedded::{CONFIG_FILE, Embedded, EmbeddedOptions, FirstAdminError};
use gantry_desktop::probe;
use gantry_desktop::server_url::ServerUrl;

async fn start(data: &std::path::Path, preferred_port: Option<u16>) -> Embedded {
    Embedded::start(EmbeddedOptions {
        data_dir: data.to_owned(),
        secret_key: common::SECRET_KEY.to_owned(),
        preferred_port,
        drivers: gantry_desktop::drivers::registry(),
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn embedded_mode_runs_simulated_printers_with_no_separate_server() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("server");
    let embedded = start(&data, None).await;

    // Only this machine can reach it, and it is a Gantry server.
    assert!(embedded.address().ip().is_loopback());
    let server = ServerUrl::parse(&embedded.url()).unwrap();
    let http = probe::client();
    probe::check_gantry(&http, &server).await.unwrap();
    // The data lives where the app put it.
    assert!(data.join("gantry.db").exists());
    assert!(data.join(CONFIG_FILE).exists());

    // No account yet: the desktop page creates the first Admin, once.
    assert!(embedded.needs_first_admin().await.unwrap());
    embedded
        .create_first_admin(common::USERNAME, common::PASSWORD)
        .await
        .unwrap();
    assert!(!embedded.needs_first_admin().await.unwrap());
    assert!(matches!(
        embedded
            .create_first_admin("someone-else", common::PASSWORD)
            .await,
        Err(FirstAdminError::AccountsExist)
    ));

    // Log in, add a Simulated Printer through the API, and watch it connect
    // on the dashboard's WebSocket.
    let cookie = common::log_in(&http, &server).await;
    let mut socket = common::fleet_socket(&server, &cookie).await;
    let created = http
        .post(server.join("/api/printers"))
        .header("cookie", &cookie)
        .json(&serde_json::json!({ "name": "Bench Sim", "driver": "simulated", "settings": {} }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let id = created.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let connected = |printer: &serde_json::Value| {
        printer["id"] == id.as_str() && printer["status"]["connection"]["state"] == "connected"
    };
    let message = common::until_message(&mut socket, |m| match m["type"].as_str() {
        Some("printer") => connected(&m["printer"]),
        Some("snapshot") => m["printers"].as_array().unwrap().iter().any(connected),
        _ => false,
    })
    .await;
    assert!(message.to_string().contains("Bench Sim"), "{message}");

    embedded.stop().await;
}

#[tokio::test]
async fn the_next_start_keeps_the_port_the_data_and_the_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let first = start(dir.path(), None).await;
    let port = first.address().port();
    first
        .create_first_admin(common::USERNAME, common::PASSWORD)
        .await
        .unwrap();
    first.stop().await;

    let again = start(dir.path(), Some(port)).await;
    assert_eq!(again.address().port(), port);
    assert!(!again.needs_first_admin().await.unwrap());
    again.stop().await;
}

#[tokio::test]
async fn a_taken_port_falls_back_to_a_free_one() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let embedded = start(dir.path(), Some(port)).await;
    assert_ne!(embedded.address().port(), port);
    embedded.stop().await;
}

#[tokio::test]
async fn a_short_secret_key_is_refused_as_gantry_refuses_it() {
    let dir = tempfile::tempdir().unwrap();
    let err = Embedded::start(EmbeddedOptions {
        data_dir: dir.path().to_owned(),
        secret_key: "short".into(),
        preferred_port: None,
        drivers: gantry_desktop::drivers::registry(),
    })
    .await
    .err()
    .unwrap();
    assert!(err.to_string().contains("secret key"), "{err}");
}
