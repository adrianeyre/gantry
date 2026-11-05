//! The PrusaLink Driver end to end, through the Fleet, against a mock
//! PrusaLink (`mock/`) written from Prusa's OpenAPI description of the API.
//! No PrusaLink simulator exists (PRD §5.6), so these run under plain
//! `cargo test`, and real printers stay unverified.
mod mock;

use std::sync::Arc;
use std::time::Duration;

use gantry_core::{
    Capability, Command, Connection, DisconnectReason, DriverRegistry, FaultSeverity, Fleet,
    JobOutcome, JobStart, JobState, MemoryJobLog, PrintUpload, PrinterConfig, PrinterState,
    PrinterView, Secrets,
};
use gantry_driver_prusalink::PrusaLinkDriver;
use mock::{Auth, MockPrusaLink};
use serde_json::json;

const PASSWORD: &str = "Kx9mockPassword";
const API_KEY: &str = "mockApiKey7Q2";

fn digest() -> Auth {
    Auth::Digest {
        username: "maker".into(),
        password: PASSWORD.into(),
    }
}

fn config(host: &str, port: u16, secrets: &[(&str, &str)]) -> PrinterConfig {
    PrinterConfig {
        id: "p".into(),
        name: "Prusa".into(),
        settings: json!({ "host": host, "port": port, "model": "MK4" })
            .as_object()
            .unwrap()
            .clone(),
        secrets: Secrets::new(
            secrets
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        ),
    }
}

fn for_mock(mock: &MockPrusaLink, secrets: &[(&str, &str)]) -> PrinterConfig {
    config(&mock.host(), mock.port(), secrets)
}

struct Harness {
    fleet: Fleet,
    log: Arc<MemoryJobLog>,
}

fn harness(config: PrinterConfig) -> Harness {
    let mut registry = DriverRegistry::new();
    registry.register(PrusaLinkDriver);
    let log = Arc::new(MemoryJobLog::new());
    let fleet = Fleet::with_command_timeout(registry, log.clone(), Duration::from_secs(10));
    fleet
        .start("prusalink", config, JobState::default())
        .unwrap();
    Harness { fleet, log }
}

impl Harness {
    async fn until(
        &self,
        secs: u64,
        what: &str,
        pred: impl Fn(&PrinterView) -> bool,
    ) -> PrinterView {
        match self
            .fleet
            .wait_for("p", Duration::from_secs(secs), &pred)
            .await
        {
            Ok(v) => (*v).clone(),
            Err(_) => panic!(
                "timed out waiting for {what}; the Printer is now {:#?}",
                self.fleet.view("p").unwrap().status
            ),
        }
    }

    async fn state(&self, state: PrinterState) -> PrinterView {
        self.until(15, &format!("{state:?}"), |v| {
            v.status.connection.is_connected() && v.status.state == Some(state)
        })
        .await
    }

    async fn disconnected(&self, reason: DisconnectReason) -> PrinterView {
        self.until(15, &format!("disconnected: {reason:?}"), |v| {
            matches!(&v.status.connection, Connection::Disconnected { reason: r, .. } if *r == reason)
        })
        .await
    }
}

