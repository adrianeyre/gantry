//! The camera endpoint, through the router, with Simulated Printers.
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use gantry_driver_simulated::SimulatedPrinter;
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

use crate::test_support::Client;

async fn connected(client: &Client, name: &str, settings: serde_json::Value) -> String {
    let id = client.add_simulated(name, settings).await;
    client
        .state
        .fleet()
        .wait_for(&id, Duration::from_secs(2), |v| {
            v.status.connection.is_connected()
        })
        .await
        .unwrap();
    id
}

fn sim(client: &Client, id: &str) -> SimulatedPrinter {
    client
        .state
        .fleet()
        .handle(id)
        .unwrap()
        .as_any()
        .downcast_ref::<SimulatedPrinter>()
        .unwrap()
        .clone()
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn camera_endpoints_need_a_session() {
    let client = Client::new().await;
    let id = connected(&client, "Cam", json!({})).await;
    let response = crate::app(client.state.clone())
        .oneshot(
            Request::get(format!("/api/printers/{id}/camera"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(sim(&client, &id).open_camera_streams(), 0);
}

#[tokio::test]
async fn streams_mjpeg_only_while_watched() {
    let client = Client::new().await;
    let id = connected(&client, "Cam", json!({})).await;
    let printer = sim(&client, &id);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(printer.open_camera_streams(), 0, "nobody is watching yet");

    let response = client.raw_get(&format!("/api/printers/{id}/camera")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("multipart/x-mixed-replace; boundary=")
    );
    let mut body = response.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert!(first.starts_with(b"--gantry-frame\r\nContent-Type: image/jpeg\r\n"));
    let jpeg_start = first.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    assert_eq!(&first[jpeg_start..jpeg_start + 2], [0xFF, 0xD8]);
    assert_eq!(printer.open_camera_streams(), 1);

    // Leaving stops the pull from the Driver.
    drop(body);
    eventually("the stream to stop", || printer.open_camera_streams() == 0).await;
    assert_eq!(client.state.cameras().active(), 0);
}

#[tokio::test]
async fn several_browsers_share_one_pull() {
    let client = Client::new().await;
    let id = connected(&client, "Cam", json!({})).await;
    let printer = sim(&client, &id);
    let mut bodies = Vec::new();
    for _ in 0..3 {
        let mut body = client
            .raw_get(&format!("/api/printers/{id}/camera"))
            .await
            .into_body();
        body.frame().await.unwrap().unwrap();
        bodies.push(body);
    }
    assert_eq!(printer.open_camera_streams(), 1);
    drop(bodies);
    eventually("the stream to stop", || printer.open_camera_streams() == 0).await;
}

#[tokio::test]
async fn shutting_down_ends_camera_streams() {
    let client = Client::new().await;
    let id = connected(&client, "Cam", json!({})).await;
    let mut body = client
        .raw_get(&format!("/api/printers/{id}/camera"))
        .await
        .into_body();
    body.frame().await.unwrap().unwrap();
    client.state.begin_shutdown();
    let end = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(frame) = body.frame().await {
            frame.unwrap();
        }
    })
    .await;
    assert!(end.is_ok(), "the stream ended");
}

#[tokio::test]
async fn a_printer_without_a_camera_has_none() {
    let client = Client::new().await;
    let id = connected(&client, "No cam", json!({ "camera": false })).await;
    assert!(
        !client
            .state
            .fleet()
            .view(&id)
            .unwrap()
            .status
            .has(gantry_core::Capability::Camera)
    );
    let response = client.raw_get(&format!("/api/printers/{id}/camera")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = client.raw_get("/api/printers/nope/camera").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A session cookie for `client`'s Admin, so a test can send its own
/// requests, from a given address say.
async fn session_cookie(client: &Client) -> String {
    let response = crate::app(client.state.clone())
        .oneshot(
            Request::post("/api/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "username": "admin", "password": crate::test_support::PASSWORD })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

/// A camera request as a browser at `from` would send it, with an
/// `X-Forwarded-For` header if given.
async fn camera_from(
    client: &Client,
    cookie: &str,
    path: &str,
    from: &str,
    forwarded_for: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::get(path)
        .header("cookie", cookie)
        .extension(ConnectInfo(SocketAddr::new(from.parse().unwrap(), 50000)));
    if let Some(ip) = forwarded_for {
        request = request.header("x-forwarded-for", ip);
    }
    crate::app(client.state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn quality(response: &axum::response::Response) -> &str {
    response.headers()["x-gantry-camera-quality"]
        .to_str()
        .unwrap()
}

#[tokio::test]
async fn remote_viewers_get_medium_and_local_ones_full() {
    let client = Client::new().await;
    let id = connected(&client, "Cam", json!({})).await;
    let cookie = session_cookie(&client).await;
    let path = format!("/api/printers/{id}/camera");

    // Without a known address (as in most tests) the viewer is local.
    let local = client.raw_get(&path).await;
    assert_eq!(quality(&local), "full");
    let lan = camera_from(&client, &cookie, &path, "192.168.1.20", None).await;
    assert_eq!(quality(&lan), "full");

    let remote = camera_from(&client, &cookie, &path, "203.0.113.9", None).await;
    assert_eq!(remote.status(), StatusCode::OK);
    assert_eq!(quality(&remote), "medium");
    let tailscale = camera_from(&client, &cookie, &path, "100.101.102.103", None).await;
    assert_eq!(quality(&tailscale), "medium");
    let mut body = remote.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert!(first.starts_with(b"--gantry-frame\r\nContent-Type: image/jpeg\r\n"));

    // Both medium viewers share one transcoder, and everyone the pull.
    assert_eq!(sim(&client, &id).open_camera_streams(), 1);
    assert_eq!(client.state.cameras().transcoders(), 1);
    drop((local, lan, tailscale, body));
    eventually("the stream to stop", || {
        sim(&client, &id).open_camera_streams() == 0
    })
    .await;
    assert_eq!(client.state.cameras().transcoders(), 0);
}

#[tokio::test]
async fn through_a_trusted_proxy_the_viewer_is_remote() {
    let options = crate::Options {
        trusted_proxies: crate::client::TrustedProxies::parse_list("127.0.0.1").unwrap(),
        ..crate::Options::default()
    };
    let client = Client::with_state(crate::test_support::state_with(options).await).await;
    let id = connected(&client, "Cam", json!({})).await;
    let cookie = session_cookie(&client).await;
    let path = format!("/api/printers/{id}/camera");
    // A tunnel on this host forwards a LAN-looking address: still remote.
    let proxied = camera_from(&client, &cookie, &path, "127.0.0.1", Some("192.168.1.7")).await;
    assert_eq!(quality(&proxied), "medium");
    // A browser on the LAN, not through the tunnel, is local.
    let direct = camera_from(&client, &cookie, &path, "192.168.1.7", None).await;
    assert_eq!(quality(&direct), "full");
}

#[tokio::test]
async fn a_viewer_can_pick_the_quality() {
    let client = Client::new().await;
    let id = connected(&client, "Cam", json!({})).await;
    let cookie = session_cookie(&client).await;
    let path = format!("/api/printers/{id}/camera");

    let low = client.raw_get(&format!("{path}?quality=low")).await;
    assert_eq!(low.status(), StatusCode::OK);
    assert_eq!(quality(&low), "low");
    let remote_full = format!("{path}?quality=full");
    let full = camera_from(&client, &cookie, &remote_full, "203.0.113.9", None).await;
    assert_eq!(quality(&full), "full");
    let medium = client.raw_get(&format!("{path}?quality=medium")).await;
    assert_eq!(quality(&medium), "medium");
    assert_eq!(sim(&client, &id).open_camera_streams(), 1);
    assert_eq!(client.state.cameras().transcoders(), 2);
    drop((low, full, medium));

    // An unknown level is refused, and opens nothing.
    let (status, body) = client.get(&format!("{path}?quality=ultra")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    eventually("the stream to stop", || {
        sim(&client, &id).open_camera_streams() == 0
    })
    .await;
    assert_eq!(client.state.cameras().transcoders(), 0);
}
