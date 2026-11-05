//! A mock of an X1 printer's RTSPS camera (port 322), for tests: a local RTSP
//! test source.
//!
//! TLS with a self-signed certificate; `OPTIONS` without a login and
//! everything else behind Digest (or, if chosen, Basic) authentication as
//! user `bblp` with the access code; one H.264 track at
//! `/streaming/live/1`, sent as RTP interleaved on the RTSP connection
//! (RFC 2326 §10.12), packetized per RFC 6184 (single NAL units, FU-A, and
//! STAP-A for in-band parameter sets).
//!
//! The video is generated at start with OpenH264's encoder: a looping
//! moving pattern, 320×240 at 10 frames a second, a keyframe every 10.
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use md5::{Digest, Md5};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, QpRange, RateControlMode,
};
use openh264::formats::YUVBuffer;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// The stream's path, as on an X1.
pub const PATH: &str = "/streaming/live/1";

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const FRAMES: usize = 30;
const FPS: u64 = 10;
const REALM: &str = "BambuMockCamera";
/// The biggest RTP payload; larger NAL units are split with FU-A.
const MTU: usize = 1000;

/// How the camera asks for a login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    Digest,
    Basic,
}

/// How the mock behaves.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub auth: Auth,
    /// Send SPS and PPS in the stream (as STAP-A) as well as in the SDP.
    pub in_band_parameter_sets: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            auth: Auth::Digest,
            in_band_parameter_sets: false,
        }
    }
}

/// One encoded frame: its NAL units without start codes.
type AccessUnit = Vec<Vec<u8>>;

struct Shared {
    access_code: String,
    options: Options,
    nonce: String,
    tls: tokio_rustls::TlsAcceptor,
    video: Vec<AccessUnit>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    cut: watch::Sender<u64>,
    playing: AtomicUsize,
    sessions_started: AtomicUsize,
    refused: AtomicUsize,
    teardowns: AtomicUsize,
    next_session: AtomicU64,
}

/// A running mock X1 camera.
pub struct MockX1Camera {
    shared: Arc<Shared>,
    address: SocketAddr,
    front: JoinHandle<()>,
}

/// Encodes the looping test video.
fn encode_video() -> (Vec<AccessUnit>, Vec<u8>, Vec<u8>) {
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(400_000))
        .max_frame_rate(FrameRate::from_hz(FPS as f32))
        .skip_frames(false)
        // A fixed quality, so every frame is encoded and nothing is skipped.
        .rate_control_mode(RateControlMode::Off)
        .qp(QpRange::new(26, 26))
        .intra_frame_period(IntraFramePeriod::from_num_frames(10));
    let mut encoder =
        Encoder::with_api_config(OpenH264API::from_source(), config).expect("an H.264 encoder");
    let mut units = Vec::new();
    let (mut sps, mut pps) = (Vec::new(), Vec::new());
    let mut rgb = vec![0u8; WIDTH * HEIGHT * 3];
    for frame in 0..FRAMES {
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let at = (y * WIDTH + x) * 3;
                let moving = (x + frame * 8) % WIDTH;
                rgb[at] = (moving * 255 / WIDTH) as u8;
                rgb[at + 1] = (y * 255 / HEIGHT) as u8;
                rgb[at + 2] = if (x / 16 + y / 16 + frame) % 2 == 0 {
                    200
                } else {
                    40
                };
            }
        }
        let yuv =
            YUVBuffer::from_rgb8_source(openh264::formats::RgbSliceU8::new(&rgb, (WIDTH, HEIGHT)));
        let bitstream = encoder.encode(&yuv).expect("encoding a test frame");
        let mut unit = Vec::new();
        for nal in openh264::nal_units(&bitstream.to_vec()) {
            let start = nal.iter().position(|b| *b == 1).map_or(0, |p| p + 1);
            let mut nal = nal[start..].to_vec();
            while nal.last() == Some(&0) {
                nal.pop();
            }
            if nal.is_empty() {
                continue;
            }
            match nal[0] & 0x1F {
                7 => sps.clone_from(&nal),
                8 => pps.clone_from(&nal),
                _ => unit.push(nal),
            }
        }
        units.push(unit);
    }
    assert!(
        !sps.is_empty() && !pps.is_empty(),
        "the encoder wrote SPS and PPS"
    );
    (units, sps, pps)
}

