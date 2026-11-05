//! The Bambu Driver end to end against the mock Bambu broker.
use std::time::Duration;

use gantry_core::{
    Connection, DisconnectReason, Driver, DriverEvent, EventReceiver, EventSender, PrinterConfig,
    PrinterHandle, PrinterState, PrinterStatus, Secrets,
};
use gantry_driver_bambu::BambuDriver;
use gantry_mock_bambu::MockBambu;

pub const SERIAL: &str = "TESTSERIAL00001";
pub const CODE: &str = "13572468";

pub fn config(address: &str, code: &str) -> PrinterConfig {
    PrinterConfig {
        id: "bambu-test".into(),
        name: "Bambu".into(),
        settings: serde_json::json!({ "host": address })
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

pub struct Connected {
    pub handle: Box<dyn PrinterHandle>,
    pub events: EventReceiver,
}

pub fn connect(address: &str, code: &str) -> Connected {
    let (sender, events) = EventSender::channel();
    let handle = BambuDriver.connect(config(address, code), sender);
    Connected { handle, events }
}

/// The next status satisfying `pred`, within `secs`.
pub async fn status(
    events: &mut EventReceiver,
    secs: u64,
    pred: impl Fn(&PrinterStatus) -> bool,
) -> PrinterStatus {
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            match events.recv().await.expect("the Driver is running") {
                DriverEvent::Status(s) if pred(&s) => return *s,
                _ => {}
            }
        }
    })
    .await
    .expect("the expected status")
}

fn disconnected_because(reason: DisconnectReason) -> impl Fn(&PrinterStatus) -> bool {
    move |s| matches!(s.connection, Connection::Disconnected { reason: r, .. } if r == reason)
}

async fn idle_printer() -> (MockBambu, Connected) {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("idle");
    let mut printer = connect(&mock.address().to_string(), CODE);
    status(&mut printer.events, 5, |s| s.connection.is_connected()).await;
    (mock, printer)
}

#[tokio::test]
async fn connects_and_maps_every_status_fixture() {
    let (mock, mut printer) = idle_printer().await;
    // The Driver asked for the whole state on connecting.
    assert!(
        mock.wait_for_request(Duration::from_secs(2), |r| {
            r["pushing"]["command"] == "pushall"
        })
        .await
        .is_some()
    );

    for (fixture, state) in [
        ("printing", PrinterState::Printing),
        ("paused", PrinterState::Paused),
        ("printing", PrinterState::Printing),
        ("finished", PrinterState::Idle),
        ("printing", PrinterState::Printing),
        ("failed", PrinterState::Idle),
        ("idle", PrinterState::Idle),
    ] {
        mock.report_fixture(fixture);
        let s = status(&mut printer.events, 3, |s| s.state == Some(state)).await;
        assert_eq!(s.job.is_some(), state != PrinterState::Idle, "{fixture}");
    }

    mock.report_fixture("printing");
    mock.report_fixture("printing_delta");
    let s = status(&mut printer.events, 3, |s| {
        s.job.as_ref().is_some_and(|j| j.progress == 44.0)
    })
    .await;
    let job = s.job.unwrap();
    assert_eq!(job.layer, Some(13));
    assert_eq!(job.remaining_secs, Some(63 * 60));
    assert_eq!(job.print_file.as_deref(), Some("benchy"));
    assert_eq!(s.temperatures.nozzle.unwrap().target, Some(220.0));
    assert!(
        s.fans
            .iter()
            .any(|f| f.name == "Part cooling" && f.percent == 100)
    );
    printer.handle.shutdown().await;
}

#[tokio::test]
async fn the_wrong_access_code_is_authentication_rejected() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    let mut printer = connect(&mock.address().to_string(), "00000000");
    status(
        &mut printer.events,
        5,
        disconnected_because(DisconnectReason::AuthenticationRejected),
    )
    .await;
    assert!(mock.refused_logins() >= 1);
    printer.handle.shutdown().await;
}

