//! The Elegoo SDCP Driver (PRD §10.2): newer Elegoo printers such as the
//! Centauri Carbon, over SDCP v3 (Elegoo's Smart Device Control Protocol,
//! github.com/cbd-tech/SDCP-Smart-Device-Control-Protocol-V3.0.0).
//!
//! Everything but the upload goes over one WebSocket,
//! `ws://<address>:3030/websocket`, as JSON messages with a `Topic`:
//! the Driver sends `sdcp/request/<MainboardID>` messages, each with a
//! `Data.Cmd`, and the printer answers on `sdcp/response/<MainboardID>`
//! (matched by `RequestID`) and pushes `sdcp/status/<MainboardID>` and
//! `sdcp/attributes/<MainboardID>` when they change. The status maps onto
//! the normalised model in [`status`]. The printer's MainboardID is learned
//! from its messages (or, best effort, from UDP discovery: `M99999` to port
//! 3000), so the only setting the Driver needs is the address. The text
//! `ping`, answered `pong`, keeps the connection alive. A dropped connection
//! is retried with backoff for as long as the Printer is in the Fleet.
//!
//! Commands: pause (Cmd 129), resume (131) and Cancel (130, SDCP's "stop");
//! a Print File is uploaded over HTTP to `/uploadFile/upload` in 1 MiB
//! chunks with its MD5, then started with Cmd 128. Fine controls
//! (temperatures, fans, the chamber light and the speed profile) are the
//! Centauri Carbon's Cmd 403, which the SDCP v3 spec doesn't document: the
//! payloads are the ones community clients use. Homing and jogging are left
//! out, because their commands' payloads are not documented anywhere
//! Gantry could check.
//!
//! The camera is the one named in the Printer's settings, or else the
//! `VideoUrl` the printer gives when its video stream is enabled (Cmd 386),
//! read as MJPEG over HTTP (`http://<address>:3031/video` on the Centauri
//! Carbon). An RTSP `VideoUrl`, as the spec's resin printers give, is not
//! used.
//!
//! A Job's key is the printer's `TaskId`, or else the file and when it
//! started. Complete is a Succeeded Job, stopped a Cancelled one, and an
//! `ErrorNumber` or the print's error status makes it Failed.
//!
//! **Unverified against real hardware.** The Driver was written from the
//! SDCP v3 spec and community implementations (the Home Assistant
//! `elegoo_printer` integration, OpenCentauri's notes), and tested only
//! against an in-process mock SDCP server written from the spec
//! (`tests/mock_sdcp/`). No Centauri Carbon has run it.
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use gantry_core::{
    CameraSource, CommandError, Connection, DisconnectReason, Driver, EventSender, FieldError,
    FieldKind, FieldSpec, FineControl, Heater, JobOutcome, LightId, PrintUpload, PrinterConfig,
    PrinterHandle, PrinterState, PrinterStatus, Secrets,
};
use gantry_driver_support::mjpeg::MjpegCamera;
use md5::{Digest, Md5};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

pub mod status;

use status::{Context, Print, Track};

/// The Driver id stored with each Elegoo SDCP Printer.
pub const DRIVER_ID: &str = "elegoo-sdcp";

/// SDCP's WebSocket and upload port.
pub const DEFAULT_PORT: u16 = 3030;

/// SDCP's UDP discovery port.
pub const DISCOVERY_PORT: u16 = 3000;

/// The size of each upload chunk. The spec says "1MB" packets; community
/// clients send 1 MiB.
pub const CHUNK: usize = 1024 * 1024;

/// SDCP's commands (`Data.Cmd`).
pub mod cmd {
    pub const STATUS: u64 = 0;
    pub const ATTRIBUTES: u64 = 1;
    pub const START_PRINT: u64 = 128;
    pub const PAUSE: u64 = 129;
    /// SDCP's "stop print": a Cancel.
    pub const CANCEL: u64 = 130;
    pub const RESUME: u64 = 131;
    pub const END_TRANSFER: u64 = 255;
    pub const VIDEO_STREAM: u64 = 386;
    /// The Centauri Carbon's device control (not in the SDCP v3 spec).
    pub const CONTROL: u64 = 403;
}

