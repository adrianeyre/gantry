//! Starting a Job from Gantry (PRD §7.3): one Job, linked to its Print File
//! and Plate, never onto an occupied bed, and none left behind by a failed
//! upload.
use std::any::Any;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use gantry_core::{
    ActiveJob, Capability, CommandError, Connection, Driver, EventSender, FieldSpec, Filament,
    FilamentSlot, Fleet, FleetError, JobOutcome, JobStart, JobState, MemoryJobLog, PrintUpload,
    PrinterConfig, PrinterHandle, PrinterState, PrinterStatus, PrinterView, Secrets,
};

/// What the scripted Printer does with an upload.
#[derive(Clone, Debug)]
enum Upload {
    /// Starts, and says what it calls the print.
    Keyed(&'static str),
    /// Starts, but only reports the print later.
    Silent,
    /// The transfer fails.
    Fails,
}

/// What each upload sent: the file name, the Plate and the size.
type Uploads = Arc<Mutex<Vec<(String, Option<u32>, usize)>>>;

#[derive(Clone)]
struct Scripted {
    events: Arc<Mutex<Option<EventSender>>>,
    upload: Arc<Mutex<Upload>>,
    uploads: Uploads,
}

struct Handle(Scripted);

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
        *self.events.lock().unwrap() = Some(events);
        Box::new(Handle(self.clone()))
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
    async fn upload_and_start(&self, upload: PrintUpload) -> Result<Option<String>, CommandError> {
        self.0
            .uploads
            .lock()
            .unwrap()
            .push((upload.file_name, upload.plate, upload.data.len()));
        match self.0.upload.lock().unwrap().clone() {
            Upload::Keyed(key) => Ok(Some(key.to_owned())),
            Upload::Silent => Ok(None),
            Upload::Fails => Err(CommandError::Refused {
                message: "the upload failed".into(),
            }),
        }
    }
    fn model(&self) -> Option<String> {
        Some("X1C".into())
    }
    async fn shutdown(&self) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn status(state: PrinterState, job: Option<&str>) -> PrinterStatus {
    let mut status = PrinterStatus::disconnected(gantry_core::DisconnectReason::Stopped);
    status.connection = Connection::Connected;
    status.state = Some(state);
    status.job = job.map(|key| ActiveJob {
        key: key.into(),
        print_file: Some("cube.gcode.3mf".into()),
        plate: Some(2),
        progress: 1.0,
        layer: None,
        total_layers: None,
        remaining_secs: None,
        started_at: None,
    });
    status.capabilities = BTreeSet::from([Capability::UploadAndStart, Capability::Pause]);
    status
}

struct Harness {
    fleet: Fleet,
    log: Arc<MemoryJobLog>,
    driver: Scripted,
}

fn harness(upload: Upload) -> Harness {
    let driver = Scripted {
        events: Arc::default(),
        upload: Arc::new(Mutex::new(upload)),
        uploads: Arc::default(),
    };
    let mut registry = gantry_core::DriverRegistry::new();
    registry.register(driver.clone());
    let log = Arc::new(MemoryJobLog::new());
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
            JobState::default(),
        )
        .unwrap();
    Harness { fleet, log, driver }
}

impl Harness {
    fn events(&self) -> EventSender {
        self.driver.events.lock().unwrap().clone().unwrap()
    }

    async fn until(&self, pred: impl Fn(&PrinterView) -> bool) -> PrinterView {
        (*self
            .fleet
            .wait_for("p", Duration::from_secs(2), pred)
            .await
            .unwrap())
        .clone()
    }

    async fn idle(&self) {
        self.events().status(status(PrinterState::Idle, None));
        self.until(|v| v.status.state == Some(PrinterState::Idle))
            .await;
    }

    async fn start(&self) -> Result<gantry_core::JobRecord, FleetError> {
        self.fleet
            .start_job(
                "p",
                JobStart {
                    print_file_id: 7,
                    print_file: "cube.gcode.3mf".into(),
                    plate: 2,
                },
                PrintUpload {
                    file_name: "cube.gcode.3mf".into(),
                    plate: Some(2),
                    data: Bytes::from_static(b"a print file"),
                    estimated_secs: Some(60),
                    layers: Some(10),
                },
            )
            .await
    }
}

