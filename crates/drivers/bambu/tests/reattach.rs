//! Job re-attachment after a Gantry restart or a reconnect (#21), against the
//! mock Bambu broker: the "printer" keeps its state while Gantry's Fleet is
//! replaced, as a real printer would.
use std::sync::Arc;
use std::time::Duration;

use gantry_core::{
    DriverRegistry, Fleet, JobOutcome, JobState, MemoryJobLog, PrinterConfig, PrinterState,
    PrinterView, Secrets,
};
use gantry_driver_bambu::BambuDriver;
use gantry_mock_bambu::MockBambu;
use serde_json::json;

const SERIAL: &str = "01PREATTACH0001";
const CODE: &str = "13572468";

fn config(mock: &MockBambu) -> PrinterConfig {
    PrinterConfig {
        id: "p1s".into(),
        name: "P1S".into(),
        settings: json!({ "host": mock.address().to_string() })
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
    }
}

/// A Fleet (a "Gantry") over `log`, started with what the log says.
fn gantry(mock: &MockBambu, log: &Arc<MemoryJobLog>) -> Fleet {
    let mut registry = DriverRegistry::new();
    registry.register(BambuDriver);
    let fleet = Fleet::new(registry, log.clone());
    let open = log.jobs().into_iter().rev().find(|j| j.ended_at.is_none());
    fleet
        .start(
            "bambu",
            config(mock),
            JobState {
                open,
                awaiting_clear: None,
            },
        )
        .unwrap();
    fleet
}

async fn until(fleet: &Fleet, pred: impl Fn(&PrinterView) -> bool) -> PrinterView {
    (*fleet
        .wait_for("p1s", Duration::from_secs(10), pred)
        .await
        .unwrap())
    .clone()
}

#[tokio::test]
async fn restarting_gantry_mid_print_re_attaches_the_job() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("idle");
    mock.report_fixture("printing");
    let log = Arc::new(MemoryJobLog::new());

    let first = gantry(&mock, &log);
    let job = until(&first, |v| v.job.is_some()).await.job.unwrap();
    first.stop_all().await;

    // Gantry restarts; the printer carries on with the same print.
    let second = gantry(&mock, &log);
    let view = until(&second, |v| v.status.state == Some(PrinterState::Printing)).await;
    assert_eq!(view.job.unwrap().id, job.id, "re-attached");
    mock.report_fixture("printing_delta");
    mock.report_fixture("finished");
    let view = until(&second, |v| {
        v.status.state == Some(PrinterState::AwaitingClear)
    })
    .await;
    assert_eq!(view.awaiting_clear.unwrap().id, job.id);
    let jobs = log.jobs();
    assert_eq!(jobs.len(), 1, "no duplicate: {jobs:?}");
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Succeeded));
}

#[tokio::test]
async fn a_print_that_finished_while_gantry_was_down_gets_its_real_outcome() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("idle");
    mock.report_fixture("printing");
    let log = Arc::new(MemoryJobLog::new());
    let first = gantry(&mock, &log);
    until(&first, |v| v.job.is_some()).await;
    first.stop_all().await;

    // It failed while Gantry was down.
    mock.report_fixture("failed");
    let second = gantry(&mock, &log);
    let view = until(&second, |v| {
        v.status.state == Some(PrinterState::AwaitingClear)
    })
    .await;
    assert_eq!(
        view.awaiting_clear.unwrap().outcome,
        Some(JobOutcome::Failed)
    );
    assert_eq!(log.jobs().len(), 1);

    // After Mark Clear, a third start sees the same FAILED report, but it is
    // already recorded: no new Job.
    second.mark_clear("p1s", false).await.unwrap();
    second.stop_all().await;
    let third = gantry(&mock, &log);
    until(&third, |v| v.status.state == Some(PrinterState::Idle)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(log.jobs().len(), 1, "{:?}", log.jobs());
}

#[tokio::test]
async fn a_different_print_after_a_reconnect_is_a_new_job_and_the_old_is_lost() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("idle");
    mock.report_fixture("printing");
    let log = Arc::new(MemoryJobLog::new());
    let fleet = gantry(&mock, &log);
    let first = until(&fleet, |v| v.job.is_some()).await.job.unwrap();

    mock.go_down();
    until(&fleet, |v| !v.status.connection.is_connected()).await;
    // Something else was started from the printer's screen meanwhile.
    mock.report(&json!({ "print": {
        "command": "push_status", "msg": 1, "gcode_state": "RUNNING",
        "task_id": "4002", "subtask_id": "2002", "subtask_name": "gear",
        "gcode_file": "/sdcard/gear.gcode.3mf", "gcode_start_time": "1762260000",
        "mc_percent": 3
    } }));
    mock.come_back().await;
    let view = until(&fleet, |v| v.job.as_ref().is_some_and(|j| j.id != first.id)).await;
    let jobs = log.jobs();
    assert_eq!(jobs.len(), 2, "{jobs:?}");
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Lost));
    assert!(jobs[1].external);
    assert_eq!(jobs[1].print_file.as_deref(), Some("gear"));
    assert_eq!(view.job.unwrap().id, jobs[1].id);
}
