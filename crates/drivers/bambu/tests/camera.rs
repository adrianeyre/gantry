//! The Bambu cameras against the mock cameras: login, framing, reconnection,
//! decoding, and stopping when nobody is watching.
use std::time::Duration;

use futures::StreamExt;
use gantry_driver_bambu::camera;
use gantry_mock_bambu::camera::MockP1Camera;
use gantry_mock_bambu::rtsp::{Auth, MockX1Camera, Options};

const CODE: &str = "12345678";
const WAIT: Duration = Duration::from_secs(10);

fn is_jpeg(frame: &[u8]) -> bool {
    frame.starts_with(&[0xFF, 0xD8]) && frame.ends_with(&[0xFF, 0xD9])
}

/// The next frame, failing the test if none comes.
async fn next_frame(
    stream: &mut futures::stream::BoxStream<
        'static,
        Result<bytes::Bytes, gantry_core::CameraError>,
    >,
) -> bytes::Bytes {
    tokio::time::timeout(WAIT, stream.next())
        .await
        .expect("a frame in time")
        .expect("the stream is still open")
        .expect("a frame, not an error")
}

#[tokio::test]
async fn p1_logs_in_and_relays_whole_jpegs() {
    let mock = MockP1Camera::start(CODE).await;
    let source = camera::p1("127.0.0.1", mock.address().port(), CODE);
    let mut stream = source.open();
    let mut seen = Vec::new();
    for _ in 0..3 {
        let frame = next_frame(&mut stream).await;
        assert!(is_jpeg(&frame));
        seen.push(frame);
    }
    // The frames are the mock's, in order and unaltered.
    assert_eq!(seen[0], gantry_mock_bambu::camera::test_frame(0));
    assert_eq!(seen[1], gantry_mock_bambu::camera::test_frame(1));
    assert_eq!(mock.accepted_logins(), 1);
    assert_eq!(mock.refused_logins(), 0);
}

#[tokio::test]
async fn p1_stops_pulling_when_the_stream_is_dropped() {
    let mock = MockP1Camera::start(CODE).await;
    let mut stream = camera::p1("127.0.0.1", mock.address().port(), CODE).open();
    next_frame(&mut stream).await;
    assert_eq!(mock.open_connections(), 1);
    drop(stream);
    assert!(
        mock.wait_for_open(WAIT, |n| n == 0).await,
        "the connection closes once nobody watches"
    );
    // And it does not come back.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(mock.open_connections(), 0);
    assert_eq!(mock.accepted_logins(), 1);
}

#[tokio::test]
async fn p1_reconnects_after_a_drop() {
    let mock = MockP1Camera::start(CODE).await;
    let mut stream = camera::p1("127.0.0.1", mock.address().port(), CODE).open();
    next_frame(&mut stream).await;
    mock.drop_connections();
    // Frames keep coming, over a second login.
    let frame = tokio::time::timeout(WAIT, async {
        loop {
            let frame = next_frame(&mut stream).await;
            if mock.accepted_logins() >= 2 {
                return frame;
            }
        }
    })
    .await
    .expect("frames after reconnecting");
    assert!(is_jpeg(&frame));
}

#[tokio::test]
async fn p1_gives_up_on_a_refused_access_code() {
    let mock = MockP1Camera::start(CODE).await;
    let mut stream = camera::p1("127.0.0.1", mock.address().port(), "wrong-code").open();
    let item = tokio::time::timeout(WAIT, stream.next())
        .await
        .expect("an answer in time")
        .expect("an item");
    let error = item.expect_err("the login is refused");
    assert!(error.0.contains("access code"), "{error}");
    assert!(mock.refused_logins() >= 1);
    assert_eq!(mock.accepted_logins(), 0);
    // The stream ends rather than retrying forever.
    assert!(
        tokio::time::timeout(WAIT, stream.next())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn p1_keeps_trying_an_unreachable_camera_without_ending() {
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);
    let mut stream = camera::p1("127.0.0.1", port, CODE).open();
    // Nothing listens: no frame, but no error either (it may come back).
    assert!(
        tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x1_decodes_h264_into_jpegs_with_digest_auth() {
    let mock = MockX1Camera::start(CODE).await;
    let mut stream = camera::x1("127.0.0.1", mock.address().port(), CODE).open();
    let started = std::time::Instant::now();
    let mut frames = Vec::new();
    for _ in 0..4 {
        frames.push(next_frame(&mut stream).await);
    }
    for frame in &frames {
        assert!(is_jpeg(frame));
        assert!(frame.len() > 1000, "a real picture, not an empty one");
    }
    // At most MAX_FPS: 4 frames take at least 3 intervals.
    let interval = Duration::from_millis(1000 / u64::from(camera::MAX_FPS));
    assert!(started.elapsed() >= interval * 3 - Duration::from_millis(50));
    assert_eq!(mock.sessions_started(), 1);
    assert_eq!(mock.refused_logins(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x1_handles_basic_auth_and_in_band_parameter_sets() {
    let mock = MockX1Camera::start_with(
        CODE,
        Options {
            auth: Auth::Basic,
            in_band_parameter_sets: true,
        },
    )
    .await;
    let mut stream = camera::x1("127.0.0.1", mock.address().port(), CODE).open();
    for _ in 0..2 {
        assert!(is_jpeg(&next_frame(&mut stream).await));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x1_refuses_a_wrong_access_code_once() {
    let mock = MockX1Camera::start(CODE).await;
    let mut stream = camera::x1("127.0.0.1", mock.address().port(), "wrong-code").open();
    let item = tokio::time::timeout(WAIT, stream.next())
        .await
        .expect("an answer in time")
        .expect("an item");
    let error = item.expect_err("the login is refused");
    assert!(error.0.contains("access code"), "{error}");
    assert_eq!(mock.refused_logins(), 1, "no retrying a wrong code");
    assert_eq!(mock.sessions_started(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x1_reconnects_after_a_drop_and_stops_when_dropped() {
    let mock = MockX1Camera::start(CODE).await;
    let mut stream = camera::x1("127.0.0.1", mock.address().port(), CODE).open();
    next_frame(&mut stream).await;
    assert_eq!(mock.playing(), 1);

    mock.drop_connections();
    tokio::time::timeout(WAIT, async {
        loop {
            next_frame(&mut stream).await;
            if mock.sessions_started() >= 2 {
                break;
            }
        }
    })
    .await
    .expect("frames after reconnecting");

    drop(stream);
    assert!(
        mock.wait_for_playing(WAIT, |n| n == 0).await,
        "the session ends once nobody watches"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(mock.playing(), 0);
    assert_eq!(mock.sessions_started(), 2);
}
