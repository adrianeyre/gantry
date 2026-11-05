//! The OctoPrint Driver (PRD §10.2): almost any printer, old Marlin machines
//! included, through an OctoPrint server on a Raspberry Pi or similar.
//!
//! Status arrives over OctoPrint's push socket (`/sockjs/websocket`, the
//! raw WebSocket transport), authenticated with a passive login made with the
//! API key. Commands go over REST: pause, resume and Cancel through
//! `/api/job`, fine controls through `/api/printer/*`, and a Print File
//! through `/api/files/local`, which also starts it. A Job's ending is
//! OctoPrint's own `PrintDone`, `PrintFailed` or `PrintCancelled` event.
//! A dropped connection is retried with backoff for as long as the Printer
//! is in the Fleet; OctoPrint answering but not connected to its printer is
//! a lost Connection too.
//!
//! The camera is the one named in the Printer's settings, or else the
//! webcam OctoPrint is set up with, read as MJPEG (or snapshots) over HTTP.
//!
//! Tested against OctoPrint's virtual printer in Docker
//! (`docker/simulators/compose.yaml`). Real cameras are unverified.
use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use gantry_core::{
    Axis, CameraSource, CommandError, Connection, DisconnectReason, Driver, EventSender,
    FieldError, FieldKind, FieldSpec, FineControl, Heater, JobOutcome, PrintUpload, PrinterConfig,
    PrinterHandle, PrinterState, PrinterStatus, Secrets, Temperatures,
};
use gantry_driver_support::mjpeg::MjpegCamera;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

pub mod status;

use status::{Activity, Track};

/// The Driver id stored with each OctoPrint Printer.
pub const DRIVER_ID: &str = "octoprint";

/// OctoPrint's usual port, behind its own web server.
pub const DEFAULT_PORT: u16 = 80;

const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(15);

/// How long without a word from OctoPrint before the connection counts as
/// lost. OctoPrint pushes the printer's state about twice a second.
const SILENCE: Duration = Duration::from_secs(20);

/// How long a print that has gone back to Operational is still held for its
/// ending event, before the Fleet is told it ended unseen.
const ENDING_GRACE: Duration = Duration::from_secs(5);

const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);

/// The OctoPrint Driver.
#[derive(Debug, Clone, Copy, Default)]
pub struct OctoPrintDriver;

impl Driver for OctoPrintDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn name(&self) -> &'static str {
        "OctoPrint"
    }

    fn fields(&self) -> Vec<FieldSpec> {
        vec![
            FieldSpec::new("host", "Address", FieldKind::Text)
                .required()
                .help("The IP address or hostname of the OctoPrint server, such as octopi.local."),
            FieldSpec::new("port", "Port", FieldKind::Number)
                .help("OctoPrint's port: 80 on OctoPi, 5000 when it runs on its own.")
                .default_value(DEFAULT_PORT.into()),
            FieldSpec::new("api_key", "API key", FieldKind::Secret)
                .required()
                .help("An application key from OctoPrint's Settings → Application Keys, or your user's API key."),
            FieldSpec::new("model", "Printer model", FieldKind::Text).help(
                "The printer's model as your slicer names it, such as Ender-3 V2. A Print File is only \
                 ever sent to the model it was sliced for.",
            ),
            FieldSpec::new("camera_url", "Camera URL", FieldKind::Text).help(
                "An MJPEG stream or snapshot URL. Leave it empty to use the webcam OctoPrint is set up with.",
            ),
        ]
    }

    fn validate(
        &self,
        settings: &serde_json::Map<String, Value>,
        secrets: &Secrets,
    ) -> Vec<FieldError> {
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
                message: "Just the address, such as octopi.local, without http:// or a path".into(),
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
            ws: format!("ws://{host}:{port}/sockjs/websocket"),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            api_key: printer
                .secrets
                .get("api_key")
                .unwrap_or_default()
                .to_owned(),
            model: printer.text("model").map(str::to_owned),
            camera_url: printer.text("camera_url").map(str::to_owned),
            events,
            state: Mutex::new(State::default()),
        });
        let task = tokio::spawn(run(shared.clone()));
        Box::new(OctoPrintPrinter {
            shared,
            task: Mutex::new(Some(task)),
        })
    }
}

#[derive(Default)]
struct State {
    current: Value,
    temperatures: Temperatures,
    track: Option<Track>,
    /// When the followed print was first seen back at Operational.
    idle_since: Option<tokio::time::Instant>,
    status: Option<PrinterStatus>,
    camera: Option<Arc<MjpegCamera>>,
}

