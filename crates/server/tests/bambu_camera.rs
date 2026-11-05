//! The camera relay pulls a Bambu camera only while someone watches (PRD
//! §9.3), against the mock P1/A1 and X1 cameras.
use std::time::Duration;

use gantry_mock_bambu::camera::MockP1Camera;
use gantry_mock_bambu::rtsp::MockX1Camera;
use gantry_server::camera::CameraRelay;

const CODE: &str = "12345678";
const WAIT: Duration = Duration::from_secs(10);

async fn until(pred: impl Fn() -> bool) -> bool {
    tokio::time::timeout(WAIT, async {
        while !pred() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_p1_camera_is_pulled_only_while_watched() {
    let mock = MockP1Camera::start(CODE).await;
    let relay = CameraRelay::new();
    let source = gantry_driver_bambu::camera::p1("127.0.0.1", mock.address().port(), CODE);

    // Nobody watching: nothing connects.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mock.accepted_logins(), 0);

    let mut first = relay.watch("p1", source.clone());
    // A second viewer shares the same upstream pull.
    let mut second = relay.watch("p1", source);
    let frame = tokio::time::timeout(WAIT, first.frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(frame.starts_with(&[0xFF, 0xD8]));
    tokio::time::timeout(WAIT, second.frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mock.open_connections(), 1);
    assert_eq!(relay.active(), 1);

    drop(first);
    drop(second);
    assert!(until(|| relay.active() == 0).await, "the relay stops");
    assert!(
        mock.wait_for_open(WAIT, |n| n == 0).await,
        "and the camera connection closes"
    );
    assert_eq!(mock.accepted_logins(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_x1_camera_is_pulled_only_while_watched() {
    let mock = MockX1Camera::start(CODE).await;
    let relay = CameraRelay::new();
    let source = gantry_driver_bambu::camera::x1("127.0.0.1", mock.address().port(), CODE);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mock.sessions_started(), 0);

    let mut viewer = relay.watch("x1", source);
    let frame = tokio::time::timeout(WAIT, viewer.frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(frame.starts_with(&[0xFF, 0xD8]) && frame.ends_with(&[0xFF, 0xD9]));
    assert_eq!(mock.playing(), 1);

    drop(viewer);
    assert!(until(|| relay.active() == 0).await, "the relay stops");
    assert!(
        mock.wait_for_playing(WAIT, |n| n == 0).await,
        "and the RTSP session ends"
    );
    assert_eq!(mock.sessions_started(), 1);
}
