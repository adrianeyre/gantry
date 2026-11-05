//! The real-printer probe (#27, #28), against the mock broker and camera.
use std::time::Duration;

use gantry_core::PrinterState;
use gantry_driver_bambu::model::Model;
use gantry_driver_bambu::probe::{ProbeError, ProbeOptions, probe};
use gantry_mock_bambu::MockBambu;
use gantry_mock_bambu::camera::MockP1Camera;

const SERIAL: &str = "01PPROBESERIAL1";
const CODE: &str = "11223344";

fn options(mock: &MockBambu, out: &std::path::Path) -> ProbeOptions {
    ProbeOptions {
        host: mock.address().ip().to_string(),
        port: mock.address().port(),
        serial: SERIAL.into(),
        access_code: CODE.into(),
        model: None,
        listen: Duration::from_millis(1500),
        camera: None,
        camera_port: None,
        out: Some(out.to_owned()),
    }
}

#[tokio::test]
async fn records_model_firmware_status_and_redacted_payloads() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("idle");
    mock.report_fixture("printing");
    let dir = tempfile::tempdir().unwrap();

    let report = probe(&options(&mock, dir.path())).await.unwrap();
    assert_eq!(report.model, Some(Model::P1S), "from the serial prefix");
    assert!(
        report
            .firmware
            .contains(&("ota".to_owned(), gantry_mock_bambu::FIRMWARE.to_owned()))
    );
    assert!(report.reports >= 1);
    assert!(report.first_status_after.unwrap() < Duration::from_secs(5));
    assert_eq!(report.status.unwrap().state, Some(PrinterState::Printing));
    // Every payload saved, none with a credential in it.
    assert!(report.saved.len() >= 2);
    for path in &report.saved {
        let text = std::fs::read_to_string(path).unwrap();
        assert!(
            !text.contains(SERIAL),
            "{} holds the serial",
            path.display()
        );
        assert!(
            !text.contains(CODE),
            "{} holds the access code",
            path.display()
        );
    }
    // It only asked; it changed nothing.
    let commands: Vec<String> = mock
        .requests()
        .iter()
        .filter_map(|r| r.get("print").map(|p| p["command"].to_string()))
        .collect();
    assert!(commands.is_empty(), "{commands:?}");
}

#[tokio::test]
async fn reads_the_camera_and_lets_it_go() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("idle");
    let camera = MockP1Camera::start(CODE).await;
    let dir = tempfile::tempdir().unwrap();
    let mut options = options(&mock, dir.path());
    options.listen = Duration::from_millis(300);
    options.camera = Some(Duration::from_millis(1500));
    options.camera_port = Some(camera.address().port());

    let report = probe(&options).await.unwrap();
    let check = report.camera.unwrap();
    assert!(check.frames > 0, "{check:?}");
    assert!(check.first_frame_bytes.unwrap() > 0);
    assert_eq!(check.error, None);
    let frame = std::fs::read(dir.path().join("camera-first-frame.jpg")).unwrap();
    assert_eq!(&frame[..2], &[0xFF, 0xD8], "a JPEG");
    // Nobody is watching any more, so the camera connection closes.
    assert!(
        camera
            .wait_for_open(Duration::from_secs(5), |open| open == 0)
            .await
    );
}

#[tokio::test]
async fn a_wrong_access_code_is_reported() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    let dir = tempfile::tempdir().unwrap();
    let mut options = options(&mock, dir.path());
    options.access_code = "00000000".into();
    let err = probe(&options).await.unwrap_err();
    assert!(
        matches!(&err, ProbeError::Connect(detail) if detail.contains("access code")),
        "{err}"
    );
}