#[tokio::test]
async fn connects_with_digest_and_maps_the_status() {
    let mock = MockPrusaLink::start(digest()).await;
    let h = harness(for_mock(&mock, &[("password", PASSWORD)]));
    let view = h.state(PrinterState::Idle).await;
    assert_eq!(view.model.as_deref(), Some("MK4"));
    let nozzle = view.status.temperatures.nozzle.unwrap();
    assert_eq!((nozzle.current, nozzle.target), (214.8, Some(215.0)));
    assert_eq!(view.status.temperatures.bed.unwrap().current, 60.1);
    for capability in [
        Capability::Pause,
        Capability::Resume,
        Capability::Cancel,
        Capability::UploadAndStart,
    ] {
        assert!(view.status.has(capability), "{capability:?}");
    }
    assert!(!view.status.has(Capability::Camera));
    assert!(!view.status.has(Capability::NozzleTemperature));
    let requests = mock.requests();
    assert!(
        requests.contains(&"GET /api/version".to_owned()),
        "{requests:?}"
    );
    assert!(requests.contains(&"GET /api/v1/status".to_owned()));
    assert!(
        mock.with(|p| p.refused) >= 1,
        "the first request is challenged"
    );

    mock.with(|p| p.state = "BUSY".into());
    h.state(PrinterState::Busy).await;

    mock.with(|p| {
        p.state = "ERROR".into();
        p.message = Some("Heater failure".into());
    });
    let view = h
        .until(15, "an error Fault", |v| !v.status.faults.is_empty())
        .await;
    assert_eq!(
        view.status.state,
        Some(PrinterState::Idle),
        "no Error state"
    );
    assert_eq!(view.status.faults[0].severity, FaultSeverity::Error);
    assert_eq!(view.status.faults[0].message, "Heater failure");

    mock.with(|p| {
        p.state = "ATTENTION".into();
        p.message = None;
    });
    let view = h
        .until(15, "a warning Fault", |v| {
            v.status
                .faults
                .first()
                .is_some_and(|f| f.severity == FaultSeverity::Warning)
        })
        .await;
    assert_eq!(view.status.state, Some(PrinterState::Idle));

    mock.with(|p| p.state = "IDLE".into());
    h.until(15, "the Fault cleared", |v| v.status.faults.is_empty())
        .await;
    assert!(
        h.fleet.command("p", Command::Pause).await.is_err(),
        "nothing to pause"
    );
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn an_api_key_is_sent_to_older_versions() {
    let mock = MockPrusaLink::start(Auth::ApiKey(API_KEY.into())).await;
    let h = harness(for_mock(&mock, &[("api_key", API_KEY)]));
    h.state(PrinterState::Idle).await;
    assert_eq!(mock.with(|p| p.refused), 0);
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn an_external_print_is_paused_resumed_and_cancelled() {
    let mock = MockPrusaLink::start(digest()).await;
    let h = harness(for_mock(&mock, &[("password", PASSWORD)]));
    h.state(PrinterState::Idle).await;

    let id = mock.start_print("external.gcode");
    let view = h.state(PrinterState::Preparing).await;
    let active = view.status.job.clone().unwrap();
    assert_eq!(active.key, format!("prusalink-{id}@external.gcode"));
    assert_eq!(active.print_file.as_deref(), Some("external.gcode"));
    assert!(view.job.unwrap().external, "an External Job");

    mock.progress(30.0);
    let view = h.state(PrinterState::Printing).await;
    let active = view.status.job.unwrap();
    assert_eq!(active.progress, 30.0);
    assert_eq!(active.remaining_secs, Some(700));

    h.fleet.command("p", Command::Pause).await.unwrap();
    h.state(PrinterState::Paused).await;
    h.fleet.command("p", Command::Resume).await.unwrap();
    h.state(PrinterState::Printing).await;
    h.fleet.command("p", Command::Cancel).await.unwrap();
    let view = h.state(PrinterState::AwaitingClear).await;
    assert_eq!(
        view.awaiting_clear.unwrap().outcome,
        Some(JobOutcome::Cancelled)
    );
    let requests = mock.requests();
    for request in [
        format!("PUT /api/v1/job/{id}/pause"),
        format!("PUT /api/v1/job/{id}/resume"),
        format!("DELETE /api/v1/job/{id}"),
    ] {
        assert!(requests.contains(&request), "{request} in {requests:?}");
    }
    let jobs = h.log.jobs();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0].driver_key, active.key);
    h.fleet.mark_clear("p", false).await.unwrap();
    h.state(PrinterState::Idle).await;
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn a_plate_is_uploaded_started_and_runs_to_success() {
    let mock = MockPrusaLink::start(digest()).await;
    let h = harness(for_mock(&mock, &[("password", PASSWORD)]));
    h.state(PrinterState::Idle).await;

    let gcode = b"; sliced for MK4\nG28\nG1 X10 Y10\n".to_vec();
    let job = h
        .fleet
        .start_job(
            "p",
            JobStart {
                print_file_id: 3,
                print_file: "gantry cube.gcode".into(),
                plate: 1,
            },
            PrintUpload {
                file_name: "gantry cube.gcode".into(),
                plate: None,
                data: gcode.clone().into(),
                estimated_secs: None,
                layers: None,
            },
        )
        .await
        .unwrap();
    let uploads = mock.uploads();
    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0].path, "/api/v1/files/usb/gantry%20cube.gcode");
    assert_eq!(uploads[0].data, gcode, "the Plate's G-code, byte for byte");
    assert_eq!(uploads[0].print_after_upload.as_deref(), Some("?1"));
    assert_eq!(uploads[0].overwrite.as_deref(), Some("?1"));

    let view = h
        .until(15, "the uploaded print", |v| {
            v.status.job.is_some() && v.job.as_ref().is_some_and(|j| !j.driver_key.is_empty())
        })
        .await;
    let recorded = view.job.unwrap();
    assert_eq!(recorded.id, job.id, "the same Job, now keyed");
    assert!(!recorded.external);
    mock.progress(60.0);
    h.state(PrinterState::Printing).await;
    mock.finish();
    let view = h.state(PrinterState::AwaitingClear).await;
    let ended = view.awaiting_clear.unwrap();
    assert_eq!(ended.id, job.id);
    assert_eq!(ended.outcome, Some(JobOutcome::Succeeded));
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn a_wrong_password_or_key_is_authentication_rejected() {
    let mock = MockPrusaLink::start(digest()).await;
    let h = harness(for_mock(&mock, &[("password", "not-the-password")]));
    let view = h
        .disconnected(DisconnectReason::AuthenticationRejected)
        .await;
    match view.status.connection {
        Connection::Disconnected { detail, .. } => {
            assert!(!detail.unwrap_or_default().contains("not-the-password"));
        }
        other => panic!("{other:?}"),
    }
    h.fleet.stop_all().await;

    let mock = MockPrusaLink::start(Auth::ApiKey(API_KEY.into())).await;
    let h = harness(for_mock(&mock, &[("api_key", "not-the-key")]));
    h.disconnected(DisconnectReason::AuthenticationRejected)
        .await;
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn nothing_listening_is_unreachable() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let h = harness(config("127.0.0.1", port, &[("password", PASSWORD)]));
    h.disconnected(DisconnectReason::Unreachable).await;
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn reconnects_after_the_printer_restarts() {
    let mock = MockPrusaLink::start(digest()).await;
    let h = harness(for_mock(&mock, &[("password", PASSWORD)]));
    h.state(PrinterState::Idle).await;
    mock.go_down();
    let view = h.disconnected(DisconnectReason::Unreachable).await;
    assert!(view.status.state.is_none(), "unknown while disconnected");
    tokio::time::sleep(Duration::from_secs(1)).await;
    mock.come_back().await;
    h.until(30, "connected again", |v| {
        v.status.connection.is_connected() && v.status.state == Some(PrinterState::Idle)
    })
    .await;
    h.fleet.stop_all().await;
}
