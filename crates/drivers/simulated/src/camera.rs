//! The Simulated Printer's camera: a looping test pattern of JPEG frames.
//!
//! The frames are rendered once per process. A stream yields one every
//! [`FRAME_INTERVAL`] for as long as it is held, and stops when dropped, so a
//! Simulated Printer's camera costs nothing while nobody watches.
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures::stream::{self, BoxStream, StreamExt};
use gantry_core::{CameraError, CameraSource};

/// How often a frame is sent: 5 frames a second.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(200);

const WIDTH: u16 = 320;
const HEIGHT: u16 = 240;
const FRAMES: usize = 25;

/// The colour bars across the top of the test pattern.
const BARS: [[u8; 3]; 7] = [
    [235, 235, 235],
    [235, 235, 16],
    [16, 235, 235],
    [16, 235, 16],
    [235, 16, 235],
    [235, 16, 16],
    [16, 16, 235],
];

fn render(frame: usize) -> Bytes {
    let (w, h) = (usize::from(WIDTH), usize::from(HEIGHT));
    let mut rgb = vec![0u8; w * h * 3];
    let sweep = frame * w / FRAMES;
    for y in 0..h {
        for x in 0..w {
            let pixel = if y < h / 3 {
                BARS[x * BARS.len() / w]
            } else if x.abs_diff(sweep) < 6 {
                [255, 255, 255]
            } else if y > h * 5 / 6 {
                // A "bed" that fills as the loop goes round.
                if x * FRAMES < (frame + 1) * w {
                    [46, 125, 50]
                } else {
                    [40, 40, 40]
                }
            } else {
                [24, 26, 30]
            };
            let at = (y * w + x) * 3;
            rgb[at..at + 3].copy_from_slice(&pixel);
        }
    }
    let mut jpeg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpeg, 70)
        .encode(&rgb, WIDTH, HEIGHT, jpeg_encoder::ColorType::Rgb)
        .expect("encoding an in-memory test pattern cannot fail");
    Bytes::from(jpeg)
}

fn frames() -> &'static [Bytes] {
    static FRAMES_CELL: OnceLock<Vec<Bytes>> = OnceLock::new();
    FRAMES_CELL.get_or_init(|| (0..FRAMES).map(render).collect())
}

/// A Simulated Printer's camera.
#[derive(Debug, Default, Clone)]
pub struct TestPatternCamera {
    open: Arc<AtomicUsize>,
}

impl TestPatternCamera {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many streams are open right now.
    pub fn open_streams(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

/// Counts an open stream for as long as it lives.
struct OpenGuard(Arc<AtomicUsize>);

impl Drop for OpenGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl CameraSource for TestPatternCamera {
    fn open(&self) -> BoxStream<'static, Result<Bytes, CameraError>> {
        self.open.fetch_add(1, Ordering::SeqCst);
        let guard = OpenGuard(self.open.clone());
        stream::unfold((0usize, guard), |(n, guard)| async move {
            if n > 0 {
                tokio::time::sleep(FRAME_INTERVAL).await;
            }
            let frame = frames()[n % FRAMES].clone();
            Some((Ok(frame), (n + 1, guard)))
        })
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_jpegs() {
        for frame in frames() {
            assert_eq!(&frame[..2], [0xFF, 0xD8], "JPEG start of image");
            assert_eq!(&frame[frame.len() - 2..], [0xFF, 0xD9], "JPEG end of image");
        }
        assert_ne!(frames()[0], frames()[1], "the pattern moves");
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_loops_and_is_counted_while_open() {
        let camera = TestPatternCamera::new();
        assert_eq!(camera.open_streams(), 0);
        let mut stream = camera.open();
        assert_eq!(camera.open_streams(), 1);
        for _ in 0..FRAMES + 2 {
            stream.next().await.unwrap().unwrap();
        }
        drop(stream);
        assert_eq!(camera.open_streams(), 0);
    }
}