const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(15);

/// How often the Driver sends `ping`.
const HEARTBEAT: Duration = Duration::from_secs(10);

/// How long without a word from the printer (a `pong` included) before the
/// connection counts as lost.
const SILENCE: Duration = Duration::from_secs(30);

/// How long a print that has gone idle without saying how it ended is held
/// for its ending, before the Fleet is told it ended unseen.
const ENDING_GRACE: Duration = Duration::from_secs(5);

const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);

/// The Elegoo SDCP Driver.
#[derive(Debug, Clone, Copy, Default)]
pub struct ElegooSdcpDriver;

impl Driver for ElegooSdcpDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn name(&self) -> &'static str {
        "Elegoo (SDCP)"
    }

    fn fields(&self) -> Vec<FieldSpec> {
        vec![
            FieldSpec::new("host", "Address", FieldKind::Text)
                .required()
                .help("The printer's IP address, from its network settings screen."),
            FieldSpec::new("port", "Port", FieldKind::Number)
                .help("SDCP's port, 3030 unless something sits in between.")
                .default_value(DEFAULT_PORT.into()),
            FieldSpec::new("model", "Printer model", FieldKind::Text).help(
                "The printer's model as your slicer names it, such as Elegoo Centauri Carbon. A Print \
                 File is only ever sent to the model it was sliced for.",
            ),
            FieldSpec::new("camera_url", "Camera URL", FieldKind::Text).help(
                "An MJPEG stream URL. Leave it empty to use the camera stream the printer offers.",
            ),
        ]
    }

    fn validate(&self, settings: &Map<String, Value>, secrets: &Secrets) -> Vec<FieldError> {
        let mut errors = gantry_core::driver::validate_fields(&self.fields(), settings, secrets);
        if let Some(port) = settings.get("port").and_then(Value::as_f64)
            && !(1.0..=65535.0).contains(&port)
        {
            errors.push(FieldError {
                field: "port".into(),
                message: "Must be between 1 and 65535".into(),
            });
        }
        if let Some(host) = settings.get("host").and_then(Value::as_str)
            && (host.contains('/') || host.contains(' '))
        {
            errors.push(FieldError {
                field: "host".into(),
                message: "Just the address, such as 192.168.1.40, without ws:// or a path".into(),
            });
        }
        errors
    }

    fn connect(&self, printer: PrinterConfig, events: EventSender) -> Box<dyn PrinterHandle> {
        let host = printer
            .text("host")
            .unwrap_or("localhost")
            .trim()
            .to_owned();
        let port = printer
            .number("port")
            .map_or(DEFAULT_PORT, |p| p.clamp(1.0, 65535.0) as u16);
        let shared = Arc::new(Shared {
            name: printer.name.clone(),
            base: format!("http://{host}:{port}"),
            ws: format!("ws://{host}:{port}/websocket"),
            host,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            client_id: uuid::Uuid::new_v4().simple().to_string(),
            model: printer.text("model").map(str::to_owned),
            camera_url: printer.text("camera_url").map(str::to_owned),
            events,
            state: Mutex::new(State::default()),
        });
        let task = tokio::spawn(run(shared.clone()));
        Box::new(ElegooSdcpPrinter {
            shared,
            task: Mutex::new(Some(task)),
        })
    }
}

#[derive(Default)]
struct State {
    /// The status so far, merged from every status message.
    raw: Map<String, Value>,
    attributes: Value,
    mainboard: Option<String>,
    /// Whether this session has had a status message.
    heard: bool,
    track: Option<Track>,
    /// When the followed print was first seen gone without an ending.
    idle_since: Option<tokio::time::Instant>,
    status: Option<PrinterStatus>,
    camera: Option<Arc<MjpegCamera>>,
    /// The open connection's outgoing messages.
    outbox: Option<mpsc::UnboundedSender<String>>,
    /// Requests waiting for their response, by `RequestID`.
    pending: HashMap<String, oneshot::Sender<Value>>,
}

