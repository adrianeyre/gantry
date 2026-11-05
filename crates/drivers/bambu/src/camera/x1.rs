//! The X1 camera: RTSP over TLS on port 322, `rtsps://<ip>:322/streaming/live/1`,
//! user `bblp` and the access code as password. The H.264 video is decoded
//! and re-encoded as JPEG frames for the MJPEG relay (ADR-0007).
//!
//! Community-documented (OpenBambuAPI, and the ha-bambulab and
//! Bambu Studio sources); never verified here against a real camera.
use std::time::Duration;

use bytes::Bytes;
use futures::stream::BoxStream;
use gantry_core::{CameraError, CameraSource};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::h264::{Depacketizer, JpegWorker};
use super::rtsp::{self, Challenge, Incoming, Response};
use super::{Ended, Target, USERNAME, connect_tls, frames};

/// The most JPEG frames a second sent on from an X1 camera.
pub const MAX_FPS: u32 = 5;

/// The JPEG quality frames are re-encoded at.
const JPEG_QUALITY: u8 = 75;

/// The camera's stream path.
pub const PATH: &str = "/streaming/live/1";

/// How long the camera may go quiet before the connection is retried.
const DATA_TIMEOUT: Duration = Duration::from_secs(15);

/// How long one RTSP request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// An X1 series camera.
#[derive(Debug, Clone)]
pub struct X1Camera {
    target: Target,
}

impl X1Camera {
    pub fn new(host: &str, port: u16, access_code: &str) -> Self {
        Self {
            target: Target {
                host: host.trim().to_owned(),
                port,
                access_code: access_code.to_owned(),
            },
        }
    }
}

impl CameraSource for X1Camera {
    fn open(&self) -> BoxStream<'static, Result<Bytes, CameraError>> {
        frames("x1", self.target.clone(), session)
    }
}

type Tls = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;
type Stream = BufReader<Tls>;

/// One RTSP conversation's state: sequence numbers, the login and the
/// session, remembered between requests.
struct Client {
    target: Target,
    cseq: u32,
    challenge: Option<Challenge>,
    nonce_count: u32,
    session: Option<String>,
}

enum RequestError {
    Refused(String),
    Failed(String),
}

