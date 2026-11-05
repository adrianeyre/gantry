//! The camera relay (PRD §5.3, §7.1, §9.3).
//!
//! The server pulls a Printer's camera from its Driver and relays it to
//! browsers as MJPEG over HTTP. A camera is pulled only while someone is
//! watching. Every viewer of one Printer shares a single upstream pull, which
//! stops once the last viewer leaves.
//!
//! A viewer can ask for a lower [`Quality`], and remote viewers get one by
//! default, to fit home upload bandwidth. Each lower level of a Printer has
//! at most one transcoder, shared by everyone watching at that level. It
//! takes frames from the upstream pull, drops them to meet the level's frame
//! rate, and shrinks the rest. It stops once its last viewer leaves.
mod transcode;

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::StreamExt;
use gantry_core::{CameraSource, Capability, FieldError};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::time::Instant;

pub use transcode::Quality;

use crate::AppState;
use crate::api::ApiError;
use crate::client::ClientInfo;

/// The multipart boundary between frames.
const BOUNDARY: &str = "gantry-frame";

/// The response header naming the quality a viewer is sent.
pub const QUALITY_HEADER: &str = "x-gantry-camera-quality";

struct Relay {
    frames: broadcast::Sender<Bytes>,
    /// The newest frame, so a new viewer sees a picture at once.
    latest: Mutex<Option<Bytes>>,
}

impl Relay {
    fn new() -> (Arc<Self>, broadcast::Receiver<Bytes>) {
        let (frames, receiver) = broadcast::channel(4);
        let relay = Arc::new(Self {
            frames,
            latest: Mutex::new(None),
        });
        (relay, receiver)
    }

    fn viewer(&self) -> Viewer {
        Viewer {
            first: self.latest.lock().expect("latest lock").clone(),
            frames: self.frames.subscribe(),
        }
    }

    /// Sends `frame` to every viewer. False if there were none.
    fn publish(&self, frame: Bytes) -> bool {
        *self.latest.lock().expect("latest lock") = Some(frame.clone());
        self.frames.send(frame).is_ok()
    }
}

/// What is being relayed: a Printer's upstream pull at [`Quality::Full`], or
/// one of its transcoders.
type Key = (String, Quality);
type Relays = Arc<Mutex<HashMap<Key, Arc<Relay>>>>;

/// Every camera being relayed right now, by Printer and quality.
#[derive(Default, Clone)]
pub struct CameraRelay {
    relays: Relays,
    /// How many frames the transcoders have taken on, for tests.
    transcoded: Arc<AtomicU64>,
}

/// One viewer's feed. Dropping it is how a viewer leaves.
pub struct Viewer {
    pub first: Option<Bytes>,
    pub frames: broadcast::Receiver<Bytes>,
}

impl CameraRelay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts watching `printer`'s camera at full quality, pulling from
    /// `source` only if no one else already is.
    pub fn watch(&self, printer: &str, source: Arc<dyn CameraSource>) -> Viewer {
        self.watch_at(printer, source, Quality::Full)
    }

    /// Starts watching `printer`'s camera at `quality`. The upstream pull,
    /// and the transcoder for a lower level, start only if not yet running.
    pub fn watch_at(
        &self,
        printer: &str,
        source: Arc<dyn CameraSource>,
        quality: Quality,
    ) -> Viewer {
        let mut relays = self.relays.lock().expect("relay lock");
        let key = (printer.to_owned(), quality);
        if let Some(relay) = relays.get(&key) {
            return relay.viewer();
        }
        let (relay, receiver) = Relay::new();
        relays.insert(key.clone(), relay.clone());
        if quality == Quality::Full {
            drop(relays);
            tokio::spawn(pull(self.relays.clone(), key, relay, source));
        } else {
            // Join or start the upstream pull under the same lock, so it
            // can't stop between being found and being joined.
            let upstream_key = (printer.to_owned(), Quality::Full);
            let upstream = if let Some(upstream) = relays.get(&upstream_key) {
                upstream.frames.subscribe()
            } else {
                let (upstream, receiver) = Relay::new();
                relays.insert(upstream_key.clone(), upstream.clone());
                tokio::spawn(pull(self.relays.clone(), upstream_key, upstream, source));
                receiver
            };
            drop(relays);
            tokio::spawn(transcoder(
                self.relays.clone(),
                key,
                relay,
                upstream,
                self.transcoded.clone(),
            ));
        }
        Viewer {
            first: None,
            frames: receiver,
        }
    }

    /// How many cameras are being pulled right now.
    pub fn active(&self) -> usize {
        self.count(|q| q == Quality::Full)
    }

    /// How many transcoders are running right now, over every Printer.
    pub fn transcoders(&self) -> usize {
        self.count(|q| q != Quality::Full)
    }

    /// How many frames the transcoders have taken on since the start. Each
    /// frame is taken on once per level, however many watch it.
    pub fn transcoded_frames(&self) -> u64 {
        self.transcoded.load(Ordering::SeqCst)
    }

    fn count(&self, which: impl Fn(Quality) -> bool) -> usize {
        self.relays
            .lock()
            .expect("relay lock")
            .keys()
            .filter(|(_, q)| which(*q))
            .count()
    }
}