struct Shared {
    name: String,
    host: String,
    base: String,
    ws: String,
    client: reqwest::Client,
    /// The `Id` Gantry's requests carry.
    client_id: String,
    model: Option<String>,
    camera_url: Option<String>,
    events: EventSender,
    state: Mutex<State>,
}

struct Down {
    reason: DisconnectReason,
    detail: String,
}

fn down(reason: DisconnectReason, detail: impl Into<String>) -> Down {
    Down {
        reason,
        detail: detail.into(),
    }
}

/// An SDCP request.
pub fn request_message(
    client_id: &str,
    mainboard: &str,
    request_id: &str,
    cmd: u64,
    data: Value,
) -> Value {
    json!({
        "Id": client_id,
        "Data": {
            "Cmd": cmd,
            "Data": data,
            "RequestID": request_id,
            "MainboardID": mainboard,
            "TimeStamp": time::OffsetDateTime::now_utc().unix_timestamp(),
            "From": 0,
        },
        "Topic": format!("sdcp/request/{mainboard}"),
    })
}

/// The MainboardID a message names, wherever it sits.
pub fn mainboard_of(message: &Value) -> Option<String> {
    [
        &message["MainboardID"],
        &message["Data"]["MainboardID"],
        &message["Attributes"]["MainboardID"],
        &message["Status"]["MainboardID"],
    ]
    .into_iter()
    .find_map(|v| v.as_str().filter(|s| !s.is_empty()))
    .map(str::to_owned)
    .or_else(|| {
        let topic = message["Topic"].as_str()?;
        let id = topic.rsplit_once('/')?.1;
        (!id.is_empty() && topic.starts_with("sdcp/")).then(|| id.to_owned())
    })
}

