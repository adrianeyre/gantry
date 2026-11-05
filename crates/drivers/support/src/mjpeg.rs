//! A camera served over HTTP (PRD §10.4): an MJPEG stream
//! (`multipart/x-mixed-replace`, as mjpg-streamer, ustreamer, crowsnest and
//! OctoPrint's webcam serve it), or a URL that returns one JPEG snapshot,
//! which is fetched again and again. Either way it is a
//! [`CameraSource`] of JPEG frames, which the server's camera relay takes
//! like any other: it is only opened while someone watches, and dropping
//! the stream closes the connection.
//!
//! Frames are found by their JPEG start and end markers rather than by the
//! multipart boundary, which streamers write in many different ways.
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use futures::stream::BoxStream;
use gantry_core::{CameraError, CameraSource};
use tokio::sync::mpsc;

/// How often a snapshot URL is fetched again.
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(500);

/// The largest frame accepted; anything bigger is not a camera frame.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Splits a byte stream into the JPEG frames in it.
#[derive(Debug, Default)]
pub struct FrameSplitter {
    buffer: BytesMut,
}

impl FrameSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes in more bytes; returns every frame they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Bytes> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            let Some(start) = find(&self.buffer, &[0xFF, 0xD8]) else {
                // Keep a trailing 0xFF: it may be half of a start marker.
                let keep = usize::from(self.buffer.last() == Some(&0xFF));
                let drop = self.buffer.len() - keep;
                let _ = self.buffer.split_to(drop);
                break;
            };
            let _ = self.buffer.split_to(start);
            match find(&self.buffer[2..], &[0xFF, 0xD9]) {
                Some(end) => {
                    let frame = self.buffer.split_to(end + 4).freeze();
                    frames.push(frame);
                }
                None => {
                    if self.buffer.len() > MAX_FRAME {
                        self.buffer.clear();
                    }
                    break;
                }
            }
        }
        frames
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A camera at an HTTP URL.
#[derive(Clone)]
pub struct MjpegCamera {
    url: String,
    client: reqwest::Client,
    headers: reqwest::header::HeaderMap,
}

impl std::fmt::Debug for MjpegCamera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Headers may carry an API key, so only the URL is shown.
        f.debug_struct("MjpegCamera")
            .field("url", &self.url)
            .finish()
    }
}

impl MjpegCamera {
    /// A camera at `url`, fetched with these headers (such as an API key).
    pub fn new(url: impl Into<String>, headers: reqwest::header::HeaderMap) -> Self {
        Self {
            url: url.into(),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            headers,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    async fn pull(self, frames: mpsc::Sender<Result<Bytes, CameraError>>) {
        loop {
            let response = self
                .client
                .get(&self.url)
                .headers(self.headers.clone())
                .send()
                .await
                .and_then(reqwest::Response::error_for_status);
            let response = match response {
                Ok(r) => r,
                Err(e) => {
                    let _ = frames
                        .send(Err(CameraError(format!("the camera didn't answer: {e}"))))
                        .await;
                    return;
                }
            };
            let multipart = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|t| t.to_ascii_lowercase().starts_with("multipart/"));
            let mut splitter = FrameSplitter::new();
            let mut response = response;
            loop {
                let chunk = match response.chunk().await {
                    Ok(Some(c)) => c,
                    Ok(None) => break,
                    Err(e) => {
                        let _ = frames
                            .send(Err(CameraError(format!("the camera stream broke: {e}"))))
                            .await;
                        return;
                    }
                };
                for frame in splitter.push(&chunk) {
                    if frames.send(Ok(frame)).await.is_err() {
                        // Nobody is watching any more.
                        return;
                    }
                }
            }
            if multipart {
                let _ = frames
                    .send(Err(CameraError("the camera stream ended".into())))
                    .await;
                return;
            }
            // A snapshot: fetch it again in a moment, while someone watches.
            tokio::select! {
                () = tokio::time::sleep(SNAPSHOT_INTERVAL) => {}
                () = frames.closed() => return,
            }
        }
    }
}

impl CameraSource for MjpegCamera {
    fn open(&self) -> BoxStream<'static, Result<Bytes, CameraError>> {
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(self.clone().pull(tx));
        futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|f| (f, rx)) })
            .boxed()
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn jpeg(n: u8) -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xE0, n, n, 0x00, 0xFF, 0xD9]
    }

    #[test]
    fn frames_are_split_however_the_bytes_arrive() {
        let mut stream = Vec::new();
        for n in 1..=3 {
            stream.extend_from_slice(
                b"--boundarydonotcross\r\nContent-Type: image/jpeg\r\nContent-Length: 9\r\n\r\n",
            );
            stream.extend_from_slice(&jpeg(n));
            stream.extend_from_slice(b"\r\n");
        }
        for size in [1, 2, 5, 64] {
            let mut splitter = FrameSplitter::new();
            let frames: Vec<Bytes> = stream.chunks(size).flat_map(|c| splitter.push(c)).collect();
            assert_eq!(frames.len(), 3, "chunks of {size}");
            assert_eq!(&frames[1][..], &jpeg(2)[..]);
        }
    }

    /// A one-connection-at-a-time HTTP server answering every request with
    /// `respond`.
    async fn serve(multipart: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut request = [0u8; 1024];
                    let _ = socket.read(&mut request).await;
                    if multipart {
                        let _ = socket
                            .write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace;boundary=frame\r\n\r\n",
                            )
                            .await;
                        for n in 0..50u8 {
                            let mut part = b"--frame\r\nContent-Type: image/jpeg\r\n\r\n".to_vec();
                            part.extend_from_slice(&jpeg(n));
                            part.extend_from_slice(b"\r\n");
                            if socket.write_all(&part).await.is_err() {
                                return;
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    } else {
                        let body = jpeg(7);
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = socket.write_all(head.as_bytes()).await;
                        let _ = socket.write_all(&body).await;
                    }
                });
            }
        });
        format!("http://{address}/")
    }

    #[tokio::test]
    async fn an_mjpeg_stream_gives_its_frames() {
        let camera = MjpegCamera::new(serve(true).await, Default::default());
        let frames: Vec<_> = camera.open().take(3).collect().await;
        assert_eq!(frames.len(), 3);
        assert_eq!(&frames[2].as_ref().unwrap()[..], &jpeg(2)[..]);
    }

    #[tokio::test]
    async fn a_snapshot_url_is_fetched_again_and_again() {
        let camera = MjpegCamera::new(serve(false).await, Default::default());
        let frames: Vec<_> = camera.open().take(2).collect().await;
        assert!(
            frames
                .iter()
                .all(|f| f.as_ref().is_ok_and(|f| f[..] == jpeg(7)[..]))
        );
    }

    #[tokio::test]
    async fn a_camera_that_isnt_there_is_an_error() {
        let camera = MjpegCamera::new("http://127.0.0.1:9/", Default::default());
        let first = camera.open().next().await.unwrap();
        assert!(first.is_err());
    }
}