/// Forgets `relay` if nobody is watching it. Under the lock, so no viewer can
/// join between the check and the removal.
fn forget_if_unwatched(relays: &Relays, key: &Key, relay: &Relay) -> bool {
    let mut relays = relays.lock().expect("relay lock");
    if relay.frames.receiver_count() == 0 {
        relays.remove(key);
        return true;
    }
    false
}

/// Forgets `relay` once it has ended, unless it was already replaced.
fn forget(relays: &Relays, key: &Key, relay: &Arc<Relay>) {
    let mut relays = relays.lock().expect("relay lock");
    if relays
        .get(key)
        .is_some_and(|current| Arc::ptr_eq(current, relay))
    {
        relays.remove(key);
    }
}

/// Pulls one camera until nobody is watching, then stops and forgets it.
async fn pull(relays: Relays, key: Key, relay: Arc<Relay>, source: Arc<dyn CameraSource>) {
    let printer = &key.0;
    let mut upstream = source.open();
    tracing::debug!(printer = %printer, "camera relay started");
    loop {
        let frame = match upstream.next().await {
            Some(Ok(frame)) => frame,
            Some(Err(e)) => {
                tracing::warn!(printer = %printer, error = %e, "camera stream failed");
                break;
            }
            None => break,
        };
        if !relay.publish(frame) && forget_if_unwatched(&relays, &key, &relay) {
            tracing::debug!(printer = %printer, "camera relay stopped: no viewers");
            return;
        }
    }
    forget(&relays, &key, &relay);
}

/// Sends one Printer's camera at a lower quality until nobody watches at
/// that level. While it runs it is a viewer of the upstream pull.
///
/// It never queues. After each frame it waits out the level's frame gap,
/// then takes the newest frame and skips the rest, including any that came
/// while it was busy. A frame that won't decode is skipped.
async fn transcoder(
    relays: Relays,
    key: Key,
    relay: Arc<Relay>,
    mut upstream: broadcast::Receiver<Bytes>,
    transcoded: Arc<AtomicU64>,
) {
    let (printer, quality) = (&key.0, key.1);
    let gap = quality.min_interval().unwrap_or_default();
    let mut next_at = Instant::now();
    tracing::debug!(printer = %printer, quality = quality.as_str(), "camera transcoder started");
    'frames: loop {
        let mut frame = match upstream.recv().await {
            Ok(frame) => frame,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        tokio::time::sleep_until(next_at).await;
        loop {
            match upstream.try_recv() {
                Ok(newer) => frame = newer,
                Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Closed) => break 'frames,
            }
        }
        next_at = Instant::now() + gap;
        if relay.frames.receiver_count() == 0 && forget_if_unwatched(&relays, &key, &relay) {
            break;
        }
        transcoded.fetch_add(1, Ordering::SeqCst);
        // Decoding and encoding are CPU work: keep them off the runtime.
        let result = tokio::task::spawn_blocking(move || transcode::transcode(&frame, quality))
            .await
            .expect("transcoding doesn't panic");
        let small = match result {
            Ok(small) => small,
            Err(e) => {
                tracing::debug!(printer = %printer, error = %e, "camera frame skipped");
                continue;
            }
        };
        if !relay.publish(small) && forget_if_unwatched(&relays, &key, &relay) {
            break;
        }
    }
    tracing::debug!(printer = %printer, quality = quality.as_str(), "camera transcoder stopped");
    forget(&relays, &key, &relay);
}

fn part(frame: &Bytes) -> Bytes {
    let mut part = format!(
        "--{BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        frame.len()
    )
    .into_bytes();
    part.extend_from_slice(frame);
    part.extend_from_slice(b"\r\n");
    Bytes::from(part)
}

#[derive(Deserialize)]
pub struct CameraQuery {
    quality: Option<String>,
}

