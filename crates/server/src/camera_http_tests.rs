//! HTTP cameras (MJPEG streams and snapshots, as Moonraker and OctoPrint
//! serve them) through the camera relay, the same as any other camera.
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use gantry_core::{DriverRegistry, Fleet};
use gantry_driver_support::mjpeg::MjpegCamera;
use http_body_util::BodyExt;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::AppState;
use crate::crypto::SecretKey;
use crate::store::Store;
use crate::test_support::Client;

fn jpeg() -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, 80);
    encoder.set_progressive(false);
    encoder
        .encode(&[90u8; 16 * 16 * 3], 16, 16, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    out
}

/// A local MJPEG streamer, as mjpg-streamer serves one.
async fn streamer() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0u8; 1024];
                let _ = socket.read(&mut request).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace;boundary=boundarydonotcross\r\n\r\n")
                    .await;
                let frame = jpeg();
                loop {
                    let head = format!(
                        "--boundarydonotcross\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                        frame.len()
                    );
                    if socket.write_all(head.as_bytes()).await.is_err()
                        || socket.write_all(&frame).await.is_err()
                        || socket.write_all(b"\r\n").await.is_err()
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            });
        }
    });
    format!("http://{address}/?action=stream")
}

/// A test double: a Driver whose Printers have an HTTP camera at `camera_url`.
struct CameraDriver;

struct CameraHandle(Arc<MjpegCamera>);

impl gantry_core::Driver for CameraDriver {
    fn id(&self) -> &'static str {
        "http-camera"
    }
    fn name(&self) -> &'static str {
        "HTTP camera"
    }
    fn fields(&self) -> Vec<gantry_core::FieldSpec> {
        vec![gantry_core::FieldSpec::new(
            "camera_url",
            "Camera",
            gantry_core::FieldKind::Text,
        )]
    }
    fn connect(
        &self,
        config: gantry_core::PrinterConfig,
        events: gantry_core::EventSender,
    ) -> Box<dyn gantry_core::PrinterHandle> {
        let mut status =
            gantry_core::PrinterStatus::disconnected(gantry_core::DisconnectReason::Stopped);
        status.connection = gantry_core::Connection::Connected;
        status.state = Some(gantry_core::PrinterState::Idle);
        status.capabilities.insert(gantry_core::Capability::Camera);
        events.status(status);
        let url = config.text("camera_url").unwrap_or_default().to_owned();
        Box::new(CameraHandle(Arc::new(MjpegCamera::new(
            url,
            Default::default(),
        ))))
    }
}

#[async_trait::async_trait]
impl gantry_core::PrinterHandle for CameraHandle {
    async fn pause(&self) -> Result<(), gantry_core::CommandError> {
        Err(gantry_core::CommandError::Unsupported)
    }
    async fn resume(&self) -> Result<(), gantry_core::CommandError> {
        Err(gantry_core::CommandError::Unsupported)
    }
    async fn cancel(&self) -> Result<(), gantry_core::CommandError> {
        Err(gantry_core::CommandError::Unsupported)
    }
    fn camera(&self) -> Option<Arc<dyn gantry_core::CameraSource>> {
        Some(self.0.clone())
    }
    async fn shutdown(&self) {}
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

async fn client_with(registry: DriverRegistry) -> Client {
    let store = Store::in_memory().await.unwrap();
    let key = SecretKey::from_config("a secret key for tests only").unwrap();
    let log = Arc::new(crate::store::jobs::SqlJobLog(store.clone()));
    Client::with_state(AppState::new(store, key, false, Fleet::new(registry, log))).await
}

/// Reads the relay's stream until it has delivered a JPEG frame.
async fn a_frame_through_the_relay(client: &Client, id: &str) {
    let response = client.raw_get(&format!("/api/printers/{id}/camera")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("multipart/x-mixed-replace")
    );
    let mut body = response.into_body();
    let mut seen = Vec::new();
    let found = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame.unwrap().into_data() {
                seen.extend_from_slice(&data);
                if seen.windows(2).any(|w| w == [0xFF, 0xD8])
                    && seen.windows(2).any(|w| w == [0xFF, 0xD9])
                {
                    return true;
                }
            }
        }
        false
    })
    .await;
    assert_eq!(found, Ok(true), "a JPEG frame through the relay");
}

#[tokio::test]
async fn an_mjpeg_camera_plugs_into_the_relay() {
    let mut registry = DriverRegistry::new();
    registry.register(CameraDriver);
    let client = client_with(registry).await;
    let (status, printer) = client
        .post(
            "/api/printers",
            json!({ "name": "Klipper cam", "driver": "http-camera",
                    "settings": { "camera_url": streamer().await } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{printer}");
    let id = printer["id"].as_str().unwrap().to_owned();
    client
        .until(&id, |v| v.status.connection.is_connected())
        .await;
    a_frame_through_the_relay(&client, &id).await;
}

#[tokio::test]
#[ignore = "needs the simulated-Klipper container (docker/simulators/compose.yaml)"]
async fn a_moonraker_printers_camera_plugs_into_the_relay() {
    let mut registry = DriverRegistry::new();
    registry.register(gantry_driver_moonraker::MoonrakerDriver);
    let client = client_with(registry).await;
    let address = std::env::var("GANTRY_MOONRAKER").unwrap_or_else(|_| "127.0.0.1:7125".into());
    let (host, port) = address.rsplit_once(':').unwrap();
    let camera = std::env::var("GANTRY_MOONRAKER_CAMERA")
        .unwrap_or_else(|_| "http://127.0.0.1:8110/?action=stream".into());
    let (status, printer) = client
        .post(
            "/api/printers",
            json!({ "name": "Virtual Klipper", "driver": "moonraker",
                    "settings": { "host": host, "port": port.parse::<u16>().unwrap(),
                                  "camera_url": camera } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{printer}");
    let id = printer["id"].as_str().unwrap().to_owned();
    client
        .state
        .fleet()
        .wait_for(&id, Duration::from_secs(60), |v| {
            v.status.has(gantry_core::Capability::Camera)
        })
        .await
        .unwrap();
    a_frame_through_the_relay(&client, &id).await;
}

#[tokio::test]
#[ignore = "needs OctoPrint's virtual printer (docker/simulators/compose.yaml)"]
async fn an_octoprint_printers_camera_plugs_into_the_relay() {
    let mut registry = DriverRegistry::new();
    registry.register(gantry_driver_octoprint::OctoPrintDriver);
    let client = client_with(registry).await;
    let address = std::env::var("GANTRY_OCTOPRINT").unwrap_or_else(|_| "127.0.0.1:5080".into());
    let (host, port) = address.rsplit_once(':').unwrap();
    let key = std::env::var("GANTRY_OCTOPRINT_API_KEY")
        .unwrap_or_else(|_| "GANTRYTESTKEY0123456789ABCDEFGHIJ".into());
    let (status, printer) = client
        .post(
            "/api/printers",
            json!({ "name": "Virtual OctoPrint", "driver": "octoprint",
                    "settings": { "host": host, "port": port.parse::<u16>().unwrap(),
                                  "camera_url": streamer().await },
                    "secrets": { "api_key": key } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{printer}");
    let id = printer["id"].as_str().unwrap().to_owned();
    client
        .state
        .fleet()
        .wait_for(&id, Duration::from_secs(60), |v| {
            v.status.has(gantry_core::Capability::Camera)
        })
        .await
        .unwrap();
    a_frame_through_the_relay(&client, &id).await;
}
