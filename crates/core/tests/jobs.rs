//! How the Fleet records Jobs and Awaiting Clear, driven by a scripted test
//! double whose events each test sends by hand.
use std::any::Any;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use gantry_core::{
    ActiveJob, Capability, CommandError, Connection, DisconnectReason, Driver, EventSender,
    FieldSpec, Fleet, JobOutcome, JobState, MemoryJobLog, PrinterConfig, PrinterHandle,
    PrinterState, PrinterStatus, PrinterView, Secrets, StartOptions,
};

/// A Driver whose events the test sends.
#[derive(Clone, Default)]
struct Scripted(Arc<Mutex<Option<EventSender>>>);

struct Handle;

impl Driver for Scripted {
    fn id(&self) -> &'static str {
        "scripted"
    }
    fn name(&self) -> &'static str {
        "Scripted"
    }
    fn fields(&self) -> Vec<FieldSpec> {
        vec![]
    }
    fn connect(&self, _: PrinterConfig, events: EventSender) -> Box<dyn PrinterHandle> {
        *self.0.lock().unwrap() = Some(events);
        Box::new(Handle)
    }
}

#[async_trait]
impl PrinterHandle for Handle {
    async fn pause(&self) -> Result<(), CommandError> {
        Ok(())
    }
    async fn resume(&self) -> Result<(), CommandError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), CommandError> {
        Ok(())
    }
    async fn shutdown(&self) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Scripted {
    fn events(&self) -> EventSender {
        self.0.lock().unwrap().clone().unwrap()
    }
}

fn status(state: PrinterState, job: Option<(&str, f64)>) -> PrinterStatus {
    PrinterStatus {
        connection: Connection::Connected,
        state: Some(state),
        job: job.map(|(key, progress)| ActiveJob {
            key: key.into(),
            print_file: Some(format!("{key}.gcode")),
            plate: None,
            progress,
            layer: None,
            total_layers: None,
            remaining_secs: None,
            started_at: None,
        }),
        temperatures: Default::default(),
        fans: vec![],
        filament_slots: vec![],
        faults: vec![],
        capabilities: BTreeSet::from([Capability::Pause]),
        control_withdrawn: None,
        speed_profile: None,
        speed_profiles: Vec::new(),
        lights: Vec::new(),
        position: None,
        loaded_slot: None,
    }
}

struct Harness {
    fleet: Fleet,
    log: Arc<MemoryJobLog>,
    driver: Scripted,
}

fn harness(jobs: JobState) -> Harness {
    harness_with(jobs, StartOptions::default())
}

fn harness_with(jobs: JobState, options: StartOptions) -> Harness {
    let driver = Scripted::default();
    let mut registry = gantry_core::DriverRegistry::new();
    registry.register(driver.clone());
    let log = Arc::new(MemoryJobLog::new());
    let fleet = Fleet::new(registry, log.clone());
    fleet
        .start_with(
            "scripted",
            PrinterConfig {
                id: "p".into(),
                name: "P".into(),
                settings: Default::default(),
                secrets: Secrets::default(),
            },
            jobs,
            options,
        )
        .unwrap();
    Harness { fleet, log, driver }
}

impl Harness {
    async fn until(&self, pred: impl Fn(&PrinterView) -> bool) -> PrinterView {
        (*self
            .fleet
            .wait_for("p", Duration::from_secs(2), pred)
            .await
            .unwrap())
        .clone()
    }
}

#[tokio::test]
async fn a_print_gantry_did_not_start_is_one_external_job() {
    let h = harness(JobState::default());
    let events = h.driver.events();
    events.status(status(PrinterState::Idle, None));
    for progress in [0.0, 10.0, 20.0, 30.0] {
        events.status(status(PrinterState::Printing, Some(("a", progress))));
    }
    let view = h
        .until(|v| v.status.job.as_ref().is_some_and(|j| j.progress == 30.0))
        .await;
    let jobs = h.log.jobs();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert!(jobs[0].external);
    assert_eq!(jobs[0].print_file.as_deref(), Some("a.gcode"));
    assert_eq!(view.job.unwrap().id, jobs[0].id);
}