/// Whether a response's `Ack` is success, and what the printer means if not.
pub fn ack(cmd: u64, data: &Value) -> Result<(), CommandError> {
    let code = data["Ack"].as_u64().unwrap_or(0);
    if code == 0 {
        return Ok(());
    }
    let message = match (cmd, code) {
        (cmd::START_PRINT, 1) => "the printer is busy",
        (cmd::START_PRINT, 2) => "the printer can't find the Print File",
        (cmd::START_PRINT, 3) => "the Print File failed its MD5 check",
        (cmd::START_PRINT, 4) => "the printer couldn't read the Print File",
        (cmd::START_PRINT, 5) => "the Print File is for another resolution",
        (cmd::START_PRINT, 6) => "the printer doesn't know the Print File's format",
        (cmd::START_PRINT, 7) => "the Print File was sliced for another printer model",
        (cmd::VIDEO_STREAM, 1) => "too many camera streams are open",
        (cmd::VIDEO_STREAM, 2) => "the printer has no camera",
        _ => "",
    };
    let message = if message.is_empty() {
        format!("the printer answered Cmd {cmd} with Ack {code}")
    } else {
        message.to_owned()
    };
    if cmd == cmd::START_PRINT && code == 1 {
        return Err(CommandError::invalid_state(message));
    }
    Err(CommandError::Refused { message })
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn report(&self, status: PrinterStatus) {
        self.lock().status = Some(status.clone());
        self.events.status(status);
    }

    /// Sends a request on the open connection without waiting for its answer.
    fn send(&self, cmd: u64, data: Value) {
        let state = self.lock();
        if let Some(outbox) = &state.outbox {
            let mainboard = state.mainboard.clone().unwrap_or_default();
            let id = uuid::Uuid::new_v4().simple().to_string();
            let message = request_message(&self.client_id, &mainboard, &id, cmd, data);
            let _ = outbox.send(message.to_string());
        }
    }

    /// Sends a request and waits for its response's `Data`.
    async fn request(&self, cmd: u64, data: Value) -> Result<Value, CommandError> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let answer = {
            let mut state = self.lock();
            let Some(outbox) = state.outbox.clone() else {
                return Err(CommandError::Disconnected {
                    reason: state
                        .status
                        .as_ref()
                        .and_then(|s| match &s.connection {
                            Connection::Disconnected { reason, .. } => Some(*reason),
                            _ => None,
                        })
                        .unwrap_or(DisconnectReason::Unreachable),
                });
            };
            let Some(mainboard) = state.mainboard.clone() else {
                return Err(CommandError::invalid_state(
                    "the Printer is still connecting",
                ));
            };
            let (reply, answer) = oneshot::channel();
            state.pending.insert(id.clone(), reply);
            let message = request_message(&self.client_id, &mainboard, &id, cmd, data);
            if outbox.send(message.to_string()).is_err() {
                state.pending.remove(&id);
                return Err(CommandError::Disconnected {
                    reason: DisconnectReason::Unreachable,
                });
            }
            answer
        };
        match tokio::time::timeout(COMMAND_TIMEOUT, answer).await {
            Ok(Ok(data)) => {
                ack(cmd, &data)?;
                Ok(data)
            }
            Ok(Err(_)) => Err(CommandError::Disconnected {
                reason: DisconnectReason::Unreachable,
            }),
            Err(_) => {
                self.lock().pending.remove(&id);
                Err(CommandError::TimedOut)
            }
        }
    }

    fn require_connected(&self) -> Result<(), CommandError> {
        match self.lock().status.as_ref().map(|s| s.connection.clone()) {
            Some(Connection::Connected) => Ok(()),
            Some(Connection::Disconnected { reason, .. }) => {
                Err(CommandError::Disconnected { reason })
            }
            _ => Err(CommandError::invalid_state(
                "the Printer is still connecting",
            )),
        }
    }

    fn printer_state(&self) -> Option<PrinterState> {
        self.lock().status.as_ref().and_then(|s| s.state)
    }

    /// Takes in one message. Returns whether it taught the Driver the
    /// printer's MainboardID.
    fn on_message(&self, message: &Value) -> bool {
        let learned = match mainboard_of(message) {
            Some(id) => {
                let mut state = self.lock();
                let new = state.mainboard.is_none();
                if state.mainboard.as_deref() != Some(&id) {
                    state.mainboard = Some(id);
                }
                new
            }
            None => false,
        };
        let topic = message["Topic"].as_str().unwrap_or_default();
        if topic.starts_with("sdcp/response/") {
            let data = &message["Data"];
            if let Some(id) = data["RequestID"].as_str()
                && let Some(reply) = self.lock().pending.remove(id)
            {
                let _ = reply.send(data["Data"].clone());
            }
        } else if let Some(Value::Object(update)) = message.get("Status") {
            {
                let mut state = self.lock();
                status::merge(&mut state.raw, update);
                state.heard = true;
            }
            self.on_status();
        } else if let Some(attributes) = message.get("Attributes") {
            let mut state = self.lock();
            if let (Value::Object(known), Value::Object(new)) = (&mut state.attributes, attributes)
            {
                status::merge(known, new);
            } else {
                state.attributes = attributes.clone();
            }
        } else if topic.starts_with("sdcp/error/") || topic.starts_with("sdcp/notice/") {
            tracing::info!(printer = %self.name, message = %message["Data"]["Data"], "the printer says");
        }
        learned
    }

    /// Follows the print in the status known so far, and reports it.
    fn on_status(&self) {
        let mut state = self.lock();
        let raw = Value::Object(state.raw.clone());
        match status::print(&raw) {
            Print::Running(_) => {
                state.idle_since = None;
                if state.track.as_ref().is_none_or(|t| !t.is(&raw)) {
                    let track = Track::new(&raw, time::OffsetDateTime::now_utc());
                    tracing::info!(printer = %self.name, key = %track.key, "a print is running");
                    state.track = Some(track);
                }
                if !status::faults(&raw).is_empty()
                    && let Some(track) = &mut state.track
                {
                    track.errored = true;
                }
            }
            Print::Ended(outcome) if state.track.is_some() => {
                state.idle_since = None;
                if let Some(track) = state.track.take() {
                    if track.is(&raw) {
                        tracing::info!(printer = %self.name, key = %track.key, outcome = outcome.as_str(), "a print ended");
                        self.events.job_ended(track.key, outcome);
                    } else {
                        tracing::info!(printer = %self.name, key = %track.key, "a print ended without the printer saying how");
                    }
                }
            }
            Print::Ended(_) | Print::None if state.track.is_some() => {
                // Its ending may be a moment behind: wait for it.
                let since = *state
                    .idle_since
                    .get_or_insert_with(tokio::time::Instant::now);
                if since.elapsed() < ENDING_GRACE {
                    return;
                }
                state.idle_since = None;
                if let Some(track) = state.track.take() {
                    if track.errored {
                        tracing::info!(printer = %self.name, key = %track.key, "a print ended after an error");
                        self.events.job_ended(track.key, JobOutcome::Failed);
                    } else {
                        tracing::info!(printer = %self.name, key = %track.key, "a print ended without the printer saying how");
                    }
                }
            }
            Print::Ended(_) | Print::None => {}
        }
        let status = status::status(
            &raw,
            Context {
                track: state.track.as_ref(),
                camera: state.camera.is_some(),
            },
        );
        state.status = Some(status.clone());
        drop(state);
        self.events.status(status);
    }

    /// The printer's model: the one in the settings, or else its own name
    /// for itself, as Elegoo's slicer names it.
    fn model(&self) -> Option<String> {
        if let Some(model) = &self.model {
            return Some(model.clone());
        }
        let state = self.lock();
        let name = state.attributes["MachineName"].as_str()?.trim();
        if name.is_empty() {
            None
        } else if name.to_ascii_lowercase().starts_with("elegoo") {
            Some(name.to_owned())
        } else {
            Some(format!("Elegoo {name}"))
        }
    }
}

