//! Bambu cameras (PRD §10.3), as [`CameraSource`]s for the server's relay.
//!
//! - **P1 and A1** ([`p1`]): TLS to port 6000, an 80-byte login packet, then
//!   a stream of JPEG frames, each after a 16-byte header.
//! - **X1** ([`x1`]): RTSP over TLS (RTSPS) on port 322, H.264 over RTP. The
//!   video is decoded with OpenH264 (compiled in from source, so there is no
//!   runtime dependency) and re-encoded as JPEG frames, at most
//!   [`x1::MAX_FPS`] a second (ADR-0007).
//!
//! A camera is pulled only while its stream is held: dropping the stream
//! closes the connection. While held, a dropped connection is retried with
//! backoff. A login the camera refuses ends the stream with an error, since
//! retrying the same access code cannot help.
//!
//! Both are tested against `gantry-mock-bambu`'s mock cameras only. Real
//! cameras are unverified (tracked in #28) until a Bambu printer is owned.
//! The access code is never logged.
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::stream::BoxStream;
use gantry_core::{CameraError, CameraSource};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::client::TlsStream;

mod h264;
mod p1;
mod rtsp;
mod x1;

pub use p1::P1Camera;
pub use x1::{MAX_FPS, X1Camera};

/// The P1 and A1 series' camera port.
pub const P1_CAMERA_PORT: u16 = 6000;

/// The X1 series' RTSPS camera port.
pub const X1_CAMERA_PORT: u16 = 322;

/// The username every Bambu camera expects.
const USERNAME: &str = "bblp";

/// The first wait before reconnecting, doubled after each failure.
const BACKOFF_MIN: Duration = Duration::from_millis(250);
/// The longest wait between reconnection attempts.
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// How long connecting (TCP and TLS) may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A P1 or A1 camera at `host:port` (normally [`P1_CAMERA_PORT`]).
pub fn p1(host: &str, port: u16, access_code: &str) -> Arc<dyn CameraSource> {
    Arc::new(P1Camera::new(host, port, access_code))
}

/// An X1 camera at `host:port` (normally [`X1_CAMERA_PORT`]).
pub fn x1(host: &str, port: u16, access_code: &str) -> Arc<dyn CameraSource> {
    Arc::new(X1Camera::new(host, port, access_code))
}

/// Where a camera is and how to log in. `Debug` never shows the code.
#[derive(Clone)]
pub(crate) struct Target {
    pub host: String,
    pub port: u16,
    pub access_code: String,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// How one connection to a camera ended.
#[derive(Debug)]
pub(crate) enum Ended {
    /// Nobody is watching any more.
    Unwatched,
    /// The camera refused the login: give up.
    Refused(String),
    /// The camera closed the connection straight after the login, without
    /// a frame. The P1 camera's way of refusing a login is not documented, so
    /// this is retried, and only taken as a refusal after
    /// [`UNANSWERED_LOGINS`] in a row.
    LoginUnanswered,
    /// Anything else (unreachable, dropped, garbled): try again.
    Failed { detail: String, had_frames: bool },
}

/// How many logins in a row may be closed without a frame before the stream
/// gives up and reports the access code as refused.
pub(crate) const UNANSWERED_LOGINS: u32 = 3;

/// Opens a TLS connection to a Bambu camera, which (like the broker) has a
/// self-signed certificate (see `tls.rs`).
pub(crate) async fn connect_tls(target: &Target) -> Result<TlsStream<TcpStream>, String> {
    let connect = async {
        let tcp = TcpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(|e| format!("unreachable: {}", e.kind()))?;
        let _ = tcp.set_nodelay(true);
        let name = ServerName::try_from(target.host.clone())
            .map_err(|_| "the address is not a valid host name".to_owned())?;
        tokio_rustls::TlsConnector::from(crate::tls::client_config())
            .connect(name, tcp)
            .await
            .map_err(|e| format!("TLS: {e}"))
    };
    tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| "timed out connecting".to_owned())?
}

/// Runs `session` again and again (with backoff) while the returned stream is
/// held, sending its frames to the stream. `session` gets the sender and
/// says how it ended.
pub(crate) fn frames<F, Fut>(
    name: &'static str,
    target: Target,
    session: F,
) -> BoxStream<'static, Result<Bytes, CameraError>>
where
    F: Fn(Target, mpsc::Sender<Result<Bytes, CameraError>>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Ended> + Send,
{
    let (tx, mut rx) = mpsc::channel::<Result<Bytes, CameraError>>(2);
    tokio::spawn(async move {
        let mut backoff = BACKOFF_MIN;
        // Consecutive connections that ended before a single frame.
        let mut empty = 0u32;
        let mut unanswered = 0u32;
        loop {
            let ended = tokio::select! {
                ended = session(target.clone(), tx.clone()) => ended,
                () = tx.closed() => Ended::Unwatched,
            };
            match ended {
                Ended::Unwatched => break,
                Ended::Refused(detail) => {
                    tracing::warn!(camera = name, host = %target.host, %detail, "camera refused the login");
                    let _ = tx.send(Err(CameraError(detail))).await;
                    break;
                }
                Ended::LoginUnanswered => {
                    unanswered += 1;
                    empty += 1;
                    if unanswered >= UNANSWERED_LOGINS {
                        let detail = "the camera closed the connection after each login: check the access code"
                            .to_owned();
                        tracing::warn!(camera = name, host = %target.host, "camera refused the login");
                        let _ = tx.send(Err(CameraError(detail))).await;
                        break;
                    }
                }
                Ended::Failed { detail, had_frames } => {
                    unanswered = 0;
                    if had_frames {
                        backoff = BACKOFF_MIN;
                        empty = 0;
                    } else {
                        empty += 1;
                    }
                    if empty <= 1 || had_frames {
                        tracing::info!(camera = name, host = %target.host, %detail, "camera connection ended; reconnecting");
                    } else {
                        tracing::debug!(camera = name, host = %target.host, %detail, "camera still unavailable");
                    }
                }
            }
            tokio::select! {
                () = tokio::time::sleep(backoff) => {}
                () = tx.closed() => break,
            }
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
        tracing::debug!(camera = name, host = %target.host, "camera stream stopped");
    });
    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}
