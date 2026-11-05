//! The PrusaLink password and API key never appear in a log, at any level,
//! from Gantry or from the HTTP client underneath it: not while connecting,
//! not in a refused connection, and not in an upload.
//!
//! Its own test binary, so a global subscriber can see every event from every
//! thread.
mod mock;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use gantry_core::{
    Connection, DisconnectReason, Driver, DriverEvent, EventReceiver, EventSender, PrintUpload,
    PrinterConfig, PrinterState, Secrets,
};
use gantry_driver_prusalink::PrusaLinkDriver;
use mock::{Auth, MockPrusaLink};

const PASSWORD: &str = "LogPassw0rd4471";
const WRONG: &str = "WrongPassw0rd9902";
const API_KEY: &str = "LogApiKey55813";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn config(mock: &MockPrusaLink, key: &str, value: &str) -> PrinterConfig {
    PrinterConfig {
        id: "logs".into(),
        name: "Logged".into(),
        settings: serde_json::json!({ "host": mock.host(), "port": mock.port() })
            .as_object()
            .unwrap()
            .clone(),
        secrets: Secrets::new([(key.to_owned(), value.to_owned())].into()),
    }
}

async fn wait(events: &mut EventReceiver, what: impl Fn(&gantry_core::PrinterStatus) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
        if let DriverEvent::Status(s) = event
            && what(&s)
        {
            return;
        }
    }
    panic!("timed out");
}

fn idle(s: &gantry_core::PrinterStatus) -> bool {
    s.connection.is_connected() && s.state == Some(PrinterState::Idle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn neither_the_password_nor_the_api_key_is_logged() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("trace"))
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();

    // A good connection, a drop and a reconnection, and an upload...
    let mock = MockPrusaLink::start(Auth::Digest {
        username: "maker".into(),
        password: PASSWORD.into(),
    })
    .await;
    let (sender, mut events) = EventSender::channel();
    let good = PrusaLinkDriver.connect(config(&mock, "password", PASSWORD), sender);
    wait(&mut events, idle).await;
    mock.go_down();
    wait(&mut events, |s| {
        matches!(s.connection, Connection::Disconnected { .. })
    })
    .await;
    mock.come_back().await;
    wait(&mut events, idle).await;
    good.upload_and_start(PrintUpload {
        file_name: "cube.gcode".into(),
        plate: None,
        data: Bytes::from_static(b"G28\n"),
        estimated_secs: None,
        layers: None,
    })
    .await
    .unwrap();
    assert_eq!(mock.uploads().len(), 1);
    good.shutdown().await;

    // ...a refused password...
    let (sender, mut events) = EventSender::channel();
    let refused = PrusaLinkDriver.connect(config(&mock, "password", WRONG), sender);
    wait(&mut events, |s| {
        matches!(
            s.connection,
            Connection::Disconnected {
                reason: DisconnectReason::AuthenticationRejected,
                ..
            }
        )
    })
    .await;
    refused.shutdown().await;

    // ...and an API key.
    let keyed = MockPrusaLink::start(Auth::ApiKey(API_KEY.into())).await;
    let (sender, mut events) = EventSender::channel();
    let key = PrusaLinkDriver.connect(config(&keyed, "api_key", API_KEY), sender);
    wait(&mut events, idle).await;
    key.shutdown().await;

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("connected to PrusaLink"),
        "logs were captured:\n{logs}"
    );
    assert!(logs.contains("PrusaLink connection down"), "{logs}");
    assert!(logs.contains("sent a Print File"), "{logs}");
    for secret in [PASSWORD, WRONG, API_KEY] {
        assert!(
            !logs.contains(secret),
            "`{secret}` reached the log:\n{logs}"
        );
    }
}