/// Asks for the video stream (Cmd 386) and takes its `VideoUrl` as the
/// camera, unless the settings name one.
async fn find_camera(shared: Arc<Shared>) {
    let answer = shared
        .request(cmd::VIDEO_STREAM, json!({ "Enable": 1 }))
        .await;
    let url = match (&shared.camera_url, answer) {
        (Some(url), _) => Some(url.clone()),
        (None, Ok(data)) => data["VideoUrl"].as_str().and_then(|url| {
            let url = url.trim();
            if url.starts_with("http://") || url.starts_with("https://") {
                Some(url.to_owned())
            } else if !url.is_empty() && !url.contains("://") {
                // Some firmware gives just `<address>:3031/video`.
                Some(format!("http://{url}"))
            } else {
                None
            }
        }),
        (None, Err(e)) => {
            tracing::info!(printer = %shared.name, error = %e, "no camera stream");
            None
        }
    };
    let Some(url) = url else { return };
    shared.lock().camera = Some(Arc::new(MjpegCamera::new(
        url,
        reqwest::header::HeaderMap::new(),
    )));
    shared.on_status();
}

/// UDP discovery (`M99999` to port 3000), for the MainboardID only.
async fn discover(host: String) -> Option<String> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await.ok()?;
    socket
        .send_to(b"M99999", (host.as_str(), DISCOVERY_PORT))
        .await
        .ok()?;
    let mut buffer = [0u8; 4096];
    let (n, _) = tokio::time::timeout(Duration::from_secs(3), socket.recv_from(&mut buffer))
        .await
        .ok()?
        .ok()?;
    let reply: Value = serde_json::from_slice(&buffer[..n]).ok()?;
    mainboard_of(&reply)
}

