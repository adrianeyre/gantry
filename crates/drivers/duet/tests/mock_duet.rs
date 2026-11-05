//! The Duet Driver end to end, through the Fleet, against a mock Duet
//! (`mock/`) written from Duet3D's documentation of RepRapFirmware 3's HTTP
//! API. They run under plain `cargo test`; real boards stay unverified.
mod mock;

use std::sync::Arc;
use std::time::Duration;

use gantry_core::{
    Axis, Capability, Command, Connection, DisconnectReason, DriverRegistry, FaultSeverity,
    FineControl, Fleet, Heater, JobOutcome, JobStart, JobState, MemoryJobLog, PrintUpload,
    PrinterConfig, PrinterState, PrinterView, Secrets,
};
use gantry_driver_duet::DuetDriver;
use mock::MockDuet;
use serde_json::json;

const PASSWORD: &str = "duetMockPass31";

fn config(host: &str, port: u16, password: Option<&str>) -> PrinterConfig {
    PrinterConfig {
        id: "d".into(),
        name: "Duet".into(),
        settings: json!({ "host": host, "port": port, "model": "Voron 2.4" })
            .as_object()
            .unwrap()
            .clone(),
        secrets: Secrets::new(
            password
                .map(|p| ("password".to_owned(), p.to_owned()))
                .into_iter()
                .collect(),
        ),
    }
}

fn for_mock(mock: &MockDuet, password: Option<&str>) -> PrinterConfig {
    config(&mock.host(), mock.port(), password)
}

struct Harness {
    fleet: Fleet,
    log: Arc<MemoryJobLog>,
}