struct Shared {
    name: String,
    base: String,
    ws: String,
    client: reqwest::Client,
    api_key: String,
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

/// OctoPrint's error message, from `{"error": "…"}`.
fn error_message(body: &Value) -> Option<String> {
    body.get("error")
        .and_then(Value::as_str)
        .map(|m| m.lines().next().unwrap_or(m).to_owned())
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&self.api_key) {
            headers.insert("X-Api-Key", value);
        }
        headers
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, Down> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .headers(self.headers())
            .timeout(Duration::from_secs(10));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|e| {
            down(
                DisconnectReason::Unreachable,
                format!("OctoPrint didn't answer: {e}"),
            )
        })?;
        match response.status().as_u16() {
            401 | 403 => Err(down(
                DisconnectReason::AuthenticationRejected,
                "OctoPrint refused: check the API key",
            )),
            s if s >= 400 => Err(down(
                DisconnectReason::ProtocolError,
                format!("OctoPrint answered {s}"),
            )),
            _ => Ok(response.json().await.unwrap_or(Value::Null)),
        }
    }

    /// A command over REST. A slow answer is taken as accepted when `slow_ok`.
    async fn command(&self, path: &str, body: Value, slow_ok: bool) -> Result<(), CommandError> {
        let sent = self
            .client
            .post(format!("{}{path}", self.base))
            .headers(self.headers())
            .json(&body)
            .timeout(COMMAND_TIMEOUT)
            .send()
            .await;
        let response = match sent {
            Ok(r) => r,
            Err(e) if e.is_timeout() && slow_ok => return Ok(()),
            Err(e) if e.is_timeout() => return Err(CommandError::TimedOut),
            Err(_) => {
                return Err(CommandError::Disconnected {
                    reason: DisconnectReason::Unreachable,
                });
            }
        };
        let status = response.status().as_u16();
        if status < 400 {
            return Ok(());
        }
        let body: Value = response.json().await.unwrap_or(Value::Null);
        let message =
            error_message(&body).unwrap_or_else(|| format!("OctoPrint answered {status}"));
        Err(match status {
            401 | 403 => CommandError::Refused {
                message: "OctoPrint refused the API key".into(),
            },
            409 => CommandError::invalid_state(message),
            _ => CommandError::Refused { message },
        })
    }

    fn report(&self, status: PrinterStatus) {
        self.lock().status = Some(status.clone());
        self.events.status(status);
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

    /// Takes in a `current` (or `history`) message.
    fn on_current(&self, current: &Value) {
        let mut state = self.lock();
        state.current = current.clone();
        if let Some(t) = status::temperatures(current) {
            state.temperatures = t;
        }
        let activity = status::activity(current);
        match activity {
            Activity::Printing | Activity::Paused => {
                state.idle_since = None;
                if let Some(file) = status::file(current)
                    && state.track.as_ref().is_none_or(|t| t.file != file)
                {
                    let now = time::OffsetDateTime::now_utc();
                    let started = now
                        - time::Duration::seconds(
                            status::print_time(current).unwrap_or(0.0).round() as i64,
                        );
                    let track = Track {
                        key: format!("{file}@{}", started.unix_timestamp()),
                        file,
                        started_at: started,
                    };
                    tracing::info!(printer = %self.name, key = %track.key, "a print is running");
                    state.track = Some(track);
                }
            }
            Activity::Idle | Activity::Offline if state.track.is_some() => {
                // Its ending event may be a moment behind: wait for it.
                let since = *state
                    .idle_since
                    .get_or_insert_with(tokio::time::Instant::now);
                if since.elapsed() < ENDING_GRACE {
                    return;
                }
                tracing::info!(printer = %self.name, "a print ended without OctoPrint saying how");
                state.track = None;
                state.idle_since = None;
            }
            Activity::Idle | Activity::Offline => {}
        }
        let camera = state.camera.is_some();
        let status = status::status(current, state.temperatures, state.track.as_ref(), camera);
        state.status = Some(status.clone());
        drop(state);
        self.events.status(status);
    }

    /// Takes in an `event` message: a print's ending.
    fn on_event(&self, event: &Value) {
        let outcome = match event["type"].as_str() {
            Some("PrintDone") => JobOutcome::Succeeded,
            Some("PrintFailed") => JobOutcome::Failed,
            Some("PrintCancelled") => JobOutcome::Cancelled,
            _ => return,
        };
        let mut state = self.lock();
        let Some(track) = state.track.take() else {
            return;
        };
        state.idle_since = None;
        tracing::info!(printer = %self.name, key = %track.key, outcome = outcome.as_str(), "a print ended");
        self.events.job_ended(track.key, outcome);
        let current = state.current.clone();
        let mut status = status::status(&current, state.temperatures, None, state.camera.is_some());
        // The print is over even if the last push still says printing.
        if status.state.is_some_and(PrinterState::has_job) {
            status.state = Some(PrinterState::Idle);
        }
        state.status = Some(status.clone());
        drop(state);
        self.events.status(status);
    }
}