#[tokio::test]
async fn each_ending_records_its_outcome_and_leaves_awaiting_clear() {
    for outcome in [
        JobOutcome::Succeeded,
        JobOutcome::Failed,
        JobOutcome::Cancelled,
    ] {
        let h = harness(JobState::default());
        let events = h.driver.events();
        events.status(status(PrinterState::Printing, Some(("a", 50.0))));
        events.job_ended("a", outcome);
        events.status(status(PrinterState::Idle, None));
        let view = h
            .until(|v| v.status.state == Some(PrinterState::AwaitingClear))
            .await;
        assert_eq!(view.awaiting_clear.unwrap().outcome, Some(outcome));
        assert!(view.job.is_none());
        let jobs = h.log.jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].outcome, Some(outcome));
        assert!(jobs[0].ended_at.is_some());
    }
}

#[tokio::test]
async fn awaiting_clear_survives_a_restart() {
    let first = harness(JobState::default());
    let events = first.driver.events();
    events.status(status(PrinterState::Printing, Some(("a", 50.0))));
    events.job_ended("a", JobOutcome::Succeeded);
    events.status(status(PrinterState::Idle, None));
    let view = first
        .until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;

    // What a Job log would hand back on the next start.
    let restarted = harness(JobState {
        open: None,
        awaiting_clear: view.awaiting_clear.clone(),
    });
    restarted
        .driver
        .events()
        .status(status(PrinterState::Idle, None));
    restarted
        .until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
}

async fn awaiting_clear_after(outcome: JobOutcome) -> Harness {
    let h = harness(JobState::default());
    let events = h.driver.events();
    events.status(status(PrinterState::Printing, Some(("a", 50.0))));
    events.job_ended("a", outcome);
    events.status(status(PrinterState::Idle, None));
    h.until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    h
}

#[tokio::test]
async fn mark_clear_returns_the_printer_to_idle() {
    let h = awaiting_clear_after(JobOutcome::Succeeded).await;
    let job = h.fleet.mark_clear("p", false).await.unwrap();
    assert_eq!(job.outcome, Some(JobOutcome::Succeeded));
    let view = h
        .until(|v| v.status.state == Some(PrinterState::Idle))
        .await;
    assert!(view.awaiting_clear.is_none());
}

#[tokio::test]
async fn mark_clear_is_only_for_awaiting_clear() {
    let h = harness(JobState::default());
    let events = h.driver.events();
    events.status(status(PrinterState::Idle, None));
    h.until(|v| v.status.state == Some(PrinterState::Idle))
        .await;
    assert!(h.fleet.mark_clear("p", false).await.is_err());
    events.status(status(PrinterState::Printing, Some(("a", 1.0))));
    h.until(|v| v.status.state == Some(PrinterState::Printing))
        .await;
    assert!(h.fleet.mark_clear("p", false).await.is_err());
    // Clearing twice: the second finds nothing to clear.
    let h = awaiting_clear_after(JobOutcome::Failed).await;
    h.fleet.mark_clear("p", false).await.unwrap();
    assert!(h.fleet.mark_clear("p", false).await.is_err());
}

#[tokio::test]
async fn only_succeeded_can_be_overruled_and_only_to_failed() {
    let h = awaiting_clear_after(JobOutcome::Succeeded).await;
    let job = h.fleet.mark_clear("p", true).await.unwrap();
    assert_eq!(job.outcome, Some(JobOutcome::Failed));
    assert!(job.overruled);
    assert_eq!(h.log.jobs()[0].outcome, Some(JobOutcome::Failed));

    for outcome in [JobOutcome::Failed, JobOutcome::Cancelled] {
        let h = awaiting_clear_after(outcome).await;
        assert!(h.fleet.mark_clear("p", true).await.is_err());
        // Still Awaiting Clear, and the Outcome is untouched.
        assert_eq!(
            h.fleet.view("p").unwrap().status.state,
            Some(PrinterState::AwaitingClear)
        );
        assert_eq!(h.log.jobs()[0].outcome, Some(outcome));
        assert!(!h.log.jobs()[0].overruled);
    }
}

