//! The Moonraker Driver (PRD §10.2): Klipper printers (Voron, Creality K1,
//! Sovol, Qidi, Elegoo Neptune 4, DIY builds) through Moonraker's API.
//!
//! Status comes over Moonraker's WebSocket: the Driver subscribes to
//! Klipper's printer objects and folds each update into the normalised
//! status model ([`status`]). Commands go over REST: pause, resume and
//! Cancel through `/printer/print/*`, fine controls as G-code through
//! `/printer/gcode/script`, and a Print File through `/server/files/upload`,
//! which also starts it. A dropped connection is retried with backoff for as
//! long as the Printer is in the Fleet, and Klipper itself shutting down is a
//! Fault, not a lost Connection.
//!
//! A print's key is Moonraker's own job id from its History, so a print is
//! recognised again after Gantry restarts or reconnects.
//!
//! The camera is the one named in the Printer's settings, or else the first
//! webcam Moonraker lists, read as MJPEG (or snapshots) over HTTP.
//!
//! Tested against the simulated-Klipper container (Klipper with simulavr and
//! Moonraker, see `docs/processes/testing-drivers-with-simulators.md`). The
//! camera was tested against that container's test-pattern streamer only:
//! real cameras (crowsnest, ustreamer, WebRTC-only setups) are unverified.
use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use gantry_core::{
    Axis, CameraSource, CommandError, Connection, DisconnectReason, Driver, EventSender,
    FieldError, FieldKind, FieldSpec, FineControl, Heater, PrintUpload, PrinterConfig,
    PrinterHandle, PrinterState, PrinterStatus, Secrets,
};
use gantry_driver_support::mjpeg::MjpegCamera;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

pub mod status;

use status::{Context, Objects, PrintState, Track};

/// The Driver id stored with each Moonraker Printer.
pub const DRIVER_ID: &str = "moonraker";

/// Moonraker's default port.
pub const DEFAULT_PORT: u16 = 7125;

const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(15);

/// How long without a word from Moonraker before the connection counts as
/// lost. Moonraker sends temperatures every second.
const SILENCE: Duration = Duration::from_secs(20);

/// How long a command's request may take. Klipper finishes the moves it has
/// queued before it pauses or cancels, so a slow answer still means the
/// command was taken: it shows in the status when it happens.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);

/// The Moonraker Driver.
#[derive(Debug, Clone, Copy, Default)]
pub struct MoonrakerDriver;

impl Driver for MoonrakerDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn name(&self) -> &'static str {
        "Moonraker (Klipper)"
    }

    fn fields(&self) -> Vec<FieldSpec> {
        vec![
            FieldSpec::new("host", "Address", FieldKind::Text)
                .required()
                .help("The IP address or hostname Moonraker answers on, such as 192.168.1.50."),
            FieldSpec::new("port", "Port", FieldKind::Number)
                .help("Moonraker's port, usually 7125.")
                .default_value(DEFAULT_PORT.into()),
            FieldSpec::new("model", "Printer model", FieldKind::Text).help(
                "The printer's model as your slicer names it, such as Voron 2.4. Klipper can't say, \
                 and a Print File is only ever sent to the model it was sliced for.",
            ),
            FieldSpec::new("camera_url", "Camera URL", FieldKind::Text).help(
                "An MJPEG stream or snapshot URL. Leave it empty to use the first webcam Moonraker lists.",
            ),
            FieldSpec::new("api_key", "API key", FieldKind::Secret).help(
                "Only if Moonraker requires one (its [authorization] section): Settings → API key in Mainsail or Fluidd.",
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
                message: "Just the address, such as 192.168.1.50, without http:// or a path".into(),
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
            host: host.clone(),
            base: format!("http://{host}:{port}"),
            ws: format!("ws://{host}:{port}/websocket"),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            api_key: printer.secrets.get("api_key").map(str::to_owned),
            model: printer.text("model").map(str::to_owned),
            camera_url: printer.text("camera_url").map(str::to_owned),
            events,
            state: Mutex::new(State::default()),
        });
        let task = tokio::spawn(run(shared.clone()));
        Box::new(MoonrakerPrinter {
            shared,
            task: Mutex::new(Some(task)),
        })
    }
}

/// What the connection keeps between updates.
#[derive(Default)]
struct State {
    objects: Objects,
    context: Context,
    status: Option<PrinterStatus>,
    camera: Option<Arc<MjpegCamera>>,
}

struct Shared {
    name: String,
    host: String,
    base: String,
    ws: String,
    client: reqwest::Client,
    api_key: Option<String>,
    model: Option<String>,
    camera_url: Option<String>,
    events: EventSender,
    state: Mutex<State>,
}