/// An address OctoPrint names, made absolute against its own.
fn absolute(base: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_owned()
    } else if url.starts_with('/') {
        format!("{base}{url}")
    } else {
        format!("{base}/{url}")
    }
}

async fn find_camera(shared: &Shared) -> Option<Arc<MjpegCamera>> {
    let url = match &shared.camera_url {
        Some(url) => absolute(&shared.base, url),
        None => {
            let settings = shared
                .request(reqwest::Method::GET, "/api/settings", None)
                .await
                .ok()?;
            let classic = &settings["plugins"]["classicwebcam"];
            let enabled = settings["webcam"]["webcamEnabled"]
                .as_bool()
                .unwrap_or(true);
            let url = [
                &classic["stream"],
                &settings["webcam"]["streamUrl"],
                &classic["snapshot"],
            ]
            .into_iter()
            .find_map(|v| v.as_str().filter(|u| !u.trim().is_empty()))?;
            if !enabled {
                return None;
            }
            absolute(&shared.base, url)
        }
    };
    Some(Arc::new(MjpegCamera::new(url, shared.headers())))
}

async fn run(shared: Arc<Shared>) {
    let mut backoff = BACKOFF_MIN;
    loop {
        if shared.events.is_closed() {
            return;
        }
        let started = tokio::time::Instant::now();
        if let Err(Down { reason, detail }) = session(&shared).await {
            tracing::info!(printer = %shared.name, reason = reason.describe(), %detail, "OctoPrint connection down");
            shared.report(PrinterStatus {
                connection: Connection::Disconnected {
                    reason,
                    detail: Some(detail),
                },
                ..PrinterStatus::disconnected(reason)
            });
            if started.elapsed() > Duration::from_secs(30) {
                backoff = BACKOFF_MIN;
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

async fn session(shared: &Shared) -> Result<(), Down> {
    shared.events.status(PrinterStatus::connecting());
    shared
        .request(reqwest::Method::GET, "/api/version", None)
        .await?;
    let login = shared
        .request(
            reqwest::Method::POST,
            "/api/login",
            Some(json!({ "passive": true })),
        )
        .await?;
    let (Some(name), Some(session)) = (login["name"].as_str(), login["session"].as_str()) else {
        return Err(down(
            DisconnectReason::ProtocolError,
            "OctoPrint's login gave no session",
        ));
    };
    let camera = find_camera(shared).await;
    shared.lock().camera = camera;
    let (socket, _) = tokio_tungstenite::connect_async(&shared.ws)
        .await
        .map_err(|e| {
            down(
                DisconnectReason::Unreachable,
                format!("the push socket didn't open: {e}"),
            )
        })?;
    let (mut tx, mut rx) = socket.split();
    tx.send(Message::Text(
        json!({ "auth": format!("{name}:{session}") })
            .to_string()
            .into(),
    ))
    .await
    .map_err(|e| {
        down(
            DisconnectReason::Unreachable,
            format!("the push socket broke: {e}"),
        )
    })?;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    let mut last_heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            message = rx.next() => {
                let message = match message {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Err(down(DisconnectReason::Unreachable, format!("the push socket broke: {e}"))),
                    None => return Err(down(DisconnectReason::Unreachable, "OctoPrint closed the connection")),
                };
                last_heard = tokio::time::Instant::now();
                let text = match message {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => return Err(down(DisconnectReason::Unreachable, "OctoPrint closed the connection")),
                    _ => continue,
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
                if let Some(current) = value.get("current").or_else(|| value.get("history")) {
                    shared.on_current(current);
                } else if let Some(event) = value.get("event") {
                    shared.on_event(event);
                } else if value.get("reauthRequired").is_some() {
                    return Ok(());
                }
            }
            _ = heartbeat.tick() => {
                if last_heard.elapsed() > SILENCE {
                    return Err(down(DisconnectReason::Unreachable, "OctoPrint stopped answering"));
                }
                // A print held back for its ending event is let go in time.
                let held = shared.lock().idle_since.is_some();
                if held {
                    let current = shared.lock().current.clone();
                    shared.on_current(&current);
                }
                if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return Err(down(DisconnectReason::Unreachable, "the push socket broke"));
                }
            }
        }
    }
}

/// One OctoPrint Printer.
pub struct OctoPrintPrinter {
    shared: Arc<Shared>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl OctoPrintPrinter {
    fn require(&self, ok: impl Fn(PrinterState) -> bool, what: &str) -> Result<(), CommandError> {
        self.shared.require_connected()?;
        match self.shared.printer_state() {
            Some(state) if ok(state) => Ok(()),
            _ => Err(CommandError::invalid_state(what)),
        }
    }
}

#[async_trait]
impl PrinterHandle for OctoPrintPrinter {
    async fn pause(&self) -> Result<(), CommandError> {
        self.require(
            |s| matches!(s, PrinterState::Printing | PrinterState::Preparing),
            "nothing is printing",
        )?;
        self.shared
            .command(
                "/api/job",
                json!({ "command": "pause", "action": "pause" }),
                true,
            )
            .await
    }

    async fn resume(&self) -> Result<(), CommandError> {
        self.require(|s| s == PrinterState::Paused, "the print is not paused")?;
        self.shared
            .command(
                "/api/job",
                json!({ "command": "pause", "action": "resume" }),
                true,
            )
            .await
    }

    async fn cancel(&self) -> Result<(), CommandError> {
        self.require(PrinterState::has_job, "nothing is printing")?;
        self.shared
            .command("/api/job", json!({ "command": "cancel" }), true)
            .await
    }

    async fn fine(&self, control: FineControl) -> Result<(), CommandError> {
        self.shared.require_connected()?;
        match control {
            FineControl::SetTemperature {
                heater: Heater::Nozzle,
                target,
            } => {
                self.shared
                    .command(
                        "/api/printer/tool",
                        json!({ "command": "target", "targets": { "tool0": target.round() } }),
                        false,
                    )
                    .await
            }
            FineControl::SetTemperature {
                heater: Heater::Bed,
                target,
            } => {
                self.shared
                    .command(
                        "/api/printer/bed",
                        json!({ "command": "target", "target": target.round() }),
                        false,
                    )
                    .await
            }
            FineControl::Home => {
                self.require(|s| !s.has_job(), "can't home while a Job is running")?;
                self.shared
                    .command(
                        "/api/printer/printhead",
                        json!({ "command": "home", "axes": ["x", "y", "z"] }),
                        true,
                    )
                    .await
            }
            FineControl::Jog { axis, distance_mm } => {
                self.require(|s| !s.has_job(), "can't jog while a Job is running")?;
                let axis = match axis {
                    Axis::X => "x",
                    Axis::Y => "y",
                    Axis::Z => "z",
                };
                self.shared
                    .command(
                        "/api/printer/printhead",
                        json!({ "command": "jog", axis: distance_mm }),
                        true,
                    )
                    .await
            }
            _ => Err(CommandError::Unsupported),
        }
    }

    /// Uploads the Plate's G-code to OctoPrint's local storage, selects it
    /// and starts it (PRD §10.2). A `.gcode.3mf` sends just the Plate's G-code.
    async fn upload_and_start(&self, upload: PrintUpload) -> Result<Option<String>, CommandError> {
        self.require(
            |s| s == PrinterState::Idle,
            "the Printer isn't Idle, so it can't start a Job",
        )?;
        let (name, gcode) = gantry_driver_support::plate::gcode_for(&upload)?;
        let part = reqwest::multipart::Part::bytes(gcode)
            .file_name(name.clone())
            .mime_str("application/octet-stream")
            .map_err(|e| CommandError::invalid(e.to_string()))?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("select", "true")
            .text("print", "true");
        let response = self
            .shared
            .client
            .post(format!("{}/api/files/local", self.shared.base))
            .headers(self.shared.headers())
            .multipart(form)
            .timeout(gantry_core::UPLOAD_TIMEOUT)
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
        if status >= 400 {
            let body: Value = response.json().await.unwrap_or(Value::Null);
            let message =
                error_message(&body).unwrap_or_else(|| format!("OctoPrint answered {status}"));
            return Err(if status == 409 {
                CommandError::invalid_state(message)
            } else {
                CommandError::Refused { message }
            });
        }
        tracing::info!(printer = %self.shared.name, file = %name, "sent a Print File");
        // The Fleet takes the next print this Printer reports to be this Job.
        Ok(None)
    }

    fn model(&self) -> Option<String> {
        self.shared.model.clone()
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
    fn addresses_are_made_absolute() {
        assert_eq!(
            absolute("http://octopi:80", "/webcam/?action=stream"),
            "http://octopi:80/webcam/?action=stream"
        );
        assert_eq!(
            absolute("http://octopi:80", "http://cam:8080/x"),
            "http://cam:8080/x"
        );
    }

    #[test]
    fn settings_are_checked() {
        let settings = json!({ "host": "octopi.local", "port": 0 });
        let errors = OctoPrintDriver.validate(settings.as_object().unwrap(), &Secrets::default());
        let fields: Vec<_> = errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, ["api_key", "port"]);
    }

    #[test]
    fn errors_are_read_from_octoprint() {
        assert_eq!(
            error_message(&json!({ "error": "Printer is not operational" })).as_deref(),
            Some("Printer is not operational")
        );
    }
}