#[tokio::test]
async fn nothing_listening_is_unreachable() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let mut printer = connect(&address.to_string(), CODE);
    status(
        &mut printer.events,
        15,
        disconnected_because(DisconnectReason::Unreachable),
    )
    .await;
    printer.handle.shutdown().await;
}

#[tokio::test]
async fn a_peer_that_does_not_speak_tls_is_a_tls_failure() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = socket.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
        }
    });
    let mut printer = connect(&address.to_string(), CODE);
    status(
        &mut printer.events,
        5,
        disconnected_because(DisconnectReason::TlsFailure),
    )
    .await;
    printer.handle.shutdown().await;
}

#[tokio::test]
async fn reconnects_after_the_broker_drops_without_a_restart() {
    let (mock, mut printer) = idle_printer().await;
    mock.go_down();
    let s = status(&mut printer.events, 10, |s| !s.connection.is_connected()).await;
    assert!(matches!(s.connection, Connection::Disconnected { .. }));
    assert_eq!(s.state, None, "the state is unknown while disconnected");

    mock.report_fixture("printing");
    mock.come_back().await;
    let s = status(&mut printer.events, 20, |s| s.connection.is_connected()).await;
    // The fresh pushall brought it up to date.
    let s = if s.state == Some(PrinterState::Printing) {
        s
    } else {
        status(&mut printer.events, 5, |s| {
            s.state == Some(PrinterState::Printing)
        })
        .await
    };
    assert_eq!(s.job.unwrap().progress, 43.0);
    assert_eq!(mock.accepted_logins(), 2);
    printer.handle.shutdown().await;
}

#[tokio::test]
async fn shutting_down_closes_the_connection() {
    let (mock, printer) = idle_printer().await;
    assert_eq!(mock.open_connections(), 1);
    printer.handle.shutdown().await;
    for _ in 0..100 {
        if mock.open_connections() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the connection stayed open");
}

#[tokio::test]
async fn missing_credentials_are_reported_not_retried() {
    let (sender, mut events) = EventSender::channel();
    let mut config = config("127.0.0.1:1", CODE);
    config.secrets = Secrets::default();
    let handle = BambuDriver.connect(config, sender);
    let s = status(&mut events, 2, |s| !s.connection.is_connected()).await;
    assert!(matches!(
        s.connection,
        Connection::Disconnected {
            reason: DisconnectReason::AuthenticationRejected,
            ..
        }
    ));
    handle.shutdown().await;
}

#[tokio::test]
async fn filament_slots_and_faults_flow_and_faults_clear() {
    let (mock, mut printer) = idle_printer().await;
    mock.report_fixture("idle");
    let s = status(&mut printer.events, 3, |s| s.filament_slots.len() == 5).await;
    assert!(s.has(gantry_core::Capability::FilamentSlots));

    mock.report_fixture("ams_multiple");
    status(&mut printer.events, 3, |s| s.filament_slots.len() == 9).await;
    mock.report_fixture("ams_none");
    status(&mut printer.events, 3, |s| s.filament_slots.len() == 1).await;

    mock.report_fixture("hms_faults");
    let s = status(&mut printer.events, 3, |s| s.faults.len() == 3).await;
    assert_eq!(
        s.state,
        Some(PrinterState::Paused),
        "Faults sit beside the state"
    );
    // A later partial update without hms keeps them...
    mock.report_fixture("printing_delta");
    let s = status(&mut printer.events, 3, |s| {
        s.job.as_ref().is_some_and(|j| j.progress == 44.0)
    })
    .await;
    assert_eq!(s.faults.len(), 3);
    // ...and they clear when the printer stops reporting them.
    mock.report_fixture("hms_cleared");
    status(&mut printer.events, 3, |s| s.faults.is_empty()).await;
    printer.handle.shutdown().await;
}