/// Why a session ended.
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

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(key) = &self.api_key
            && let Ok(value) = reqwest::header::HeaderValue::from_str(key)
        {
            headers.insert("X-Api-Key", value);
        }
        headers
    }

    /// A GET for the connection, with its failures as reasons for being down.
    async fn fetch(&self, path: &str) -> Result<Value, Down> {
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .headers(self.headers())
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| {
                down(
                    DisconnectReason::Unreachable,
                    format!("Moonraker didn't answer: {e}"),
                )
            })?;
        match response.status().as_u16() {
            401 | 403 => {
                return Err(down(
                    DisconnectReason::AuthenticationRejected,
                    "Moonraker refused: check the API key",
                ));
            }
            s if s >= 400 => {
                let body: Value = response.json().await.unwrap_or(Value::Null);
                return Err(down(
                    DisconnectReason::ProtocolError,
                    error_message(&body).unwrap_or_else(|| format!("Moonraker answered {s}")),
                ));
            }
            _ => {}
        }
        let body: Value = response.json().await.map_err(|_| {
            down(
                DisconnectReason::ProtocolError,
                "Moonraker's answer wasn't JSON",
            )
        })?;
        Ok(body.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Sends a command. A slow answer is taken as accepted when `slow_ok`.
    async fn command(
        &self,
        path: &str,
        query: &[(&str, String)],
        slow_ok: bool,
    ) -> Result<(), CommandError> {
        let sent = self
            .client
            .post(format!("{}{path}", self.base))
            .headers(self.headers())
            .query(query)
            .timeout(COMMAND_TIMEOUT)
            .send()
            .await;
        let response = match sent {
            Ok(r) => r,
            Err(e) if e.is_timeout() && slow_ok => {
                tracing::debug!(printer = %self.name, path, "Moonraker is still carrying out a command");
                return Ok(());
            }
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
        Err(match status {
            401 | 403 => CommandError::Refused {
                message: "Moonraker refused the API key".into(),
            },
            _ => CommandError::Refused {
                message: error_message(&body)
                    .unwrap_or_else(|| format!("Moonraker answered {status}")),
            },
        })
    }

    async fn gcode(&self, script: &str, slow_ok: bool) -> Result<(), CommandError> {
        self.command(
            "/printer/gcode/script",
            &[("script", script.to_owned())],
            slow_ok,
        )
        .await
    }

    /// Publishes the status from the objects, and any ending first.
    fn publish(&self, ended: Option<(String, gantry_core::JobOutcome)>) {
        let mut state = self.lock();
        if let Some((key, outcome)) = ended {
            self.events.job_ended(key, outcome);
        }
        let status = status::status(&state.objects, &state.context);
        state.status = Some(status.clone());
        drop(state);
        self.events.status(status);
    }

    fn report_down(&self, reason: DisconnectReason, detail: Option<String>) {
        let status = PrinterStatus {
            connection: Connection::Disconnected { reason, detail },
            ..PrinterStatus::disconnected(reason)
        };
        self.lock().status = Some(status.clone());
        self.events.status(status);
    }

    fn print_state(&self) -> Option<PrinterState> {
        let state = self.lock();
        state
            .status
            .as_ref()
            .filter(|s| s.connection.is_connected())
            .and_then(|s| s.state)
    }

    fn require_connected(&self) -> Result<(), CommandError> {
        let state = self.lock();
        match state.status.as_ref().map(|s| &s.connection) {
            Some(Connection::Connected) => Ok(()),
            Some(Connection::Disconnected { reason, .. }) => {
                Err(CommandError::Disconnected { reason: *reason })
            }
            _ => Err(CommandError::invalid_state(
                "the Printer is still connecting",
            )),
        }
    }
}

/// Moonraker's error message, from `{"error": {"message": …}}`.
fn error_message(body: &Value) -> Option<String> {
    body.get("error")?
        .get("message")?
        .as_str()
        .map(|m| m.lines().next().unwrap_or(m).to_owned())
}

/// The connection's task: sessions one after another, with backoff.
async fn run(shared: Arc<Shared>) {
    let mut backoff = BACKOFF_MIN;
    loop {
        if shared.events.is_closed() {
            return;
        }
        let started = tokio::time::Instant::now();
        match session(&shared).await {
            Ok(()) => backoff = BACKOFF_MIN,
            Err(Down { reason, detail }) => {
                tracing::info!(printer = %shared.name, reason = reason.describe(), %detail, "Moonraker connection down");
                shared.report_down(reason, Some(detail));
                if started.elapsed() > Duration::from_secs(30) {
                    backoff = BACKOFF_MIN;
                }
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// The address of a webcam stream Moonraker lists, made absolute.
fn absolute(host: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_owned()
    } else if url.starts_with('/') {
        // Relative to the web front end (Mainsail, Fluidd) on port 80.
        format!("http://{host}{url}")
    } else {
        format!("http://{host}/{url}")
    }
}

async fn find_camera(shared: &Shared) -> Option<Arc<MjpegCamera>> {
    let url = match &shared.camera_url {
        Some(url) => absolute(&shared.host, url),
        None => {
            let list = shared.fetch("/server/webcams/list").await.ok()?;
            let webcam = list
                .get("webcams")?
                .as_array()?
                .iter()
                .find(|w| w.get("enabled").and_then(Value::as_bool) != Some(false))?;
            let stream = webcam
                .get("stream_url")
                .and_then(Value::as_str)
                .filter(|u| !u.is_empty())
                .or_else(|| webcam.get("snapshot_url").and_then(Value::as_str))?;
            absolute(&shared.host, stream)
        }
    };
    Some(Arc::new(MjpegCamera::new(url, shared.headers())))
}

/// The key for the print running now: Moonraker's job id when its History
/// has it, else the file and when it started.
async fn track_for(shared: &Shared, filename: &str, total_duration: f64) -> Track {
    let history = shared
        .fetch("/server/history/list?limit=1&order=desc")
        .await
        .ok();
    let job = history
        .as_ref()
        .and_then(|h| h.get("jobs"))
        .and_then(Value::as_array)
        .and_then(|jobs| jobs.first())
        .filter(|j| j.get("filename").and_then(Value::as_str) == Some(filename))
        .filter(|j| j.get("status").and_then(Value::as_str) == Some("in_progress"));
    let now = time::OffsetDateTime::now_utc();
    match job.and_then(|j| Some((j.get("job_id")?.as_str()?, j.get("start_time")?.as_f64()?))) {
        Some((id, start)) => Track {
            key: format!("moonraker-{id}"),
            filename: filename.to_owned(),
            started_at: time::OffsetDateTime::from_unix_timestamp(start as i64).unwrap_or(now),
        },
        None => {
            let started = now - time::Duration::seconds(total_duration.round() as i64);
            Track {
                key: format!("{filename}@{}", started.unix_timestamp()),
                filename: filename.to_owned(),
                started_at: started,
            }
        }
    }
}

/// Folds in the objects' latest state: starts following a print that has
/// appeared, and reports one that has ended.
async fn follow(shared: &Shared) {
    let (print, filename, total) = {
        let state = shared.lock();
        (
            status::print_state(&state.objects),
            status::filename(&state.objects),
            status::total_duration(&state.objects),
        )
    };
    let current = shared.lock().context.track.clone();
    let mut ended = None;
    match print {
        PrintState::Printing | PrintState::Paused => {
            if let Some(filename) = filename
                && current.as_ref().is_none_or(|t| t.filename != filename)
            {
                let track = track_for(shared, &filename, total).await;
                tracing::info!(printer = %shared.name, key = %track.key, "a print is running");
                shared.lock().context.track = Some(track);
            }
        }
        PrintState::Ended(outcome) => {
            if let Some(track) = shared.lock().context.track.take() {
                ended = Some((track.key, outcome));
            }
        }
        PrintState::Standby => {
            // Klipper restarted mid-print: the ending wasn't seen, and the
            // Fleet records the Job as Lost.
            shared.lock().context.track = None;
        }
    }
    shared.publish(ended);
}

/// One connection: check the server, subscribe to the printer objects, and
/// follow updates until something goes wrong.
async fn session(shared: &Shared) -> Result<(), Down> {
    shared.events.status(PrinterStatus::connecting());
    let info = shared.fetch("/server/info").await?;
    if info.get("klippy_connected").and_then(Value::as_bool) == Some(false) {
        return Err(down(
            DisconnectReason::Unreachable,
            "Moonraker is running, but can't reach Klipper",
        ));
    }
    let klippy_state = info
        .get("klippy_state")
        .and_then(Value::as_str)
        .unwrap_or("ready")
        .to_owned();
    if klippy_state == "startup" {
        tokio::time::sleep(Duration::from_secs(2)).await;
        return Ok(());
    }
    let mut url = shared.ws.clone();
    if shared.api_key.is_some() {
        let token = shared.fetch("/access/oneshot_token").await?;
        if let Some(token) = token.as_str() {
            url = format!("{url}?token={token}");
        }
    }
    let (socket, _) = tokio_tungstenite::connect_async(&url).await.map_err(|e| {
        down(
            DisconnectReason::Unreachable,
            format!("the WebSocket didn't open: {e}"),
        )
    })?;
    let (mut tx, mut rx) = socket.split();

    let ready = klippy_state == "ready";
    let names: Vec<String> = if ready {
        shared
            .fetch("/printer/objects/list")
            .await
            .ok()
            .and_then(|l| l.get("objects").cloned())
            .and_then(|o| serde_json::from_value(o).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let message = if ready {
        None
    } else {
        shared.fetch("/printer/info").await.ok().and_then(|i| {
            i.get("state_message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    };
    let camera = find_camera(shared).await;
    {
        let mut state = shared.lock();
        state.context.klippy_ready = ready;
        state.context.klippy_message = message;
        state.context.camera = camera.is_some();
        state.camera = camera;
        state.objects.clear();
    }
    if ready {
        let mut wanted = serde_json::Map::new();
        for name in [
            "print_stats",
            "virtual_sdcard",
            "display_status",
            "extruder",
            "heater_bed",
            "fan",
            "toolhead",
            "webhooks",
            "idle_timeout",
            "pause_resume",
        ] {
            if names.is_empty() || names.iter().any(|n| n == name) {
                wanted.insert(name.into(), Value::Null);
            }
        }
        for name in &names {
            if name.starts_with("filament_switch_sensor ")
                || name.starts_with("filament_motion_sensor ")
                || status::chamber_object(std::slice::from_ref(name)).is_some()
            {
                wanted.insert(name.clone(), Value::Null);
            }
        }
        let subscribe = json!({
            "jsonrpc": "2.0",
            "method": "printer.objects.subscribe",
            "params": { "objects": wanted },
            "id": 1,
        });
        tx.send(Message::Text(subscribe.to_string().into()))
            .await
            .map_err(|e| {
                down(
                    DisconnectReason::Unreachable,
                    format!("the WebSocket broke: {e}"),
                )
            })?;
    } else {
        // Klipper is shut down or in error: connected, with a Fault.
        follow(shared).await;
    }

    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    let mut last_heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            message = rx.next() => {
                let message = match message {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Err(down(DisconnectReason::Unreachable, format!("the WebSocket broke: {e}"))),
                    None => return Err(down(DisconnectReason::Unreachable, "Moonraker closed the connection")),
                };
                last_heard = tokio::time::Instant::now();
                let text = match message {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => return Err(down(DisconnectReason::Unreachable, "Moonraker closed the connection")),
                    _ => continue,
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
                if value.get("id").and_then(Value::as_i64) == Some(1) {
                    if let Some(objects) = value.get("result").and_then(|r| r.get("status")).and_then(Value::as_object) {
                        status::merge(&mut shared.lock().objects, objects);
                        follow(shared).await;
                    } else if let Some(e) = value.get("error") {
                        return Err(down(DisconnectReason::ProtocolError, format!("Moonraker refused the subscription: {e}")));
                    }
                    continue;
                }
                match value.get("method").and_then(Value::as_str) {
                    Some("notify_status_update") => {
                        if let Some(update) = value.get("params").and_then(|p| p.get(0)).and_then(Value::as_object) {
                            status::merge(&mut shared.lock().objects, update);
                            follow(shared).await;
                        }
                    }
                    // Klipper changed state: start again, to see it as it is now.
                    Some("notify_klippy_ready" | "notify_klippy_shutdown" | "notify_klippy_disconnected") => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
            _ = heartbeat.tick() => {
                if last_heard.elapsed() > SILENCE {
                    return Err(down(DisconnectReason::Unreachable, "Moonraker stopped answering"));
                }
                if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return Err(down(DisconnectReason::Unreachable, "the WebSocket broke"));
                }
            }
        }
    }
}

/// One Moonraker Printer.
pub struct MoonrakerPrinter {
    shared: Arc<Shared>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl MoonrakerPrinter {
    fn require(&self, ok: impl Fn(PrinterState) -> bool, what: &str) -> Result<(), CommandError> {
        self.shared.require_connected()?;
        match self.shared.print_state() {
            Some(state) if ok(state) => Ok(()),
            _ => Err(CommandError::invalid_state(what)),
        }
    }
}

fn has_job(state: PrinterState) -> bool {
    state.has_job()
}

#[async_trait]
impl PrinterHandle for MoonrakerPrinter {
    async fn pause(&self) -> Result<(), CommandError> {
        self.require(
            |s| matches!(s, PrinterState::Printing | PrinterState::Preparing),
            "nothing is printing",
        )?;
        self.shared.command("/printer/print/pause", &[], true).await
    }

    async fn resume(&self) -> Result<(), CommandError> {
        self.require(|s| s == PrinterState::Paused, "the print is not paused")?;
        self.shared
            .command("/printer/print/resume", &[], true)
            .await
    }

    async fn cancel(&self) -> Result<(), CommandError> {
        self.require(has_job, "nothing is printing")?;
        self.shared
            .command("/printer/print/cancel", &[], true)
            .await
    }

    async fn fine(&self, control: FineControl) -> Result<(), CommandError> {
        self.shared.require_connected()?;
        match control {
            FineControl::SetTemperature { heater, target } => {
                // Klipper's own command, not M104/M140, which many configs
                // turn into macros of their own.
                let name = match heater {
                    Heater::Nozzle => "extruder",
                    Heater::Bed => "heater_bed",
                };
                self.shared
                    .gcode(
                        &format!("SET_HEATER_TEMPERATURE HEATER={name} TARGET={target:.0}"),
                        false,
                    )
                    .await
            }
            FineControl::SetFan { fan, percent } => {
                if fan != "part" {
                    return Err(CommandError::invalid(format!("no fan `{fan}`")));
                }
                let value = (f64::from(percent.min(100)) * 255.0 / 100.0).round();
                self.shared.gcode(&format!("M106 S{value:.0}"), false).await
            }
            FineControl::Home => {
                self.require(|s| !s.has_job(), "can't home while a Job is running")?;
                self.shared.gcode("G28", true).await
            }
            FineControl::Jog { axis, distance_mm } => {
                self.require(|s| !s.has_job(), "can't jog while a Job is running")?;
                let (letter, feed) = match axis {
                    Axis::X => ('x', 6000),
                    Axis::Y => ('y', 6000),
                    Axis::Z => ('z', 600),
                };
                let homed = status::homed(&self.shared.lock().objects);
                if !homed.contains(letter) {
                    return Err(CommandError::invalid_state(
                        "home the Printer before jogging",
                    ));
                }
                let axis = letter.to_ascii_uppercase();
                self.shared
                    .gcode(
                        &format!("G91\nG1 {axis}{distance_mm:.2} F{feed}\nG90"),
                        true,
                    )
                    .await
            }
            _ => Err(CommandError::Unsupported),
        }
    }

    /// Uploads the Plate's G-code to Moonraker's `gcodes` folder and starts
    /// it (PRD §10.2). A `.gcode.3mf` sends just the Plate's G-code.
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
            .text("root", "gcodes")
            .text("print", "true")
            .part("file", part);
        let response = self
            .shared
            .client
            .post(format!("{}/server/files/upload", self.shared.base))
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
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status >= 400 {
            return Err(CommandError::Refused {
                message: error_message(&body)
                    .unwrap_or_else(|| format!("Moonraker answered {status}")),
            });
        }
        if body.get("print_started").and_then(Value::as_bool) == Some(false) {
            return Err(CommandError::Refused {
                message: format!("Moonraker took {name} but didn't start it"),
            });
        }
        tracing::info!(printer = %self.shared.name, file = %name, "sent a Print File");
        // Moonraker's job id is known once the print shows; the Fleet takes
        // the next print this Printer reports to be this Job.
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
    fn webcam_addresses_are_made_absolute() {
        assert_eq!(
            absolute("voron.local", "/webcam/?action=stream"),
            "http://voron.local/webcam/?action=stream"
        );
        assert_eq!(
            absolute("voron.local", "http://cam:8080/stream"),
            "http://cam:8080/stream"
        );
        assert_eq!(
            absolute("10.0.0.5", "webcam/snapshot"),
            "http://10.0.0.5/webcam/snapshot"
        );
    }

    #[test]
    fn settings_are_checked() {
        let settings = json!({ "host": "http://voron/", "port": 70000 });
        let errors = MoonrakerDriver.validate(settings.as_object().unwrap(), &Secrets::default());
        let fields: Vec<_> = errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, ["port", "host"]);
        let fine = json!({ "host": "192.168.1.50" });
        assert!(
            MoonrakerDriver
                .validate(fine.as_object().unwrap(), &Secrets::default())
                .is_empty()
        );
    }

    #[test]
    fn errors_are_read_from_moonraker() {
        let body =
            json!({ "error": { "code": 400, "message": "Must home axis first: 0.000\nmore" } });
        assert_eq!(
            error_message(&body).as_deref(),
            Some("Must home axis first: 0.000")
        );
    }
}