async fn run(shared: Arc<Shared>) {
    let mut backoff = BACKOFF_MIN;
    loop {
        if shared.events.is_closed() {
            return;
        }
        let started = tokio::time::Instant::now();
        let result = session(&shared).await;
        {
            let mut state = shared.lock();
            state.outbox = None;
            state.pending.clear();
            state.heard = false;
            state.camera = None;
        }
        if let Err(Down { reason, detail }) = result {
            tracing::info!(printer = %shared.name, reason = reason.describe(), %detail, "SDCP connection down");
            shared.report(PrinterStatus {
                connection: Connection::Disconnected {
                    reason,
                    detail: Some(detail),
                },
                ..PrinterStatus::disconnected(reason)
            });
        }
        if started.elapsed() > Duration::from_secs(30) {
            backoff = BACKOFF_MIN;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

async fn session(shared: &Arc<Shared>) -> Result<(), Down> {
    shared.events.status(PrinterStatus::connecting());
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(&shared.ws),
    )
    .await
    .map_err(|_| down(DisconnectReason::Unreachable, "the printer didn't answer"))?
    .map_err(|e| {
        down(
            DisconnectReason::Unreachable,
            format!("the printer's WebSocket didn't open: {e}"),
        )
    })?;
    let (mut tx, mut rx) = socket.split();
    let (outbox, mut outgoing) = mpsc::unbounded_channel::<String>();
    shared.lock().outbox = Some(outbox);
    // The printer pushes its status when it changes; these ask for it now.
    shared.send(cmd::ATTRIBUTES, json!({}));
    shared.send(cmd::STATUS, json!({}));
    let (found, mut discovered) = oneshot::channel();
    if shared.lock().mainboard.is_none() {
        let host = shared.host.clone();
        tokio::spawn(async move {
            let _ = found.send(discover(host).await);
        });
    }
    let mut discovering = true;
    let mut camera_asked = false;
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    heartbeat.tick().await;
    let mut last_heard = tokio::time::Instant::now();
    let broke = |e: tokio_tungstenite::tungstenite::Error| {
        down(
            DisconnectReason::Unreachable,
            format!("the printer's WebSocket broke: {e}"),
        )
    };
    loop {
        tokio::select! {
            message = rx.next() => {
                let message = match message {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Err(broke(e)),
                    None => return Err(down(DisconnectReason::Unreachable, "the printer closed the connection")),
                };
                last_heard = tokio::time::Instant::now();
                let text = match message {
                    Message::Text(t) => t.to_string(),
                    Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                    Message::Close(_) => return Err(down(DisconnectReason::Unreachable, "the printer closed the connection")),
                    _ => continue,
                };
                if text.trim() == "pong" {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    tracing::debug!(printer = %shared.name, "not an SDCP message");
                    continue;
                };
                let learned = shared.on_message(&value);
                let heard = shared.lock().heard;
                if learned && !heard {
                    // Asked again, now addressed to this printer.
                    shared.send(cmd::ATTRIBUTES, json!({}));
                    shared.send(cmd::STATUS, json!({}));
                }
                if heard && !camera_asked {
                    camera_asked = true;
                    tokio::spawn(find_camera(shared.clone()));
                }
            }
            Some(message) = outgoing.recv() => {
                tx.send(Message::Text(message.into())).await.map_err(broke)?;
            }
            found = &mut discovered, if discovering => {
                discovering = false;
                let Ok(Some(id)) = found else { continue };
                let mut state = shared.lock();
                if state.mainboard.is_none() {
                    tracing::info!(printer = %shared.name, "found the printer by UDP discovery");
                    state.mainboard = Some(id);
                    drop(state);
                    shared.send(cmd::ATTRIBUTES, json!({}));
                    shared.send(cmd::STATUS, json!({}));
                }
            }
            _ = heartbeat.tick() => {
                if last_heard.elapsed() > SILENCE {
                    let heard = shared.lock().heard;
                    return Err(down(DisconnectReason::Unreachable, if heard {
                        "the printer stopped answering"
                    } else {
                        "the printer didn't report its status"
                    }));
                }
                // A print held back for its ending is let go in time.
                let held = shared.lock().idle_since.is_some();
                if held {
                    shared.on_status();
                }
                tx.send(Message::Text("ping".into())).await.map_err(broke)?;
            }
        }
    }
}

/// One Elegoo SDCP Printer.
pub struct ElegooSdcpPrinter {
    shared: Arc<Shared>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl ElegooSdcpPrinter {
    fn require(&self, ok: impl Fn(PrinterState) -> bool, what: &str) -> Result<(), CommandError> {
        self.shared.require_connected()?;
        match self.shared.printer_state() {
            Some(state) if ok(state) => Ok(()),
            _ => Err(CommandError::invalid_state(what)),
        }
    }

    async fn control(&self, data: Value) -> Result<(), CommandError> {
        self.shared.require_connected()?;
        self.shared.request(cmd::CONTROL, data).await.map(|_| ())
    }

    /// Sends one chunk of an upload.
    async fn send_chunk(
        &self,
        name: &str,
        chunk: Vec<u8>,
        offset: usize,
        md5: &str,
        uuid: &str,
        total: usize,
    ) -> Result<(), CommandError> {
        let part = reqwest::multipart::Part::bytes(chunk)
            .file_name(name.to_owned())
            .mime_str("application/octet-stream")
            .map_err(|e| CommandError::invalid(e.to_string()))?;
        let form = reqwest::multipart::Form::new()
            .text("S-File-MD5", md5.to_owned())
            .text("Check", "1")
            .text("Offset", offset.to_string())
            .text("Uuid", uuid.to_owned())
            .text("TotalSize", total.to_string())
            .part("File", part);
        let response = self
            .shared
            .client
            .post(format!("{}/uploadFile/upload", self.shared.base))
            .multipart(form)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    CommandError::TimedOut
                } else {
                    CommandError::Disconnected {
                        reason: DisconnectReason::Unreachable,
                    }
                }
            })?;
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status >= 400 || body["success"].as_bool() != Some(true) {
            let detail = body["messages"]
                .as_array()
                .and_then(|m| m.first())
                .map(|m| m["message"].to_string())
                .unwrap_or_else(|| format!("HTTP {status}, code {}", body["code"]));
            return Err(CommandError::Refused {
                message: format!("the printer refused the upload ({detail})"),
            });
        }
        Ok(())
    }
}