fn harness(config: PrinterConfig) -> Harness {
    let mut registry = DriverRegistry::new();
    registry.register(DuetDriver);
    let log = Arc::new(MemoryJobLog::new());
    let fleet = Fleet::with_command_timeout(registry, log.clone(), Duration::from_secs(10));
    fleet.start("duet", config, JobState::default()).unwrap();
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
            .wait_for("d", Duration::from_secs(secs), &pred)
            .await
        {
            Ok(v) => (*v).clone(),
            Err(_) => panic!(
                "timed out waiting for {what}; the Printer is now {:#?}",
                self.fleet.view("d").unwrap().status
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
async fn connects_and_maps_the_object_model() {
    let mock = MockDuet::start(Some(PASSWORD)).await;
    let h = harness(for_mock(&mock, Some(PASSWORD)));
    let view = h.state(PrinterState::Idle).await;
    assert_eq!(view.model.as_deref(), Some("Voron 2.4"));
    let nozzle = view.status.temperatures.nozzle.unwrap();
    assert_eq!((nozzle.current, nozzle.target), (205.2, Some(210.0)));
    assert_eq!(view.status.temperatures.bed.unwrap().current, 59.8);
    assert_eq!(view.status.fans.len(), 2);
    assert_eq!(view.status.fans[0].percent, 35);
    assert!(view.status.fans[0].controllable && !view.status.fans[1].controllable);
    for capability in [
        Capability::Pause,
        Capability::Resume,
        Capability::Cancel,
        Capability::UploadAndStart,
        Capability::NozzleTemperature,
        Capability::BedTemperature,
        Capability::Homing,
        Capability::Jogging,
        Capability::Fans,
    ] {
        assert!(view.status.has(capability), "{capability:?}");
    }
    assert!(!view.status.has(Capability::Camera));
    let requests = mock.with(|b| b.requests.clone());
    assert_eq!(requests[0], "/rr_connect");
    assert!(requests.contains(&"/rr_model".to_owned()));

    mock.with(|b| b.status = "busy".into());
    h.state(PrinterState::Busy).await;
    mock.with(|b| b.status = "halted".into());
    let view = h
        .until(15, "a Fault", |v| !v.status.faults.is_empty())
        .await;
    assert_eq!(
        view.status.state,
        Some(PrinterState::Idle),
        "no Error state"
    );
    assert_eq!(view.status.faults[0].severity, FaultSeverity::Error);
    mock.with(|b| b.status = "idle".into());
    h.until(15, "the Fault cleared", |v| v.status.faults.is_empty())
        .await;
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn an_external_print_is_paused_resumed_and_cancelled() {
    let mock = MockDuet::start(None).await;
    let h = harness(for_mock(&mock, None));
    h.state(PrinterState::Idle).await;

    mock.start_print("0:/gcodes/external.gcode");
    let view = h.state(PrinterState::Preparing).await;
    let active = view.status.job.clone().unwrap();
    assert!(active.key.starts_with("external.gcode@"), "{}", active.key);
    assert_eq!(active.print_file.as_deref(), Some("external.gcode"));
    assert!(view.job.unwrap().external, "an External Job");

    mock.advance(25, 5);
    let view = h.state(PrinterState::Printing).await;
    let job = view.status.job.unwrap();
    assert_eq!(job.progress, 25.0);
    assert_eq!((job.layer, job.total_layers), (Some(5), Some(20)));
    assert_eq!(job.remaining_secs, Some(300));
    assert_eq!(job.key, active.key, "the key holds for the whole print");

    h.fleet.command("d", Command::Pause).await.unwrap();
    h.state(PrinterState::Paused).await;
    h.fleet.command("d", Command::Resume).await.unwrap();
    h.state(PrinterState::Printing).await;
    h.fleet.command("d", Command::Cancel).await.unwrap();
    let view = h.state(PrinterState::AwaitingClear).await;
    assert_eq!(
        view.awaiting_clear.unwrap().outcome,
        Some(JobOutcome::Cancelled)
    );
    assert_eq!(
        mock.gcodes(),
        ["M25", "M24", "M25", "M0"],
        "Cancel pauses, then M0"
    );
    let jobs = h.log.jobs();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0].driver_key, active.key);
    h.fleet.mark_clear("d", false).await.unwrap();
    h.state(PrinterState::Idle).await;
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn a_plate_is_uploaded_started_and_runs_to_success() {
    let mock = MockDuet::start(Some(PASSWORD)).await;
    let h = harness(for_mock(&mock, Some(PASSWORD)));
    h.state(PrinterState::Idle).await;

    let gcode = b"; sliced for a Voron\nG28\nG1 X10 Y10\n".to_vec();
    let job = h
        .fleet
        .start_job(
            "d",
            JobStart {
                print_file_id: 5,
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
    let files = mock.with(|b| b.files.clone());
    assert_eq!(
        files.get("0:/gcodes/gantry cube.gcode"),
        Some(&gcode),
        "the Plate's G-code, byte for byte: {:?}",
        files.keys()
    );
    assert_eq!(mock.gcodes(), ["M32 \"0:/gcodes/gantry cube.gcode\""]);

    let view = h
        .until(15, "the uploaded print", |v| {
            v.status.job.is_some() && v.job.as_ref().is_some_and(|j| !j.driver_key.is_empty())
        })
        .await;
    let recorded = view.job.unwrap();
    assert_eq!(recorded.id, job.id, "the same Job, now keyed");
    assert!(!recorded.external);
    mock.advance(70, 14);
    h.state(PrinterState::Printing).await;
    mock.finish();
    let view = h.state(PrinterState::AwaitingClear).await;
    let ended = view.awaiting_clear.unwrap();
    assert_eq!(ended.id, job.id);
    assert_eq!(ended.outcome, Some(JobOutcome::Succeeded));
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn fine_controls_are_g_code() {
    let mock = MockDuet::start(None).await;
    let h = harness(for_mock(&mock, None));
    h.state(PrinterState::Idle).await;
    for control in [
        FineControl::SetTemperature {
            heater: Heater::Nozzle,
            target: 200.0,
        },
        FineControl::SetTemperature {
            heater: Heater::Bed,
            target: 55.0,
        },
        FineControl::SetFan {
            fan: "0".into(),
            percent: 50,
        },
        FineControl::Home,
        FineControl::Jog {
            axis: Axis::X,
            distance_mm: 10.0,
        },
        FineControl::Jog {
            axis: Axis::Z,
            distance_mm: -0.5,
        },
    ] {
        h.fleet
            .command("d", Command::Fine(control.clone()))
            .await
            .unwrap_or_else(|e| panic!("{control:?}: {e}"));
    }
    assert_eq!(
        mock.gcodes(),
        [
            "M104 T0 S200",
            "M140 S55",
            "M106 P0 S0.50",
            "G28",
            "G91",
            "G1 X10 F6000",
            "G90",
            "G91",
            "G1 Z-0.5 F600",
            "G90"
        ]
    );
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn a_wrong_password_is_authentication_rejected() {
    let mock = MockDuet::start(Some(PASSWORD)).await;
    let h = harness(for_mock(&mock, Some("not-the-password")));
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

    // No password where the board wants one is refused too.
    let h = harness(for_mock(&mock, None));
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
    let h = harness(config("127.0.0.1", port, None));
    h.disconnected(DisconnectReason::Unreachable).await;
    h.fleet.stop_all().await;
}

#[tokio::test]
async fn reconnects_after_the_board_restarts_and_when_its_session_expires() {
    let mock = MockDuet::start(Some(PASSWORD)).await;
    let h = harness(for_mock(&mock, Some(PASSWORD)));
    h.state(PrinterState::Idle).await;

    // An expired session is opened again without losing the Connection.
    mock.expire_sessions();
    let mut updates = h.fleet.subscribe();
    tokio::time::timeout(Duration::from_secs(10), async {
        while mock.with(|b| b.connects) < 2 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("a new session");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    while let Ok(update) = updates.try_recv() {
        if let gantry_core::FleetUpdate::Printer(view) = update {
            assert!(
                view.status.connection.is_connected(),
                "{:?}",
                view.status.connection
            );
        }
    }

    // A restarted board is a lost Connection that recovers.
    mock.expire_sessions();
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