/// The quality a viewer asked for or, if they didn't, the default for where
/// they are. An unknown level is refused, not guessed at.
fn choose(asked: Option<&str>, client: Option<&ClientInfo>) -> Result<Quality, ApiError> {
    match asked {
        None | Some("") => Ok(Quality::default_for(
            client.is_some_and(ClientInfo::is_remote),
        )),
        Some(value) => Quality::parse(value).ok_or_else(|| {
            ApiError::Invalid(vec![FieldError {
                field: "quality".into(),
                message: "must be full, medium or low".into(),
            }])
        }),
    }
}

/// `GET /api/printers/{id}/camera`: the Printer's camera as MJPEG.
///
/// `?quality=full|medium|low` picks the level. Without it, remote clients
/// get `medium` and local ones `full`. The `X-Gantry-Camera-Quality`
/// response header names the level sent.
pub async fn stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<CameraQuery>,
    client: Option<Extension<ClientInfo>>,
) -> Result<Response, ApiError> {
    let quality = choose(query.quality.as_deref(), client.as_deref())?;
    let view = state.fleet().view(&id)?;
    let source = state
        .fleet()
        .handle(&id)?
        .camera()
        .filter(|_| view.status.has(Capability::Camera))
        .ok_or_else(|| ApiError::NotFound("this Printer has no camera".into()))?;
    let Viewer { first, frames } = state.cameras().watch_at(&id, source, quality);

    let body = futures::stream::unfold(
        (first, frames, state),
        |(first, mut frames, state)| async move {
            if let Some(frame) = first {
                return Some((Ok::<_, Infallible>(part(&frame)), (None, frames, state)));
            }
            loop {
                tokio::select! {
                    received = frames.recv() => match received {
                        Ok(frame) => return Some((Ok(part(&frame)), (None, frames, state))),
                        // A slow viewer skips frames rather than holding others up.
                        Err(RecvError::Lagged(_)) => {}
                        Err(RecvError::Closed) => return None,
                    },
                    // The server is shutting down: end the stream.
                    () = state.stopping() => return None,
                }
            }
        },
    );
    let mut response = Body::from_stream(body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&format!("multipart/x-mixed-replace; boundary={BOUNDARY}"))
            .expect("a valid header"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(QUALITY_HEADER, HeaderValue::from_static(quality.as_str()));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use futures::stream::BoxStream;
    use gantry_core::CameraError;

    use super::transcode::{dimensions, tests::jpeg};
    use super::*;

    /// A test double: a camera that counts how often it is opened and how
    /// many streams are live. It sends `frames` in turn, one every `every`.
    struct CountingCamera {
        opened: AtomicUsize,
        live: Arc<AtomicUsize>,
        frames: Vec<Bytes>,
        every: Duration,
    }

    impl Default for CountingCamera {
        /// Frames that aren't JPEGs, 50 a second.
        fn default() -> Self {
            Self::sending((0..10).map(|n| Bytes::from(format!("frame {n}"))).collect())
        }
    }

    impl CountingCamera {
        fn sending(frames: Vec<Bytes>) -> Self {
            Self {
                opened: AtomicUsize::new(0),
                live: Arc::default(),
                frames,
                every: Duration::from_millis(20),
            }
        }

        /// A 1280 by 720 camera at 20 frames a second.
        fn hd() -> Self {
            Self {
                every: Duration::from_millis(50),
                ..Self::sending(vec![jpeg(1280, 720), jpeg(1280, 720 - 16)])
            }
        }
    }

    struct Live(Arc<AtomicUsize>);

    impl Drop for Live {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl CameraSource for CountingCamera {
        fn open(&self) -> BoxStream<'static, Result<Bytes, CameraError>> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            self.live.fetch_add(1, Ordering::SeqCst);
            let live = Live(self.live.clone());
            let (frames, every) = (self.frames.clone(), self.every);
            futures::stream::unfold((0usize, live), move |(n, live)| {
                let frame = frames[n % frames.len()].clone();
                async move {
                    tokio::time::sleep(every).await;
                    Some((Ok(frame), (n + 1, live)))
                }
            })
            .boxed()
        }
    }

    async fn eventually(what: &str, check: impl Fn() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    /// A viewer's next frame, skipping any it fell behind on.
    async fn next(frames: &mut broadcast::Receiver<Bytes>) -> Bytes {
        loop {
            match frames.recv().await {
                Ok(frame) => return frame,
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("the feed ended"),
            }
        }
    }

    /// Every frame a viewer is sent over `window`.
    async fn collect(frames: &mut broadcast::Receiver<Bytes>, window: Duration) -> Vec<Bytes> {
        let mut got = Vec::new();
        let _ = tokio::time::timeout(window, async {
            loop {
                match frames.recv().await {
                    Ok(frame) => got.push(frame),
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                }
            }
        })
        .await;
        got
    }

    #[tokio::test]
    async fn nothing_is_pulled_while_no_one_watches() {
        let relay = CameraRelay::new();
        let camera = Arc::new(CountingCamera::default());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(camera.opened.load(Ordering::SeqCst), 0);
        assert_eq!(relay.active(), 0);

        let viewer = relay.watch("p", camera.clone());
        let mut frames = viewer.frames;
        frames.recv().await.unwrap();
        assert_eq!(camera.live.load(Ordering::SeqCst), 1);

        // The last viewer leaves: the upstream pull stops.
        drop(frames);
        eventually("the pull to stop", || {
            camera.live.load(Ordering::SeqCst) == 0
        })
        .await;
        assert_eq!(relay.active(), 0);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(camera.opened.load(Ordering::SeqCst), 1, "not reopened");
    }

    #[tokio::test]
    async fn several_viewers_share_one_upstream_pull() {
        let relay = CameraRelay::new();
        let camera = Arc::new(CountingCamera::default());
        let mut a = relay.watch("p", camera.clone()).frames;
        let mut b = relay.watch("p", camera.clone()).frames;
        let mut c = relay.watch("p", camera.clone()).frames;
        for viewer in [&mut a, &mut b, &mut c] {
            viewer.recv().await.unwrap();
        }
        assert_eq!(camera.opened.load(Ordering::SeqCst), 1);
        assert_eq!(camera.live.load(Ordering::SeqCst), 1);

        // One leaving doesn't stop the others.
        drop(a);
        tokio::time::sleep(Duration::from_millis(100)).await;
        b.recv().await.unwrap();
        assert_eq!(camera.live.load(Ordering::SeqCst), 1);
        drop((b, c));
        eventually("the pull to stop", || {
            camera.live.load(Ordering::SeqCst) == 0
        })
        .await;

        // A viewer arriving later starts a fresh pull.
        let mut d = relay.watch("p", camera.clone()).frames;
        d.recv().await.unwrap();
        assert_eq!(camera.opened.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_late_viewer_sees_the_latest_frame_at_once() {
        let relay = CameraRelay::new();
        let camera = Arc::new(CountingCamera::default());
        let mut first = relay.watch("p", camera.clone()).frames;
        first.recv().await.unwrap();
        let late = relay.watch("p", camera.clone());
        assert!(late.first.is_some());
    }

    #[tokio::test]
    async fn full_quality_is_the_cameras_own_frames() {
        let relay = CameraRelay::new();
        let camera = Arc::new(CountingCamera::hd());
        let mut viewer = relay.watch_at("p", camera.clone(), Quality::Full).frames;
        let frames = collect(&mut viewer, Duration::from_millis(500)).await;
        assert!(
            frames.len() >= 5,
            "not frame-rate limited: {}",
            frames.len()
        );
        for frame in &frames {
            assert!(camera.frames.contains(frame), "byte for byte");
        }
        assert_eq!(relay.transcoders(), 0);
        assert_eq!(relay.transcoded_frames(), 0);
    }

    #[tokio::test]
    async fn medium_is_smaller_and_slower() {
        let relay = CameraRelay::new();
        let camera = Arc::new(CountingCamera::hd());
        let mut viewer = relay.watch_at("p", camera.clone(), Quality::Medium).frames;
        // Wait for the first, then count over a second and a half.
        next(&mut viewer).await;
        let window = Duration::from_millis(1500);
        let frames = collect(&mut viewer, window).await;
        // The camera sent 30 in that time; 5 a second allows 7.5.
        assert!(
            (3..=9).contains(&frames.len()),
            "{} frames in 1.5 s",
            frames.len()
        );
        for frame in &frames {
            let (width, height) = dimensions(frame).expect("a JPEG");
            assert!(width <= 960, "{width} wide");
            assert!(height < 720);
        }
        assert_eq!(dimensions(&frames[0]).unwrap().0, 960);
    }

    #[tokio::test]
    async fn low_is_smaller_still() {
        let relay = CameraRelay::new();
        let camera = Arc::new(CountingCamera::hd());
        let mut viewer = relay.watch_at("p", camera.clone(), Quality::Low).frames;
        next(&mut viewer).await;
        let frames = collect(&mut viewer, Duration::from_millis(1500)).await;
        // 2 a second allows 3.
        assert!(
            (1..=4).contains(&frames.len()),
            "{} frames in 1.5 s",
            frames.len()
        );
        assert!(frames.iter().all(|f| dimensions(f).unwrap().0 == 480));
    }

    #[tokio::test]
    async fn viewers_at_one_level_share_one_transcoder() {
        let relay = CameraRelay::new();
        let camera = Arc::new(CountingCamera::hd());
        let mut viewers: Vec<_> = (0..3)
            .map(|_| relay.watch_at("p", camera.clone(), Quality::Medium).frames)
            .collect();
        for viewer in &mut viewers {
            next(viewer).await;
        }
        assert_eq!(camera.opened.load(Ordering::SeqCst), 1);
        assert_eq!(relay.active(), 1);
        assert_eq!(relay.transcoders(), 1);

        // Three viewers, but each frame is transcoded once: no more than the
        // frame rate allows for one.
        let before = relay.transcoded_frames();
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let done = relay.transcoded_frames() - before;
        assert!((2..=7).contains(&done), "{done} transcoded in 1 s");

        // Everyone leaves: the transcoder and the pull both stop.
        drop(viewers);
        eventually("the pull to stop", || {
            camera.live.load(Ordering::SeqCst) == 0
        })
        .await;
        assert_eq!(relay.active(), 0);
        assert_eq!(relay.transcoders(), 0);
    }

    #[tokio::test]
    async fn two_levels_share_the_upstream_pull() {
        let relay = CameraRelay::new();
        let camera = Arc::new(CountingCamera::hd());
        let mut medium = relay.watch_at("p", camera.clone(), Quality::Medium).frames;
        let mut low = relay.watch_at("p", camera.clone(), Quality::Low).frames;
        let mut full = relay.watch_at("p", camera.clone(), Quality::Full).frames;
        assert_eq!(dimensions(&next(&mut medium).await).unwrap().0, 960);
        assert_eq!(dimensions(&next(&mut low).await).unwrap().0, 480);
        assert_eq!(dimensions(&next(&mut full).await).unwrap().0, 1280);
        assert_eq!(camera.opened.load(Ordering::SeqCst), 1);
        assert_eq!(relay.active(), 1);
        assert_eq!(relay.transcoders(), 2);

        // The low viewer leaves: its transcoder stops, the rest carry on.
        drop(low);
        eventually("the low transcoder to stop", || relay.transcoders() == 1).await;
        next(&mut medium).await;
        assert_eq!(camera.live.load(Ordering::SeqCst), 1);

        // A full viewer leaving doesn't stop the pull a transcoder needs.
        drop(full);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(relay.active(), 1);
        next(&mut medium).await;

        drop(medium);
        eventually("the pull to stop", || {
            camera.live.load(Ordering::SeqCst) == 0
        })
        .await;
        assert_eq!(relay.active() + relay.transcoders(), 0);
        assert_eq!(camera.opened.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn frames_that_wont_decode_are_skipped() {
        let relay = CameraRelay::new();
        let camera = Arc::new(CountingCamera::default());
        let mut viewer = relay.watch_at("p", camera.clone(), Quality::Medium).frames;
        let frames = collect(&mut viewer, Duration::from_millis(600)).await;
        assert!(frames.is_empty());
        assert!(relay.transcoded_frames() >= 2, "it kept trying");
        assert_eq!(relay.transcoders(), 1);
    }

    #[test]
    fn the_default_depends_on_where_the_viewer_is() {
        let at = |ip: &str| ClientInfo {
            ip: Some(ip.parse().unwrap()),
            https: false,
            via_proxy: false,
        };
        let (lan, remote) = (at("192.168.1.9"), at("203.0.113.9"));
        let pick = |asked, client| choose(asked, client).ok();
        assert_eq!(pick(None, None), Some(Quality::Full));
        assert_eq!(pick(None, Some(&lan)), Some(Quality::Full));
        assert_eq!(pick(None, Some(&remote)), Some(Quality::Medium));
        assert_eq!(pick(Some(""), Some(&remote)), Some(Quality::Medium));
        assert_eq!(pick(Some("low"), None), Some(Quality::Low));
        assert_eq!(pick(Some("full"), Some(&remote)), Some(Quality::Full));
        assert!(matches!(
            choose(Some("ultra"), None),
            Err(ApiError::Invalid(_))
        ));
    }
}
