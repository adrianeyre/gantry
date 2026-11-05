//! The board's password never appears in a log, at any level, from Gantry or
//! from the HTTP client underneath it, even though RepRapFirmware takes it in
//! `rr_connect`'s query: not while connecting, not when a connection drops,
//! not when it is refused, and not in an upload.
//!
//! Its own test binary, so a global subscriber can see every event from every
//! thread.
mod mock;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use gantry_core::{
    Connection, DisconnectReason, Driver, DriverEvent, EventReceiver, EventSender, PrintUpload,
    PrinterConfig, PrinterState, PrinterStatus, Secrets,
};
use gantry_driver_duet::DuetDriver;
use mock::MockDuet;

const PASSWORD: &str = "LogDuetPass8812";
const WRONG: &str = "WrongDuetPass3307";

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

fn config(mock: &MockDuet, password: &str) -> PrinterConfig {
    PrinterConfig {
        id: "logs".into(),
        name: "Logged".into(),
        settings: serde_json::json!({ "host": mock.host(), "port": mock.port() })
            .as_object()
            .unwrap()
            .clone(),
        secrets: Secrets::new([("password".to_owned(), password.to_owned())].into()),
    }
}

async fn wait(events: &mut EventReceiver, what: impl Fn(&PrinterStatus) -> bool) {
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

fn idle(s: &PrinterStatus) -> bool {
    s.connection.is_connected() && s.state == Some(PrinterState::Idle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_password_is_never_logged() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("trace"))
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();

    // A good connection, an expired session, a drop and a reconnection, and
    // an upload...
    let mock = MockDuet::start(Some(PASSWORD)).await;
    let (sender, mut events) = EventSender::channel();
    let good = DuetDriver.connect(config(&mock, PASSWORD), sender);
    wait(&mut events, idle).await;
    mock.expire_sessions();
    tokio::time::sleep(Duration::from_secs(2)).await;
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
    good.shutdown().await;

    // ...and a refused password.
    let (sender, mut events) = EventSender::channel();
    let refused = DuetDriver.connect(config(&mock, WRONG), sender);
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

    assert!(mock.with(|b| b.connects) >= 3, "sessions were opened");
    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("connected to a Duet"),
        "logs were captured:\n{logs}"
    );
    assert!(logs.contains("Duet connection down"), "{logs}");
    assert!(logs.contains("sent a Print File"), "{logs}");
    for secret in [PASSWORD, WRONG] {
        assert!(
            !logs.contains(secret),
            "`{secret}` reached the log:\n{logs}"
        );
    }
}