impl MockX1Camera {
    /// Starts a camera on a random local port that accepts `access_code`,
    /// asking for a Digest login.
    pub async fn start(access_code: &str) -> Self {
        Self::start_with(access_code, Options::default()).await
    }

    pub async fn start_with(access_code: &str, options: Options) -> Self {
        let (video, sps, pps) = tokio::task::spawn_blocking(encode_video)
            .await
            .expect("encoding the test video");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let (cut, _) = watch::channel(0);
        let nonce = format!(
            "{:032x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let shared = Arc::new(Shared {
            access_code: access_code.to_owned(),
            options,
            nonce,
            tls: crate::tls_acceptor("camera"),
            video,
            sps,
            pps,
            cut,
            playing: AtomicUsize::new(0),
            sessions_started: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
            teardowns: AtomicUsize::new(0),
            next_session: AtomicU64::new(0x1234_5678),
        });
        let accepting = shared.clone();
        let front = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                tokio::spawn(connection(accepting.clone(), stream));
            }
        });
        Self {
            shared,
            address,
            front,
        }
    }

    /// Where the camera listens (instead of port 322).
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Sessions playing right now.
    pub fn playing(&self) -> usize {
        self.shared.playing.load(Ordering::SeqCst)
    }

    /// Sessions that have started playing since the start.
    pub fn sessions_started(&self) -> usize {
        self.shared.sessions_started.load(Ordering::SeqCst)
    }

    /// Requests refused for a wrong login (not for a missing one).
    pub fn refused_logins(&self) -> usize {
        self.shared.refused.load(Ordering::SeqCst)
    }

    pub fn teardowns(&self) -> usize {
        self.shared.teardowns.load(Ordering::SeqCst)
    }

    /// Cuts every open connection; the camera keeps listening.
    pub fn drop_connections(&self) {
        self.shared.cut.send_modify(|n| *n += 1);
    }

    /// Waits until `pred` holds for the number of playing sessions.
    pub async fn wait_for_playing(&self, timeout: Duration, pred: impl Fn(usize) -> bool) -> bool {
        tokio::time::timeout(timeout, async {
            while !pred(self.playing()) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }
}

impl Drop for MockX1Camera {
    fn drop(&mut self) {
        self.front.abort();
        self.drop_connections();
    }
}

fn md5_hex(text: &str) -> String {
    Md5::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct Request {
    method: String,
    uri: String,
    headers: BTreeMap<String, String>,
}

async fn read_request<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> Option<Request> {
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim_end().to_owned();
        if line.is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        lines.push(line);
        if lines.len() > 100 {
            return None;
        }
    }
    let mut first = lines[0].split_whitespace();
    let method = first.next()?.to_owned();
    let uri = first.next()?.to_owned();
    let headers = lines[1..]
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    Some(Request {
        method,
        uri,
        headers,
    })
}

fn params(text: &str) -> BTreeMap<String, String> {
    text.split(',')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| {
            (
                k.trim().to_ascii_lowercase(),
                v.trim().trim_matches('"').to_owned(),
            )
        })
        .collect()
}

