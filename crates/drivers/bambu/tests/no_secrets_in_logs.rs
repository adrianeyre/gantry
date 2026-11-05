//! The access code and serial number never appear in a log, at any level,
//! from Gantry or from the MQTT client underneath it, nor from an upload over
//! FTPS.
//!
//! Its own test binary, so a global subscriber can see every event from every
//! thread.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use gantry_core::{
    Connection, Driver, DriverEvent, EventSender, PrintUpload, PrinterConfig, PrinterState, Secrets,
};
use gantry_driver_bambu::BambuDriver;
use gantry_mock_bambu::MockBambu;
use gantry_mock_bambu::ftps::MockFtps;

const SERIAL: &str = "LOGSERIAL000042";
const CODE: &str = "86420975";

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

fn config(address: &str, ftp_port: u16, code: &str) -> PrinterConfig {
    PrinterConfig {
        id: "logs".into(),
        name: "Logged".into(),
        settings: serde_json::json!({ "host": address, "ftp_port": ftp_port })
            .as_object()
            .unwrap()
            .clone(),
        secrets: Secrets::new(
            [
                ("serial".to_owned(), SERIAL.to_owned()),
                ("access_code".to_owned(), code.to_owned()),
            ]
            .into(),
        ),
    }
}

async fn wait(events: &mut gantry_core::EventReceiver, connected: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
        if let DriverEvent::Status(s) = event
            && s.connection.is_connected() == connected
            && (!connected || s.state == Some(PrinterState::Printing))
        {
            return;
        }
    }
    panic!("timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn neither_the_serial_nor_the_access_code_is_logged() {
    let captured = Captured::default();
    let writer = captured.clone();
    // Everything at trace, with the Driver's own directives as Gantry applies
    // them. rumqttd is the mock printer's broker, not Gantry, so it is off.
    let mut filter = tracing_subscriber::EnvFilter::new("trace,rumqttd=off");
    for directive in gantry_driver_bambu::LOG_DIRECTIVES {
        filter = filter.add_directive(directive.parse().unwrap());
    }
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();

    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("printing");
    let ftps = MockFtps::start(CODE).await;
    let address = mock.address().to_string();

    // A good connection, a status, a drop and a reconnection...
    let (sender, mut events) = EventSender::channel();
    let good = BambuDriver.connect(config(&address, ftps.port(), CODE), sender);
    wait(&mut events, true).await;
    mock.go_down();
    wait(&mut events, false).await;
    mock.come_back().await;
    wait(&mut events, true).await;

    // ...an upload, and one whose login is refused...
    let upload = || PrintUpload {
        file_name: "cube.gcode.3mf".into(),
        plate: Some(1),
        data: Bytes::from_static(b"a print file"),
        estimated_secs: None,
        layers: None,
    };
    good.upload_and_start(upload()).await.unwrap();
    assert_eq!(ftps.files().len(), 1);
    ftps.refuse_login();
    let refused = good.upload_and_start(upload()).await.unwrap_err();
    assert!(!refused.to_string().contains(CODE), "{refused}");
    good.shutdown().await;

    // ...and a refused connection.
    let (sender, mut events) = EventSender::channel();
    let refused = BambuDriver.connect(config(&address, ftps.port(), "11111111"), sender);
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if let DriverEvent::Status(s) = event
                && matches!(s.connection, Connection::Disconnected { .. })
            {
                return;
            }
        }
    })
    .await
    .unwrap();
    refused.shutdown().await;

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("connected to a Bambu printer"),
        "logs were captured:\n{logs}"
    );
    assert!(logs.contains("Bambu printer disconnected"), "{logs}");
    assert!(logs.contains("uploaded a Print File over FTPS"), "{logs}");
    for secret in [SERIAL, CODE, "11111111"] {
        assert!(
            !logs.contains(secret),
            "`{secret}` reached the log:\n{logs}"
        );
    }
}
