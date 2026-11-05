//! Sending a Print File to a Bambu printer (PRD §7.3, §10.3): upload over
//! FTPS, then start the Plate over MQTT, against the mock FTPS server, the
//! mock broker and a real Fleet. A failed upload starts nothing and leaves no
//! Job.
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use gantry_core::{
    CommandError, DriverRegistry, Fleet, FleetError, JobRecord, JobStart, JobState, MemoryJobLog,
    PrintUpload, PrinterConfig, PrinterState, Secrets,
};
use gantry_driver_bambu::BambuDriver;
use gantry_mock_bambu::MockBambu;
use gantry_mock_bambu::ftps::MockFtps;
use md5::{Digest, Md5};
use serde_json::Value;

const SERIAL: &str = "UPLSERIAL000777";
const CODE: &str = "24681357";

struct Harness {
    mock: MockBambu,
    ftps: MockFtps,
    fleet: Fleet,
    log: Arc<MemoryJobLog>,
}

/// An idle printer whose FTPS server is the mock, or nothing at all when
/// `ftps_listening` is false.
async fn harness(ftps_listening: bool) -> Harness {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("idle");
    let ftps = MockFtps::start(CODE).await;
    let ftp_port = if ftps_listening {
        ftps.port()
    } else {
        // A port nothing listens on.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let mut registry = DriverRegistry::new();
    registry.register(BambuDriver);
    let log = Arc::new(MemoryJobLog::new());
    let fleet = Fleet::new(registry, log.clone());
    fleet
        .start(
            "bambu",
            PrinterConfig {
                id: "b".into(),
                name: "Bambu".into(),
                settings: serde_json::json!({
                    "host": mock.address().to_string(),
                    "ftp_port": ftp_port,
                })
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
    fleet
        .wait_for("b", Duration::from_secs(10), |v| {
            v.status.state == Some(PrinterState::Idle)
        })
        .await
        .unwrap();
    Harness {
        mock,
        ftps,
        fleet,
        log,
    }
}

const FILE: &[u8] = b"PK\x03\x04 a pretend .gcode.3mf, bytes as they are: \x00\xff\r\n";

fn upload(file_name: &str, plate: Option<u32>) -> (JobStart, PrintUpload) {
    (
        JobStart {
            print_file_id: 3,
            print_file: file_name.into(),
            plate: plate.unwrap_or(1),
        },
        PrintUpload {
            file_name: file_name.into(),
            plate,
            data: Bytes::from_static(FILE),
            estimated_secs: Some(600),
            layers: Some(50),
        },
    )
}

impl Harness {
    async fn send(&self, file_name: &str, plate: Option<u32>) -> Result<JobRecord, FleetError> {
        let (start, upload) = upload(file_name, plate);
        self.fleet.start_job("b", start, upload).await
    }

    /// The start requests the broker has seen.
    fn starts(&self) -> Vec<Value> {
        self.mock
            .requests()
            .into_iter()
            .filter(|r| {
                matches!(
                    r["print"]["command"].as_str(),
                    Some("project_file" | "gcode_file")
                )
            })
            .collect()
    }

    async fn start_request(&self, command: &str) -> Value {
        self.mock
            .wait_for_request(Duration::from_secs(3), |r| r["print"]["command"] == command)
            .await
            .unwrap_or_else(|| panic!("no `{command}` request was recorded"))
    }
}

fn refusal(result: Result<JobRecord, FleetError>) -> String {
    match result {
        Err(FleetError::Command(CommandError::Refused { message })) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn uploads_the_file_then_starts_the_chosen_plate() {
    let h = Arc::new(harness(true).await);
    // Hold the reply after the transfer, to see that nothing starts before it.
    h.ftps.hold();
    let sending = tokio::spawn({
        let h = h.clone();
        async move { h.send("cube.gcode.3mf", Some(2)).await }
    });

    let stored = h
        .ftps
        .wait_for_file("cube.gcode.3mf", Duration::from_secs(10))
        .await
        .expect("the file was uploaded");
    assert_eq!(stored, FILE, "the exact bytes");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        h.starts().is_empty(),
        "nothing starts before the upload ends"
    );

    h.ftps.release();
    let job = sending.await.unwrap().unwrap();
    assert_eq!(job.plate, Some(2));

    let start = h.start_request("project_file").await;
    let body = &start["print"];
    assert_eq!(body["param"], "Metadata/plate_2.gcode");
    assert_eq!(body["url"], "file:///sdcard/cube.gcode.3mf");
    assert_eq!(body["subtask_name"], "cube");
    let md5: String = Md5::digest(FILE)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    assert_eq!(body["md5"], md5);
    // The idle fixture has a filament unit.
    assert_eq!(body["use_ams"], true);
    assert!(body["sequence_id"].is_string());
    assert_eq!(h.starts().len(), 1, "started once");

    assert_eq!(h.ftps.files().len(), 1);
    assert_eq!(
        h.ftps.data_sessions_resumed(),
        [true],
        "the data connection resumed the control connection's TLS session"
    );
    let logins = h.ftps.logins();
    assert_eq!(logins.len(), 1);
    assert!(logins[0].accepted && logins[0].user == "bblp");
    assert_eq!(h.log.jobs().len(), 1, "one Job, waiting for the print");
}

#[tokio::test]
async fn plain_gcode_starts_with_gcode_file() {
    let h = harness(true).await;
    h.send("../folder/benchy.gcode", None).await.unwrap();
    assert_eq!(
        h.ftps.files()[0].0,
        "benchy.gcode",
        "no path reaches the printer"
    );
    let start = h.start_request("gcode_file").await;
    assert_eq!(start["print"]["param"], "/sdcard/benchy.gcode");
    assert_eq!(h.starts().len(), 1);
}

#[tokio::test]
async fn a_refused_access_code_starts_nothing_and_leaves_no_job() {
    let h = harness(true).await;
    h.ftps.refuse_login();
    let message = refusal(h.send("cube.gcode.3mf", Some(1)).await);
    assert!(message.contains("access code for FTPS"), "{message}");
    assert!(!message.contains(CODE), "{message}");
    assert!(h.ftps.files().is_empty());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.starts().is_empty(), "{:?}", h.starts());
    assert!(h.log.jobs().is_empty());
}

#[tokio::test]
async fn a_refused_store_is_reported_with_the_printers_reply() {
    let h = harness(true).await;
    h.ftps.fail_stor(552, "Disk full.");
    let message = refusal(h.send("cube.gcode.3mf", Some(1)).await);
    assert!(message.contains("552 Disk full."), "{message}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.starts().is_empty());
    assert!(h.log.jobs().is_empty());
}

#[tokio::test]
async fn an_ftps_server_that_isnt_there_is_reported() {
    let h = harness(false).await;
    let message = refusal(h.send("cube.gcode.3mf", Some(1)).await);
    assert!(message.contains("could not reach"), "{message}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.starts().is_empty());
    assert!(h.log.jobs().is_empty());
}

#[tokio::test]
async fn a_file_the_printer_cant_start_isnt_sent() {
    let h = harness(true).await;
    let refused = h.send("cube.stl", None).await;
    assert!(
        matches!(
            refused,
            Err(FleetError::Command(CommandError::Invalid { .. }))
        ),
        "{refused:?}"
    );
    assert!(h.ftps.logins().is_empty(), "nothing was uploaded");
    assert!(h.log.jobs().is_empty());
}
