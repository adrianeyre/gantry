//! Bambu pause, resume and Cancel, and Bambu prints recorded as Jobs, against
//! the mock Bambu broker and a real Fleet.
use std::sync::Arc;
use std::time::{Duration, Instant};

use gantry_core::{
    Command, CommandError, DriverRegistry, Fleet, FleetError, JobOutcome, JobState, MemoryJobLog,
    PrinterConfig, PrinterState, PrinterView, Secrets,
};
use gantry_driver_bambu::BambuDriver;
use gantry_mock_bambu::MockBambu;
use serde_json::Value;

const SERIAL: &str = "JOBSERIAL000123";
const CODE: &str = "97531864";

struct Harness {
    mock: MockBambu,
    fleet: Fleet,
    log: Arc<MemoryJobLog>,
}

async fn harness() -> Harness {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("idle");
    let mut registry = DriverRegistry::new();
    registry.register(BambuDriver);
    let log = Arc::new(MemoryJobLog::new());
    let fleet = Fleet::new(registry, log.clone());
    fleet
        .start(
            "bambu",
            PrinterConfig {
                id: "x1".into(),
                name: "X1".into(),
                settings: serde_json::json!({ "host": mock.address().to_string() })
                    .as_object()
                    .unwrap()
                    .clone(),
                secrets: Secrets::new(
                    [
                        ("serial".to_owned(), SERIAL.to_owned()),
                        ("access_code".to_owned(), CODE.to_owned()),
                    ]
                    .into(),
                ),
            },
            JobState::default(),
        )
        .unwrap();
    let h = Harness { mock, fleet, log };
    h.until(|v| v.status.state == Some(PrinterState::Idle))
        .await;
    h
}

impl Harness {
    async fn until(&self, pred: impl Fn(&PrinterView) -> bool) -> PrinterView {
        (*self
            .fleet
            .wait_for("x1", Duration::from_secs(10), pred)
            .await
            .unwrap())
        .clone()
    }

    async fn request(&self, command: &str) -> Value {
        self.mock
            .wait_for_request(Duration::from_secs(3), |r| r["print"]["command"] == command)
            .await
            .unwrap_or_else(|| panic!("no `{command}` request was recorded"))
    }

    async fn print(&self, fixtures: &[&str], until: PrinterState) -> PrinterView {
        for fixture in fixtures {
            self.mock.report_fixture(fixture);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.until(|v| v.status.state == Some(until)).await
    }
}

#[tokio::test]
async fn commands_publish_the_expected_requests() {
    let h = harness().await;
    h.print(&["printing"], PrinterState::Printing).await;

    h.fleet.command("x1", Command::Pause).await.unwrap();
    let pause = h.request("pause").await;
    assert_eq!(pause["print"]["param"], "");
    assert!(pause["print"]["sequence_id"].is_string());

    h.fleet.command("x1", Command::Resume).await.unwrap();
    h.request("resume").await;

    h.fleet.command("x1", Command::Cancel).await.unwrap();
    // Bambu calls Cancel "stop".
    h.request("stop").await;

    let sequences: Vec<String> = h
        .mock
        .requests()
        .iter()
        .filter_map(|r| r["print"]["sequence_id"].as_str().map(str::to_owned))
        .collect();
    let mut unique = sequences.clone();
    unique.dedup();
    assert_eq!(sequences.len(), 3);
    assert_eq!(unique, sequences, "each command has its own sequence id");
}

#[tokio::test]
async fn a_full_print_is_one_job_that_succeeded() {
    let h = harness().await;
    h.print(
        &["printing", "printing_delta", "paused", "printing"],
        PrinterState::Printing,
    )
    .await;
    let view = h.print(&["finished"], PrinterState::AwaitingClear).await;
    let jobs = h.log.jobs();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert!(jobs[0].external, "a print Gantry didn't start");
    assert_eq!(jobs[0].print_file.as_deref(), Some("benchy"));
    assert_eq!(jobs[0].plate, Some(1));
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Succeeded));
    assert_eq!(view.awaiting_clear.unwrap().id, jobs[0].id);
}

#[tokio::test]
async fn a_failed_print_is_failed() {
    let h = harness().await;
    h.print(&["printing"], PrinterState::Printing).await;
    h.print(&["failed"], PrinterState::AwaitingClear).await;
    assert_eq!(h.log.jobs()[0].outcome, Some(JobOutcome::Failed));
}

#[tokio::test]
async fn a_print_stopped_on_the_printer_is_cancelled() {
    let h = harness().await;
    h.print(&["printing"], PrinterState::Printing).await;
    h.print(&["cancelled"], PrinterState::AwaitingClear).await;
    assert_eq!(h.log.jobs()[0].outcome, Some(JobOutcome::Cancelled));
}

#[tokio::test]
async fn a_print_gantry_cancels_is_cancelled_then_awaiting_clear() {
    let h = harness().await;
    h.print(&["printing"], PrinterState::Printing).await;
    h.fleet.command("x1", Command::Cancel).await.unwrap();
    h.request("stop").await;
    // Whatever error the printer gives the stopped print, it was a Cancel.
    h.print(&["failed"], PrinterState::AwaitingClear).await;
    let jobs = h.log.jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Cancelled));
}

#[tokio::test]
async fn commands_while_disconnected_are_refused_at_once_and_never_sent() {
    let h = harness().await;
    h.print(&["printing"], PrinterState::Printing).await;
    h.mock.go_down();
    h.until(|v| !v.status.connection.is_connected()).await;

    let started = Instant::now();
    let refused = h.fleet.command("x1", Command::Pause).await;
    assert!(started.elapsed() < Duration::from_millis(200));
    assert!(
        matches!(
            refused,
            Err(FleetError::Command(CommandError::Disconnected { .. }))
        ),
        "{refused:?}"
    );

    h.mock.come_back().await;
    h.until(|v| v.status.state == Some(PrinterState::Printing))
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        h.mock
            .requests()
            .iter()
            .all(|r| r["print"]["command"] != "pause"),
        "the refused pause was not queued: {:?}",
        h.mock.requests()
    );
}

#[tokio::test]
async fn a_print_that_ends_while_disconnected_is_recorded_on_reconnection() {
    let h = harness().await;
    h.print(&["printing"], PrinterState::Printing).await;
    h.mock.go_down();
    h.until(|v| !v.status.connection.is_connected()).await;
    // It finished while Gantry couldn't see it.
    h.mock.report_fixture("finished");
    h.mock.come_back().await;
    h.until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    let jobs = h.log.jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Succeeded));
}