#[tokio::test]
async fn starting_a_plate_creates_one_job_linked_to_its_print_file_and_plate() {
    let h = harness(Upload::Keyed("k1"));
    h.idle().await;
    let job = h.start().await.unwrap();
    assert!(!job.external);
    assert_eq!(job.print_file_id, Some(7));
    assert_eq!(job.plate, Some(2));
    assert_eq!(
        h.driver.uploads.lock().unwrap().as_slice(),
        [("cube.gcode.3mf".to_owned(), Some(2), 12)]
    );
    // The Printer reports the print under the key it gave: still one Job.
    h.events()
        .status(status(PrinterState::Printing, Some("k1")));
    let view = h.until(|v| v.status.job.is_some()).await;
    assert_eq!(view.job.as_ref().unwrap().id, job.id);
    assert_eq!(view.job.as_ref().unwrap().driver_key, "k1");
    let jobs = h.log.jobs();
    assert_eq!(jobs.len(), 1, "{jobs:?}");

    // Its ending is recorded against it, and the Printer awaits clearing.
    h.events().job_ended("k1", JobOutcome::Succeeded);
    h.events().status(status(PrinterState::Idle, None));
    let view = h
        .until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    assert_eq!(view.awaiting_clear.as_ref().unwrap().id, job.id);
    assert_eq!(h.log.jobs().len(), 1);
}

#[tokio::test]
async fn a_print_reported_later_is_adopted_by_the_job_not_made_external() {
    let h = harness(Upload::Silent);
    h.idle().await;
    let job = h.start().await.unwrap();
    assert!(job.is_pending());
    // Still Idle while the Printer gets going: not Lost.
    h.events().status(status(PrinterState::Idle, None));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.fleet.view("p").unwrap().job.as_ref().unwrap().id, job.id);
    h.events()
        .status(status(PrinterState::Printing, Some("later")));
    let view = h
        .until(|v| v.job.as_ref().is_some_and(|j| j.driver_key == "later"))
        .await;
    assert_eq!(view.job.unwrap().id, job.id);
    let jobs = h.log.jobs();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert!(!jobs[0].external);
}

#[tokio::test]
async fn a_failed_upload_is_reported_and_leaves_no_job() {
    let h = harness(Upload::Fails);
    h.idle().await;
    let err = h.start().await.unwrap_err();
    assert!(err.to_string().contains("upload failed"), "{err}");
    assert!(h.log.jobs().is_empty());
    let view = h.until(|v| v.job.is_none()).await;
    assert!(view.job.is_none());
}

#[tokio::test]
async fn a_printer_awaiting_clear_refuses_a_new_job() {
    let h = harness(Upload::Keyed("k1"));
    h.idle().await;
    h.start().await.unwrap();
    h.events()
        .status(status(PrinterState::Printing, Some("k1")));
    h.events().job_ended("k1", JobOutcome::Failed);
    h.events().status(status(PrinterState::Idle, None));
    h.until(|v| v.status.state == Some(PrinterState::AwaitingClear))
        .await;
    let err = h.start().await.unwrap_err();
    assert!(err.to_string().contains("Awaiting Clear"), "{err}");
    assert_eq!(h.log.jobs().len(), 1, "no second Job");
    assert_eq!(h.driver.uploads.lock().unwrap().len(), 1, "nothing sent");

    // After Mark Clear it may start again.
    h.fleet.mark_clear("p", false).await.unwrap();
    h.until(|v| v.status.state == Some(PrinterState::Idle))
        .await;
    h.start().await.unwrap();
}

#[tokio::test]
async fn a_busy_or_printing_printer_refuses_a_new_job() {
    let h = harness(Upload::Keyed("k1"));
    h.events().status(status(PrinterState::Busy, None));
    h.until(|v| v.status.state == Some(PrinterState::Busy))
        .await;
    assert!(h.start().await.is_err());
    assert!(h.log.jobs().is_empty());
}

#[tokio::test]
async fn the_model_comes_from_the_driver() {
    let h = harness(Upload::Silent);
    assert_eq!(h.fleet.view("p").unwrap().model.as_deref(), Some("X1C"));
}

#[tokio::test]
async fn slots_at_or_below_the_threshold_are_marked_low() {
    let h = harness(Upload::Silent);
    let slot = |id: &str, left: Option<u8>| FilamentSlot {
        id: id.into(),
        label: id.into(),
        filament: Some(Filament {
            kind: "PLA".into(),
            colour: "#FFFFFF".into(),
            remaining_percent: left,
        }),
        low: false,
    };
    let mut s = status(PrinterState::Idle, None);
    s.filament_slots = vec![
        slot("a", Some(10)),
        slot("b", Some(15)),
        slot("c", Some(16)),
        slot("d", None),
    ];
    h.events().status(s);
    let view = h.until(|v| v.status.filament_slots.len() == 4).await;
    let low: Vec<bool> = view.status.filament_slots.iter().map(|s| s.low).collect();
    assert_eq!(low, [true, true, false, false], "the default is 15%");

    h.fleet.set_low_filament_threshold(5);
    let view = h
        .until(|v| v.status.filament_slots.iter().all(|s| !s.low))
        .await;
    assert!(view.status.filament_slots.iter().all(|s| !s.low));
    h.fleet.set_low_filament_threshold(50);
    h.until(|v| v.status.filament_slots.iter().filter(|s| s.low).count() == 3)
        .await;
}