/// Whether the request carries a correct login.
fn authorised(shared: &Shared, request: &Request) -> Option<bool> {
    let header = request.headers.get("authorization")?;
    let ok = match shared.options.auth {
        Auth::Basic => header
            .strip_prefix("Basic ")
            .and_then(|b| STANDARD.decode(b.trim()).ok())
            .is_some_and(|d| d == format!("{}:{}", crate::USERNAME, shared.access_code).as_bytes()),
        Auth::Digest => header.strip_prefix("Digest ").is_some_and(|p| {
            let p = params(p);
            let get = |k: &str| p.get(k).map(String::as_str).unwrap_or_default();
            let ha1 = md5_hex(&format!(
                "{}:{REALM}:{}",
                crate::USERNAME,
                shared.access_code
            ));
            let ha2 = md5_hex(&format!("{}:{}", request.method, get("uri")));
            let expected = if get("qop") == "auth" {
                md5_hex(&format!(
                    "{ha1}:{}:{}:{}:auth:{ha2}",
                    get("nonce"),
                    get("nc"),
                    get("cnonce")
                ))
            } else {
                md5_hex(&format!("{ha1}:{}:{ha2}", get("nonce")))
            };
            get("username") == crate::USERNAME
                && get("nonce") == shared.nonce
                && get("response") == expected
        }),
    };
    Some(ok)
}

