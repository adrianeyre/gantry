//! Shared by the desktop app's integration tests.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use futures::StreamExt;
use gantry_desktop::server_url::ServerUrl;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub const USERNAME: &str = "admin";
pub const PASSWORD: &str = "a password for tests";
pub const SECRET_KEY: &str = "a secret key for the desktop tests";

/// Serves `router` on a free port of 127.0.0.1.
pub async fn serve(router: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    address
}

/// Logs in through `POST /api/login`, as the dashboard's login page does,
/// and returns the session cookie.
pub async fn log_in(http: &reqwest::Client, server: &ServerUrl) -> String {
    let response = http
        .post(server.join("/api/login"))
        .json(&serde_json::json!({ "username": USERNAME, "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK, "logging in");
    let cookie = response
        .headers()
        .get("set-cookie")
        .expect("a session cookie")
        .to_str()
        .unwrap();
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    cookie.split(';').next().unwrap().to_owned()
}

/// Opens the dashboard's WebSocket with a session, as the desktop app does.
pub async fn fleet_socket(
    server: &ServerUrl,
    cookie: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut url = server.join("/api/ws");
    url.set_scheme("ws").unwrap();
    url.set_query(Some("client=desktop"));
    let mut request = url.as_str().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
}

/// Reads WebSocket messages until one satisfies `pred`, for up to 10 s.
pub async fn until_message(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    pred: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let message = socket.next().await.expect("the socket stays open").unwrap();
            if let Ok(text) = message.to_text()
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(text)
                && pred(&json)
            {
                return json;
            }
        }
    })
    .await
    .expect("the message arrived in time")
}