#[tokio::test]
async fn a_new_print_clears_the_bed_and_starts_another_job() {
    let h = harness(JobState::default());
    let events = h.driver.events();
    events.status(status(PrinterState::Printing, Some(("a", 50.0))));
    events.job_ended("a", JobOutcome::Succeeded);
    events.status(status(PrinterState::Idle, None));
    h.until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    events.status(status(PrinterState::Printing, Some(("b", 1.0))));
    let view = h
        .until(|v| v.status.state == Some(PrinterState::Printing))
        .await;
    assert!(view.awaiting_clear.is_none());
    assert_eq!(h.log.jobs().len(), 2);
}

#[tokio::test]
async fn a_job_that_ended_unseen_is_still_recorded() {
    let h = harness(JobState::default());
    let events = h.driver.events();
    events.status(status(PrinterState::Idle, None));
    events.job_ended("ghost", JobOutcome::Failed);
    events.status(status(PrinterState::Idle, None));
    let view = h
        .until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    let jobs = h.log.jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Failed));
    assert_eq!(view.awaiting_clear.unwrap().driver_key, "ghost");
    // The same ending reported again is not a second Job.
    events.job_ended("ghost", JobOutcome::Failed);
    events.status(status(PrinterState::Idle, None));
    h.until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    assert_eq!(h.log.jobs().len(), 1);
}

#[tokio::test]
async fn a_disconnection_keeps_the_job_open() {
    let h = harness(JobState::default());
    let events = h.driver.events();
    events.status(status(PrinterState::Printing, Some(("a", 50.0))));
    events.status(PrinterStatus::disconnected(DisconnectReason::Unreachable));
    let view = h
        .until(|v| matches!(v.status.connection, Connection::Disconnected { .. }))
        .await;
    assert!(view.job.is_some(), "the Job is still open");
    events.status(status(PrinterState::Printing, Some(("a", 60.0))));
    let view = h.until(|v| v.status.connection.is_connected()).await;
    assert_eq!(h.log.jobs().len(), 1);
    assert!(view.job.is_some());
}

/// A Fleet over an existing Job log, started with what that log says: a
/// restarted Gantry.
fn restarted(log: Arc<MemoryJobLog>) -> Harness {
    let open = log.jobs().into_iter().rev().find(|j| j.ended_at.is_none());
    let driver = Scripted::default();
    let mut registry = gantry_core::DriverRegistry::new();
    registry.register(driver.clone());
    let fleet = Fleet::new(registry, log.clone());
    fleet
        .start(
            "scripted",
            PrinterConfig {
                id: "p".into(),
                name: "P".into(),
                settings: Default::default(),
                secrets: Secrets::default(),
            },
            JobState {
                open,
                awaiting_clear: None,
            },
        )
        .unwrap();
    Harness { fleet, log, driver }
}

fn printing_with(key: &str, file: &str, started: Option<time::OffsetDateTime>) -> PrinterStatus {
    let mut s = status(PrinterState::Printing, Some((key, 60.0)));
    let job = s.job.as_mut().unwrap();
    job.print_file = Some(file.into());
    job.started_at = started;
    s
}