impl Client {
    async fn write<W: AsyncWrite + Unpin>(
        &mut self,
        writer: &mut W,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> std::io::Result<()> {
        self.cseq += 1;
        let mut request = format!(
            "{method} {uri} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: Gantry\r\n",
            self.cseq
        );
        if let Some(challenge) = &self.challenge {
            self.nonce_count += 1;
            request.push_str(&format!(
                "Authorization: {}\r\n",
                challenge.authorize(
                    USERNAME,
                    &self.target.access_code,
                    method,
                    uri,
                    self.nonce_count
                )
            ));
        }
        if let Some(session) = &self.session {
            request.push_str(&format!("Session: {session}\r\n"));
        }
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        writer.write_all(request.as_bytes()).await?;
        writer.flush().await
    }

    /// Sends a request and reads its answer, answering one authentication
    /// challenge. A second refusal means the access code is wrong.
    async fn request(
        &mut self,
        stream: &mut Stream,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> Result<Response, RequestError> {
        let mut retried = false;
        loop {
            let exchange = async {
                self.write(stream, method, uri, headers).await?;
                rtsp::read_response(stream).await
            };
            let response = tokio::time::timeout(REQUEST_TIMEOUT, exchange)
                .await
                .map_err(|_| RequestError::Failed(format!("{method}: timed out")))?
                .map_err(|e| RequestError::Failed(format!("{method}: {e}")))?;
            if response.status != 401 {
                return Ok(response);
            }
            let challenge = Challenge::choose(&response);
            if retried || challenge.is_none() {
                return Err(RequestError::Refused(
                    "the camera refused the access code".into(),
                ));
            }
            self.challenge = challenge;
            self.nonce_count = 0;
            retried = true;
        }
    }
}

fn ok(method: &str, response: &Response) -> Result<(), RequestError> {
    if (200..300).contains(&response.status) {
        Ok(())
    } else {
        Err(RequestError::Failed(format!(
            "{method} answered {}",
            response.status
        )))
    }
}

/// Which interleaved channel RTP arrives on, from the `Transport` header.
fn rtp_channel(transport: Option<&str>) -> u8 {
    transport
        .and_then(|t| {
            t.split(';')
                .find_map(|p| p.trim().strip_prefix("interleaved="))
        })
        .and_then(|i| i.split('-').next())
        .and_then(|c| c.trim().parse().ok())
        .unwrap_or(0)
}

async fn session(target: Target, tx: mpsc::Sender<Result<Bytes, CameraError>>) -> Ended {
    let tls = match connect_tls(&target).await {
        Ok(tls) => tls,
        Err(detail) => {
            return Ended::Failed {
                detail,
                had_frames: false,
            };
        }
    };
    let mut client = Client {
        target,
        cseq: 0,
        challenge: None,
        nonce_count: 0,
        session: None,
    };
    match play(&mut client, BufReader::new(tls), &tx).await {
        Ok(ended) => ended,
        Err(RequestError::Refused(detail)) => Ended::Refused(detail),
        Err(RequestError::Failed(detail)) => Ended::Failed {
            detail,
            had_frames: false,
        },
    }
}

/// What the task reading the connection saw before it stopped.
struct ReadEnd {
    detail: String,
    had_frames: bool,
}

async fn play(
    client: &mut Client,
    mut stream: Stream,
    tx: &mpsc::Sender<Result<Bytes, CameraError>>,
) -> Result<Ended, RequestError> {
    let url = format!(
        "rtsps://{}:{}{PATH}",
        client.target.host, client.target.port
    );
    // Some servers answer OPTIONS without asking for a login; either is fine.
    let _ = client.request(&mut stream, "OPTIONS", &url, &[]).await?;
    let described = client
        .request(
            &mut stream,
            "DESCRIBE",
            &url,
            &[("Accept", "application/sdp")],
        )
        .await?;
    ok("DESCRIBE", &described)?;
    let track = rtsp::video_track(&described.body)
        .ok_or_else(|| RequestError::Failed("the camera offers no H.264 video".into()))?;
    let base = described
        .header("content-base")
        .or_else(|| described.header("content-location"))
        .unwrap_or(&url)
        .to_owned();
    let setup_url = rtsp::resolve_control(&base, track.control.as_deref());
    let set_up = client
        .request(
            &mut stream,
            "SETUP",
            &setup_url,
            &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        )
        .await?;
    ok("SETUP", &set_up)?;
    let (session_id, timeout) = rtsp::session_id(set_up.header("session").unwrap_or_default());
    if session_id.is_empty() {
        return Err(RequestError::Failed("SETUP gave no session".into()));
    }
    let channel = rtp_channel(set_up.header("transport"));
    client.session = Some(session_id);
    let played = client
        .request(&mut stream, "PLAY", &base, &[("Range", "npt=0.000-")])
        .await?;
    ok("PLAY", &played)?;

    // From here a task reads (so no read is ever cut short), a thread
    // decodes, and this loop sends JPEGs on and keeps the session alive.
    let (jpeg_tx, mut jpeg_rx) = mpsc::channel::<Bytes>(2);
    let worker = JpegWorker::spawn(
        Duration::from_millis(1000 / u64::from(MAX_FPS)),
        JPEG_QUALITY,
        jpeg_tx,
    )
    .map_err(|e| RequestError::Failed(format!("could not start decoding: {e}")))?;
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = tokio::spawn(read_video(
        BufReader::new(read),
        channel,
        Depacketizer::new(track.parameter_sets),
        worker,
    ));
    let keepalive_every = Duration::from_secs(timeout.unwrap_or(60).clamp(10, 120) / 2);
    let mut keepalive = tokio::time::interval(keepalive_every);
    keepalive.tick().await;

    let ended = loop {
        tokio::select! {
            read = &mut reader => {
                let end = read.unwrap_or_else(|e| ReadEnd { detail: e.to_string(), had_frames: false });
                break Ended::Failed { detail: end.detail, had_frames: end.had_frames };
            }
            Some(jpeg) = jpeg_rx.recv() => {
                if tx.send(Ok(jpeg)).await.is_err() {
                    break Ended::Unwatched;
                }
            }
            _ = keepalive.tick() => {
                // Bounded, so a stuck write can't stop the loop noticing the
                // viewer has gone.
                match tokio::time::timeout(Duration::from_secs(10), client.write(&mut write, "OPTIONS", &url, &[])).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => break Ended::Failed { detail: e.to_string(), had_frames: true },
                    Err(_) => break Ended::Failed { detail: "the camera stopped reading".into(), had_frames: true },
                }
            }
            () = tx.closed() => break Ended::Unwatched,
        }
    };
    reader.abort();
    if matches!(ended, Ended::Unwatched) {
        let teardown = async {
            client.write(&mut write, "TEARDOWN", &base, &[]).await?;
            write.shutdown().await
        };
        let _ = tokio::time::timeout(Duration::from_secs(1), teardown).await;
    }
    Ok(ended)
}

/// Reads RTP until the connection ends, handing access units to the decoder.
async fn read_video<R: AsyncRead + Unpin>(
    mut reader: BufReader<R>,
    channel: u8,
    mut depacketizer: Depacketizer,
    worker: JpegWorker,
) -> ReadEnd {
    let mut had_frames = false;
    loop {
        let incoming = tokio::time::timeout(DATA_TIMEOUT, rtsp::read_incoming(&mut reader)).await;
        match incoming {
            Ok(Ok(Incoming::Data {
                channel: c,
                payload,
            })) if c == channel => {
                if let Some(unit) = depacketizer.push(&payload) {
                    had_frames = true;
                    if !worker.send(unit).await {
                        return ReadEnd {
                            detail: "the decoder stopped".into(),
                            had_frames,
                        };
                    }
                }
            }
            // RTCP, and answers to keep-alives.
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                return ReadEnd {
                    detail: e.to_string(),
                    had_frames,
                };
            }
            Err(_) => {
                return ReadEnd {
                    detail: "no video for 15 s".into(),
                    had_frames,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_interleaved_channel() {
        assert_eq!(rtp_channel(Some("RTP/AVP/TCP;unicast;interleaved=2-3")), 2);
        assert_eq!(rtp_channel(None), 0);
    }
}