/// The MD5 of `data`, in lowercase hex, as `S-File-MD5` carries it.
pub fn md5_hex(data: &[u8]) -> String {
    Md5::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[async_trait]
impl PrinterHandle for ElegooSdcpPrinter {
    async fn pause(&self) -> Result<(), CommandError> {
        self.require(
            |s| matches!(s, PrinterState::Printing | PrinterState::Preparing),
            "nothing is printing",
        )?;
        self.shared.request(cmd::PAUSE, json!({})).await.map(|_| ())
    }

    async fn resume(&self) -> Result<(), CommandError> {
        self.require(|s| s == PrinterState::Paused, "the print is not paused")?;
        self.shared
            .request(cmd::RESUME, json!({}))
            .await
            .map(|_| ())
    }

    async fn cancel(&self) -> Result<(), CommandError> {
        self.require(PrinterState::has_job, "nothing is printing")?;
        self.shared
            .request(cmd::CANCEL, json!({}))
            .await
            .map(|_| ())
    }

    /// Cmd 403, with the payloads community clients send the Centauri
    /// Carbon (unverified against real hardware).
    async fn fine(&self, control: FineControl) -> Result<(), CommandError> {
        match control {
            FineControl::SetTemperature {
                heater: Heater::Nozzle,
                target,
            } => {
                self.control(json!({ "TempTargetNozzle": target.round() as i64 }))
                    .await
            }
            FineControl::SetTemperature {
                heater: Heater::Bed,
                target,
            } => {
                self.control(json!({ "TempTargetHotbed": target.round() as i64 }))
                    .await
            }
            FineControl::SetFan { fan, percent } => {
                let Some((sdcp, _, _)) = status::FANS.iter().find(|(_, id, _)| *id == fan) else {
                    return Err(CommandError::invalid(format!("no fan `{fan}`")));
                };
                self.control(json!({ "TargetFanSpeed": { *sdcp: percent } }))
                    .await
            }
            FineControl::SetLight {
                light: LightId::Chamber,
                on,
            } => {
                self.control(json!({ "LightStatus": { "SecondLight": on } }))
                    .await
            }
            FineControl::SetSpeedProfile { profile } => {
                let Some((_, pct)) = status::SPEED_PROFILES.iter().find(|(p, _)| *p == profile)
                else {
                    return Err(CommandError::invalid(format!(
                        "no speed profile `{profile}`"
                    )));
                };
                self.control(json!({ "PrintSpeedPct": pct })).await
            }
            _ => Err(CommandError::Unsupported),
        }
    }

    /// Uploads the Plate's G-code over HTTP in chunks, each carrying the
    /// whole file's MD5, then starts it with Cmd 128. A `.gcode.3mf` sends
    /// just the Plate's G-code.
    async fn upload_and_start(&self, upload: PrintUpload) -> Result<Option<String>, CommandError> {
        self.require(
            |s| s == PrinterState::Idle,
            "the Printer isn't Idle, so it can't start a Job",
        )?;
        let (name, gcode) = gantry_driver_support::plate::gcode_for(&upload)?;
        let md5 = md5_hex(&gcode);
        let uuid = uuid::Uuid::new_v4().simple().to_string();
        let total = gcode.len();
        let mut offset = 0;
        loop {
            let end = (offset + CHUNK).min(total);
            let sent = self
                .send_chunk(
                    &name,
                    gcode[offset..end].to_vec(),
                    offset,
                    &md5,
                    &uuid,
                    total,
                )
                .await;
            if let Err(e) = sent {
                // Tells the printer to drop what it has of the file.
                self.shared
                    .send(cmd::END_TRANSFER, json!({ "Uuid": uuid, "FileName": name }));
                return Err(e);
            }
            offset = end;
            if offset >= total {
                break;
            }
        }
        tracing::info!(printer = %self.shared.name, file = %name, bytes = total, "sent a Print File");
        // The spec's `Filename` takes a name or a path, a bare name meaning
        // the printer's own storage (`/local/`), so the bare name is sent.
        self.shared
            .request(
                cmd::START_PRINT,
                json!({ "Filename": name, "StartLayer": 0 }),
            )
            .await?;
        // The Fleet takes the next print this Printer reports to be this Job.
        Ok(None)
    }

    fn model(&self) -> Option<String> {
        self.shared.model()
    }

    fn camera(&self) -> Option<Arc<dyn CameraSource>> {
        self.shared
            .lock()
            .camera
            .clone()
            .map(|c| c as Arc<dyn CameraSource>)
    }

    async fn shutdown(&self) {
        if let Some(task) = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_follow_the_spec() {
        let m = request_message("c1", "mb1", "r1", cmd::PAUSE, json!({}));
        assert_eq!(m["Topic"], "sdcp/request/mb1");
        assert_eq!(m["Id"], "c1");
        assert_eq!(m["Data"]["Cmd"], 129);
        assert_eq!(m["Data"]["RequestID"], "r1");
        assert_eq!(m["Data"]["MainboardID"], "mb1");
        assert_eq!(m["Data"]["From"], 0);
        assert!(m["Data"]["TimeStamp"].as_i64().unwrap() > 0);
    }

    #[test]
    fn the_mainboard_id_is_found_wherever_it_sits() {
        assert_eq!(
            mainboard_of(&json!({ "Status": {}, "MainboardID": "a" })).as_deref(),
            Some("a")
        );
        assert_eq!(
            mainboard_of(&json!({ "Data": { "MainboardID": "b" } })).as_deref(),
            Some("b")
        );
        assert_eq!(
            mainboard_of(&json!({ "Topic": "sdcp/status/c", "MainboardID": "" })).as_deref(),
            Some("c")
        );
        assert_eq!(mainboard_of(&json!({ "Topic": "sdcp/request/" })), None);
    }

    #[test]
    fn acks_are_read() {
        assert!(ack(cmd::PAUSE, &json!({ "Ack": 0 })).is_ok());
        assert!(matches!(
            ack(cmd::START_PRINT, &json!({ "Ack": 1 })),
            Err(CommandError::InvalidState { .. })
        ));
        match ack(cmd::START_PRINT, &json!({ "Ack": 7 })) {
            Err(CommandError::Refused { message }) => assert!(message.contains("model")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn md5_is_lowercase_hex() {
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn settings_are_checked() {
        let settings = json!({ "host": "ws://192.168.1.40", "port": 70000 });
        let errors = ElegooSdcpDriver.validate(settings.as_object().unwrap(), &Secrets::default());
        let fields: Vec<_> = errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, ["port", "host"]);
        let errors = ElegooSdcpDriver.validate(&Map::new(), &Secrets::default());
        assert_eq!(errors[0].field, "host");
    }
}