#[tokio::test]
async fn a_restart_mid_print_re_attaches_the_job() {
    let first = harness(JobState::default());
    first
        .driver
        .events()
        .status(status(PrinterState::Printing, Some(("a", 40.0))));
    let job = first.until(|v| v.job.is_some()).await.job.unwrap();

    let second = restarted(first.log.clone());
    let events = second.driver.events();
    events.status(status(PrinterState::Printing, Some(("a", 60.0))));
    let view = second
        .until(|v| v.status.job.as_ref().is_some_and(|j| j.progress == 60.0))
        .await;
    assert_eq!(view.job.unwrap().id, job.id, "the same Job, re-attached");
    events.job_ended("a", JobOutcome::Succeeded);
    events.status(status(PrinterState::Idle, None));
    second
        .until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    let jobs = second.log.jobs();
    assert_eq!(jobs.len(), 1, "no duplicate: {jobs:?}");
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Succeeded));
}

#[tokio::test]
async fn the_same_file_and_start_time_re_attach_under_a_new_key() {
    let started = time::OffsetDateTime::now_utc() - time::Duration::minutes(30);
    let first = harness(JobState::default());
    first
        .driver
        .events()
        .status(printing_with("old-key", "cube.gcode", Some(started)));
    let job = first.until(|v| v.job.is_some()).await.job.unwrap();
    assert_eq!(job.started_at, started, "the printer's start time is kept");

    // After the restart the Driver knows the print by another key, but the
    // file and start time match.
    let second = restarted(first.log.clone());
    let events = second.driver.events();
    events.status(printing_with(
        "new-key",
        "cube.gcode",
        Some(started + time::Duration::seconds(1)),
    ));
    let view = second
        .until(|v| v.job.as_ref().is_some_and(|j| j.driver_key == "new-key"))
        .await;
    assert_eq!(view.job.unwrap().id, job.id);
    events.job_ended("new-key", JobOutcome::Cancelled);
    events.status(status(PrinterState::Idle, None));
    second
        .until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    let jobs = second.log.jobs();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Cancelled));

    // The same file started much later is a different print.
    let third = harness(JobState::default());
    third
        .driver
        .events()
        .status(printing_with("k1", "cube.gcode", Some(started)));
    third.until(|v| v.job.is_some()).await;
    let fourth = restarted(third.log.clone());
    fourth.driver.events().status(printing_with(
        "k2",
        "cube.gcode",
        Some(started + time::Duration::hours(2)),
    ));
    fourth
        .until(|v| v.job.as_ref().is_some_and(|j| j.driver_key == "k2"))
        .await;
    let jobs = fourth.log.jobs();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Lost));
}

#[tokio::test]
async fn a_different_print_after_a_reconnect_closes_the_open_job_as_lost() {
    let h = harness(JobState::default());
    let events = h.driver.events();
    events.status(status(PrinterState::Printing, Some(("a", 50.0))));
    h.until(|v| v.job.is_some()).await;
    events.status(PrinterStatus::disconnected(DisconnectReason::Unreachable));
    h.until(|v| !v.status.connection.is_connected()).await;
    events.status(status(PrinterState::Printing, Some(("b", 5.0))));
    let view = h
        .until(|v| v.job.as_ref().is_some_and(|j| j.driver_key == "b"))
        .await;
    let jobs = h.log.jobs();
    assert_eq!(jobs.len(), 2, "{jobs:?}");
    assert_eq!(jobs[0].driver_key, "a");
    assert_eq!(jobs[0].outcome, Some(JobOutcome::Lost));
    assert!(jobs[1].external);
    assert_eq!(view.job.unwrap().id, jobs[1].id);
}

#[tokio::test]
async fn an_open_job_the_printer_no_longer_runs_is_lost() {
    let first = harness(JobState::default());
    first
        .driver
        .events()
        .status(status(PrinterState::Printing, Some(("a", 40.0))));
    first.until(|v| v.job.is_some()).await;

    // Back after a restart: idle, and no word of how the print ended.
    let second = restarted(first.log.clone());
    second
        .driver
        .events()
        .status(status(PrinterState::Idle, None));
    let view = second
        .until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    assert_eq!(view.awaiting_clear.unwrap().outcome, Some(JobOutcome::Lost));
    assert_eq!(second.log.jobs().len(), 1);
}