fn response(cseq: &str, status: &str, headers: &[(&str, String)], body: &str) -> Vec<u8> {
    let mut text = format!("RTSP/1.0 {status}\r\nCSeq: {cseq}\r\n");
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() {
        text.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    text.push_str("\r\n");
    text.push_str(body);
    text.into_bytes()
}

async fn connection(shared: Arc<Shared>, stream: TcpStream) {
    let mut cut = shared.cut.subscribe();
    let Ok(Ok(tls)) = tokio::time::timeout(Duration::from_secs(5), shared.tls.accept(stream)).await
    else {
        return;
    };
    let (read, mut write) = tokio::io::split(tls);
    let mut reader = BufReader::new(read);
    // Everything written goes through one queue, so responses and RTP
    // packets never interleave mid-message.
    let (out, mut outgoing) = mpsc::channel::<Vec<u8>>(64);
    let writer = tokio::spawn(async move {
        while let Some(bytes) = outgoing.recv().await {
            if write.write_all(&bytes).await.is_err() || write.flush().await.is_err() {
                break;
            }
        }
        let _ = write.shutdown().await;
    });
    let mut streaming: Option<JoinHandle<()>> = None;
    let mut session: Option<String> = None;

    loop {
        let request = tokio::select! {
            r = read_request(&mut reader) => r,
            _ = cut.changed() => None,
        };
        let Some(request) = request else { break };
        let cseq = request.headers.get("cseq").cloned().unwrap_or_default();
        if request.method != "OPTIONS" {
            match authorised(&shared, &request) {
                Some(true) => {}
                found => {
                    if found == Some(false) {
                        shared.refused.fetch_add(1, Ordering::SeqCst);
                    }
                    let challenge = match shared.options.auth {
                        Auth::Basic => format!("Basic realm=\"{REALM}\""),
                        Auth::Digest => {
                            format!("Digest realm=\"{REALM}\", nonce=\"{}\"", shared.nonce)
                        }
                    };
                    let _ = out
                        .send(response(
                            &cseq,
                            "401 Unauthorized",
                            &[("WWW-Authenticate", challenge)],
                            "",
                        ))
                        .await;
                    continue;
                }
            }
        }
        let reply = match request.method.as_str() {
            "OPTIONS" => response(
                &cseq,
                "200 OK",
                &[(
                    "Public",
                    "OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN".to_owned(),
                )],
                "",
            ),
            "DESCRIBE" if request.uri.ends_with(PATH) => {
                let sdp = format!(
                    "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=Mock X1\r\nt=0 0\r\n\
                     m=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n\
                     a=fmtp:96 packetization-mode=1;sprop-parameter-sets={},{}\r\n\
                     a=control:trackID=1\r\n",
                    STANDARD.encode(&shared.sps),
                    STANDARD.encode(&shared.pps)
                );
                response(
                    &cseq,
                    "200 OK",
                    &[
                        ("Content-Base", format!("{}/", request.uri)),
                        ("Content-Type", "application/sdp".into()),
                    ],
                    &sdp,
                )
            }
            "DESCRIBE" => response(&cseq, "404 Not Found", &[], ""),
            "SETUP" if request.uri.ends_with("trackID=1") => {
                let id = format!("{:X}", shared.next_session.fetch_add(1, Ordering::SeqCst));
                session = Some(id.clone());
                response(
                    &cseq,
                    "200 OK",
                    &[
                        ("Session", format!("{id};timeout=60")),
                        (
                            "Transport",
                            "RTP/AVP/TCP;unicast;interleaved=0-1".to_owned(),
                        ),
                    ],
                    "",
                )
            }
            "SETUP" => response(&cseq, "404 Not Found", &[], ""),
            "PLAY" if session.is_some() && streaming.is_none() => {
                let _ = out
                    .send(response(
                        &cseq,
                        "200 OK",
                        &[("Session", session.clone().unwrap_or_default())],
                        "",
                    ))
                    .await;
                shared.sessions_started.fetch_add(1, Ordering::SeqCst);
                shared.playing.fetch_add(1, Ordering::SeqCst);
                streaming = Some(tokio::spawn(stream_video(shared.clone(), out.clone())));
                continue;
            }
            "PLAY" => response(&cseq, "455 Method Not Valid in This State", &[], ""),
            "TEARDOWN" => {
                shared.teardowns.fetch_add(1, Ordering::SeqCst);
                let _ = out.send(response(&cseq, "200 OK", &[], "")).await;
                break;
            }
            _ => response(&cseq, "501 Not Implemented", &[], ""),
        };
        if out.send(reply).await.is_err() {
            break;
        }
    }
    if let Some(streaming) = streaming {
        streaming.abort();
        shared.playing.fetch_sub(1, Ordering::SeqCst);
    }
    drop(out);
    let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
}

/// Sends the test video, looping, as interleaved RTP on channel 0.
async fn stream_video(shared: Arc<Shared>, out: mpsc::Sender<Vec<u8>>) {
    let mut sequence: u16 = 1000;
    let mut timestamp: u32 = 0;
    let mut ticker = tokio::time::interval(Duration::from_millis(1000 / FPS));
    for unit in shared.video.iter().cycle() {
        ticker.tick().await;
        let keyframe = unit.iter().any(|nal| nal[0] & 0x1F == 5);
        let mut payloads: Vec<Vec<u8>> = Vec::new();
        if keyframe && shared.options.in_band_parameter_sets {
            let mut stap = vec![0x78];
            for set in [&shared.sps, &shared.pps] {
                stap.extend_from_slice(&(set.len() as u16).to_be_bytes());
                stap.extend_from_slice(set);
            }
            payloads.push(stap);
        }
        for nal in unit {
            if nal.len() <= MTU {
                payloads.push(nal.clone());
                continue;
            }
            let indicator = (nal[0] & 0xE0) | 28;
            let kind = nal[0] & 0x1F;
            let chunks: Vec<&[u8]> = nal[1..].chunks(MTU).collect();
            for (i, chunk) in chunks.iter().enumerate() {
                let mut header = kind;
                if i == 0 {
                    header |= 0x80;
                }
                if i == chunks.len() - 1 {
                    header |= 0x40;
                }
                let mut payload = vec![indicator, header];
                payload.extend_from_slice(chunk);
                payloads.push(payload);
            }
        }
        let last = payloads.len().saturating_sub(1);
        for (i, payload) in payloads.into_iter().enumerate() {
            let mut packet = vec![0x80, if i == last { 0x80 | 96 } else { 96 }];
            packet.extend_from_slice(&sequence.to_be_bytes());
            packet.extend_from_slice(&timestamp.to_be_bytes());
            packet.extend_from_slice(&0x0BAD_CAFEu32.to_be_bytes());
            packet.extend_from_slice(&payload);
            let mut framed = vec![b'$', 0];
            framed.extend_from_slice(&(packet.len() as u16).to_be_bytes());
            framed.extend_from_slice(&packet);
            if out.send(framed).await.is_err() {
                return;
            }
            sequence = sequence.wrapping_add(1);
        }
        timestamp = timestamp.wrapping_add((90_000 / FPS) as u32);
    }
}