#[tokio::test]
async fn an_ending_reported_again_after_mark_clear_is_not_a_new_job() {
    let h = awaiting_clear_after(JobOutcome::Succeeded).await;
    h.fleet.mark_clear("p", false).await.unwrap();
    h.until(|v| v.status.state == Some(PrinterState::Idle))
        .await;
    // A Driver that replays the last ending after a reconnect.
    let events = h.driver.events();
    events.job_ended("a", JobOutcome::Succeeded);
    events.status(status(PrinterState::Idle, None));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let view = h.until(|_| true).await;
    assert_eq!(view.status.state, Some(PrinterState::Idle));
    assert_eq!(h.log.jobs().len(), 1);
}

#[tokio::test]
async fn an_ending_that_happened_while_gantry_was_down_gets_its_real_outcome() {
    let first = harness(JobState::default());
    first
        .driver
        .events()
        .status(status(PrinterState::Printing, Some(("a", 90.0))));
    first.until(|v| v.job.is_some()).await;
    let second = restarted(first.log.clone());
    let events = second.driver.events();
    // The Driver says how it ended, then that it's idle.
    events.job_ended("a", JobOutcome::Failed);
    events.status(status(PrinterState::Idle, None));
    let view = second
        .until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    assert_eq!(
        view.awaiting_clear.unwrap().outcome,
        Some(JobOutcome::Failed)
    );
    assert_eq!(second.log.jobs().len(), 1);
}

fn auto_clear() -> StartOptions {
    StartOptions {
        auto_clear: true,
        ..StartOptions::default()
    }
}

#[tokio::test]
async fn auto_clear_is_off_by_default() {
    let h = harness(JobState::default());
    assert!(!h.fleet.view("p").unwrap().auto_clear);
    assert!(!StartOptions::default().auto_clear);
}

#[tokio::test]
async fn with_auto_clear_a_succeeded_job_skips_awaiting_clear() {
    let h = harness_with(JobState::default(), auto_clear());
    let events = h.driver.events();
    events.status(status(PrinterState::Printing, Some(("a", 50.0))));
    events.job_ended("a", JobOutcome::Succeeded);
    events.status(status(PrinterState::Idle, None));
    let view = h
        .until(|v| v.last_ended.is_some() && v.status.state == Some(PrinterState::Idle))
        .await;
    assert!(view.awaiting_clear.is_none());
    assert_eq!(
        view.last_ended.unwrap().outcome,
        Some(JobOutcome::Succeeded)
    );
    // Nothing to Mark Clear.
    assert!(h.fleet.mark_clear("p", false).await.is_err());
}

#[tokio::test]
async fn with_auto_clear_failed_and_cancelled_jobs_still_await_clear() {
    for outcome in [JobOutcome::Failed, JobOutcome::Cancelled, JobOutcome::Lost] {
        let h = harness_with(JobState::default(), auto_clear());
        let events = h.driver.events();
        events.status(status(PrinterState::Printing, Some(("a", 50.0))));
        events.job_ended("a", outcome);
        events.status(status(PrinterState::Idle, None));
        let view = h
            .until(|v| v.status.state == Some(PrinterState::AwaitingClear))
            .await;
        assert_eq!(view.awaiting_clear.unwrap().outcome, Some(outcome));
        assert_eq!(view.last_ended.unwrap().outcome, Some(outcome));
    }
}

#[tokio::test]
async fn auto_clear_can_be_turned_on_while_running() {
    let h = harness(JobState::default());
    h.fleet.set_auto_clear("p", true).unwrap();
    h.until(|v| v.auto_clear).await;
    let events = h.driver.events();
    events.status(status(PrinterState::Printing, Some(("a", 50.0))));
    events.job_ended("a", JobOutcome::Succeeded);
    events.status(status(PrinterState::Idle, None));
    let view = h
        .until(|v| v.last_ended.is_some() && v.status.state == Some(PrinterState::Idle))
        .await;
    assert!(view.awaiting_clear.is_none());
}
