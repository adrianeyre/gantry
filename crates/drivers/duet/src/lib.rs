//! The Duet Driver (PRD §10.2): Duet-based machines running RepRapFirmware 3
//! in standalone mode, through its HTTP API (the `rr_` requests Duet Web
//! Control uses).
//!
//! The Driver opens a session with `rr_connect` (with the board's password,
//! if it has one), then polls the object model with `rr_model` about once a
//! second and folds it into the normalised status model ([`status`]).
//! Commands are G-code through `rr_gcode`: `M25` pauses, `M24` resumes, and
//! Cancel pauses first if need be and then sends `M0`, which is how
//! RepRapFirmware cancels a paused print. Fine controls are G-code too:
//! `M104`/`M140` for temperatures, `M106` for fans, `G28` for homing and a
//! relative `G1` for jogging. A Plate's G-code is sent with `rr_upload` to
//! `0:/gcodes/` and started with `M32`. A print's ending is read from the
//! object model's `job.lastFileName`, `lastFileCancelled` and
//! `lastFileAborted` (Succeeded, Cancelled, Failed). A dropped connection is
//! retried with backoff for as long as the Printer is in the Fleet, and an
//! expired session is opened again.
//!
//! A Duet in SBC mode (with a Raspberry Pi running DSF) speaks a different
//! API and isn't covered. There is no camera: RepRapFirmware has none of its
//! own.
//!
//! **Unverified against a real printer.** It is tested against a mock HTTP
//! server written from Duet3D's documentation of the HTTP API and object
//! model (`tests/mock`), and stays unproven until a Duet board is available.
use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use gantry_core::{
    Axis, CommandError, Connection, DisconnectReason, Driver, EventSender, FieldError, FieldKind,
    FieldSpec, FineControl, Heater, PrintUpload, PrinterConfig, PrinterHandle, PrinterState,
    PrinterStatus, Secrets,
};
use serde_json::Value;
use tokio::task::JoinHandle;

pub mod status;

use status::{Activity, GCODES, Track};

/// The Driver id stored with each Duet Printer.
pub const DRIVER_ID: &str = "duet";

/// The board's web port.
pub const DEFAULT_PORT: u16 = 80;

/// The password RepRapFirmware takes when none is set.
pub const DEFAULT_PASSWORD: &str = "reprap";

/// How often the object model is read.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

/// How long a print that has gone back to idle is held for the object model
/// to say how it ended, before the Fleet is told it ended unseen.
const ENDING_GRACE: Duration = Duration::from_secs(5);

/// How long Cancel waits for a print to finish pausing before it sends `M0`.
const PAUSE_WAIT: Duration = Duration::from_secs(7);

/// The Duet Driver.
#[derive(Debug, Clone, Copy, Default)]
pub struct DuetDriver;

impl Driver for DuetDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn name(&self) -> &'static str {
        "Duet (RepRapFirmware)"
    }

    fn fields(&self) -> Vec<FieldSpec> {
        vec![
            FieldSpec::new("host", "Address", FieldKind::Text)
                .required()
                .help("The Duet's IP address or hostname, such as duet3.local."),
            FieldSpec::new("port", "Port", FieldKind::Number)
                .help("The board's web port, normally 80.")
                .default_value(DEFAULT_PORT.into()),
            FieldSpec::new("password", "Password", FieldKind::Secret)
                .help("Only if the board has one (M551 in config.g). Leave it empty otherwise."),
            FieldSpec::new("model", "Printer model", FieldKind::Text).help(
                "The printer's model as your slicer names it. A Print File is only ever sent to \
                 the model it was sliced for.",
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
                message: "Just the address, such as duet3.local, without http:// or a path".into(),
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
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            password: printer
                .secrets
                .get("password")
                .filter(|p| !p.is_empty())
                .unwrap_or(DEFAULT_PASSWORD)
                .to_owned(),
            session_key: Mutex::new(None),
            model: printer.text("model").map(str::to_owned),
            events,
            state: Mutex::new(State::default()),
        });
        let task = tokio::spawn(run(shared.clone()));
        Box::new(DuetPrinter {
            shared,
            task: Mutex::new(Some(task)),
        })
    }
}

#[derive(Default)]
struct State {
    track: Option<Track>,
    /// When the followed print was first seen no longer running.
    idle_since: Option<tokio::time::Instant>,
    status: Option<PrinterStatus>,
}

struct Shared {
    name: String,
    base: String,
    client: reqwest::Client,
    password: String,
    /// RepRapFirmware 3.5's session key, sent back as `X-Session-Key`.
    session_key: Mutex<Option<String>>,
    model: Option<String>,
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

/// Why a request failed.
enum Fail {
    Down(Down),
    /// The board no longer knows the session: open another.
    Expired,
}

impl From<Down> for Fail {
    fn from(down: Down) -> Self {
        Self::Down(down)
    }
}

/// What `rr_connect` said.
fn connect_error(body: &Value) -> Option<Down> {
    match body["err"].as_i64() {
        Some(0) => None,
        Some(1) => Some(down(
            DisconnectReason::AuthenticationRejected,
            "The Duet refused the password",
        )),
        Some(2) => Some(down(
            DisconnectReason::ProtocolError,
            "The Duet has no free sessions: close a browser tab running Duet Web Control",
        )),
        Some(n) => Some(down(
            DisconnectReason::ProtocolError,
            format!("The Duet refused the connection (error {n})"),
        )),
        None => Some(down(
            DisconnectReason::ProtocolError,
            "The Duet's answer to rr_connect wasn't understood",
        )),
    }
}

/// A file name the board can store and `M32` can quote.
fn safe_name(name: &str) -> String {
    name.replace(['/', '\\'], "_").replace('"', "'")
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .timeout(REQUEST_TIMEOUT);
        if let Some(key) = self
            .session_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            && let Ok(mut value) = reqwest::header::HeaderValue::from_str(key)
        {
            value.set_sensitive(true);
            request = request.header("X-Session-Key", value);
        }
        request
    }

    /// Opens a session. The password travels in the query, as
    /// RepRapFirmware asks: errors are reported without the URL.
    async fn open_session(&self) -> Result<(), Down> {
        *self
            .session_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let response = self
            .request(reqwest::Method::GET, "/rr_connect")
            .query(&[("password", self.password.as_str())])
            .send()
            .await
            .map_err(|e| {
                down(
                    DisconnectReason::Unreachable,
                    format!("The Duet didn't answer: {}", e.without_url()),
                )
            })?;
        match response.status().as_u16() {
            401 | 403 => {
                return Err(down(
                    DisconnectReason::AuthenticationRejected,
                    "The Duet refused the password",
                ));
            }
            s if s >= 400 => {
                return Err(down(
                    DisconnectReason::ProtocolError,
                    format!(
                        "The Duet answered {s} to rr_connect: is it RepRapFirmware in standalone mode?"
                    ),
                ));
            }
            _ => {}
        }
        let body: Value = response.json().await.map_err(|_| {
            down(
                DisconnectReason::ProtocolError,
                "The Duet's answer to rr_connect wasn't JSON",
            )
        })?;
        if let Some(down) = connect_error(&body) {
            return Err(down);
        }
        let key = match &body["sessionKey"] {
            Value::Number(n) => Some(n.to_string()),
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
        *self
            .session_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = key;
        Ok(())
    }

    /// A read from the object model: its `result`.
    async fn model(&self, query: &[(&str, &str)]) -> Result<Value, Fail> {
        let response = self
            .request(reqwest::Method::GET, "/rr_model")
            .query(query)
            .send()
            .await
            .map_err(|e| {
                down(
                    DisconnectReason::Unreachable,
                    format!("The Duet didn't answer: {}", e.without_url()),
                )
            })?;
        match response.status().as_u16() {
            401 | 403 => return Err(Fail::Expired),
            s if s >= 400 => {
                return Err(Fail::Down(down(
                    DisconnectReason::ProtocolError,
                    format!("The Duet answered {s} to rr_model"),
                )));
            }
            _ => {}
        }
        let mut body: Value = response.json().await.map_err(|_| {
            down(
                DisconnectReason::ProtocolError,
                "The Duet's object model wasn't JSON",
            )
        })?;
        match body.get_mut("result") {
            Some(result) => Ok(result.take()),
            None => Err(Fail::Down(down(
                DisconnectReason::ProtocolError,
                "The Duet's object model had no result",
            ))),
        }
    }

    /// The whole object model: the live values, with the job in full (its
    /// file and how the last one ended are not live values).
    async fn read_model(&self) -> Result<Value, Fail> {
        let mut model = self.model(&[("flags", "d99fn")]).await?;
        let job = self.model(&[("key", "job"), ("flags", "d99vn")]).await?;
        if let Some(object) = model.as_object_mut() {
            object.insert("job".into(), job);
        }
        Ok(model)
    }

    /// Sends G-code (several lines may go at once). An expired session is
    /// opened again and the G-code sent once more.
    async fn gcode(&self, code: &str) -> Result<(), CommandError> {
        for attempt in 0..2 {
            let sent = self
                .request(reqwest::Method::GET, "/rr_gcode")
                .query(&[("gcode", code)])
                .send()
                .await;
            let response = match sent {
                Ok(r) => r,
                Err(e) if e.is_timeout() => return Err(CommandError::TimedOut),
                Err(_) => {
                    return Err(CommandError::Disconnected {
                        reason: DisconnectReason::Unreachable,
                    });
                }
            };
            match response.status().as_u16() {
                401 | 403 if attempt == 0 => {
                    self.open_session()
                        .await
                        .map_err(|d| CommandError::Refused { message: d.detail })?;
                }
                401 | 403 => {
                    return Err(CommandError::Refused {
                        message: "The Duet refused the session".into(),
                    });
                }
                s if s >= 400 => {
                    return Err(CommandError::Refused {
                        message: format!("The Duet answered {s}"),
                    });
                }
                _ => return Ok(()),
            }
        }
        Ok(())
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

    /// Folds in one read of the object model: starts following a print that
    /// has appeared, and reports one that has ended.
    fn take_in(&self, model: &Value) {
        let mut state = self.lock();
        let mut ended = None;
        if status::running(model) {
            state.idle_since = None;
            if let Some(file) = status::file(model)
                && state.track.as_ref().is_none_or(|t| t.file != file)
            {
                let track = Track::new(file, model);
                tracing::info!(printer = %self.name, key = %track.key, "a print is running");
                state.track = Some(track);
            }
        } else if let Some(track) = state.track.clone() {
            let outcome = status::outcome(model, &track.file).or_else(|| {
                (status::activity(model) == Activity::Halted)
                    .then_some(gantry_core::JobOutcome::Failed)
            });
            match outcome {
                Some(outcome) => ended = Some((track.key, outcome)),
                None => {
                    // The object model may be a moment behind: wait for it.
                    let since = *state
                        .idle_since
                        .get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() < ENDING_GRACE {
                        return;
                    }
                    tracing::info!(printer = %self.name, "a print ended without the Duet saying how");
                    state.track = None;
                    state.idle_since = None;
                }
            }
        }
        if let Some((key, outcome)) = ended {
            state.track = None;
            state.idle_since = None;
            tracing::info!(printer = %self.name, %key, outcome = outcome.as_str(), "a print ended");
            self.events.job_ended(key, outcome);
        }
        let status = status::status(model, state.track.as_ref());
        state.status = Some(status.clone());
        drop(state);
        self.events.status(status);
    }

    /// Waits for the print to be Paused, reading the board itself.
    async fn until_paused(&self) -> bool {
        let deadline = tokio::time::Instant::now() + PAUSE_WAIT;
        while tokio::time::Instant::now() < deadline {
            if let Ok(state) = self.model(&[("key", "state"), ("flags", "d99fn")]).await
                && state["status"].as_str() == Some("paused")
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        false
    }
}

async fn run(shared: Arc<Shared>) {
    let mut backoff = BACKOFF_MIN;
    loop {
        if shared.events.is_closed() {
            return;
        }
        let Down { reason, detail } = session(&shared, &mut backoff).await;
        tracing::info!(printer = %shared.name, reason = reason.describe(), %detail, "Duet connection down");
        {
            let mut state = shared.lock();
            state.track = None;
            state.idle_since = None;
        }
        shared.report(PrinterStatus {
            connection: Connection::Disconnected {
                reason,
                detail: Some(detail),
            },
            ..PrinterStatus::disconnected(reason)
        });
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One connection: open a session, then read the object model until
/// something goes wrong. A good first read resets the backoff.
async fn session(shared: &Shared, backoff: &mut Duration) -> Down {
    shared.events.status(PrinterStatus::connecting());
    if let Err(down) = shared.open_session().await {
        return down;
    }
    let mut first = true;
    loop {
        match shared.read_model().await {
            Ok(model) => {
                if first {
                    tracing::info!(printer = %shared.name, "connected to a Duet");
                    *backoff = BACKOFF_MIN;
                    first = false;
                }
                shared.take_in(&model);
            }
            Err(Fail::Expired) => {
                tracing::debug!(printer = %shared.name, "the Duet's session expired: opening another");
                if let Err(down) = shared.open_session().await {
                    return down;
                }
                continue;
            }
            Err(Fail::Down(down)) => return down,
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One Duet Printer.
pub struct DuetPrinter {
    shared: Arc<Shared>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl DuetPrinter {
    fn require(&self, ok: impl Fn(PrinterState) -> bool, what: &str) -> Result<(), CommandError> {
        self.shared.require_connected()?;
        match self.shared.printer_state() {
            Some(state) if ok(state) => Ok(()),
            _ => Err(CommandError::invalid_state(what)),
        }
    }
}

#[async_trait]
impl PrinterHandle for DuetPrinter {
    async fn pause(&self) -> Result<(), CommandError> {
        self.require(
            |s| matches!(s, PrinterState::Printing | PrinterState::Preparing),
            "nothing is printing",
        )?;
        self.shared.gcode("M25").await
    }

    async fn resume(&self) -> Result<(), CommandError> {
        self.require(|s| s == PrinterState::Paused, "the print is not paused")?;
        self.shared.gcode("M24").await
    }

    /// RepRapFirmware cancels a paused print with `M0`, so a running one is
    /// paused first.
    async fn cancel(&self) -> Result<(), CommandError> {
        self.require(PrinterState::has_job, "nothing is printing")?;
        if self.shared.printer_state() != Some(PrinterState::Paused) {
            self.shared.gcode("M25").await?;
            if !self.shared.until_paused().await {
                return Err(CommandError::invalid_state(
                    "the print is still pausing: Cancel again once it is Paused",
                ));
            }
        }
        self.shared.gcode("M0").await
    }

    async fn fine(&self, control: FineControl) -> Result<(), CommandError> {
        self.shared.require_connected()?;
        match control {
            FineControl::SetTemperature {
                heater: Heater::Nozzle,
                target,
            } => {
                self.shared
                    .gcode(&format!("M104 T0 S{}", target.round()))
                    .await
            }
            FineControl::SetTemperature {
                heater: Heater::Bed,
                target,
            } => {
                self.shared
                    .gcode(&format!("M140 S{}", target.round()))
                    .await
            }
            FineControl::SetFan { fan, percent } => {
                let Ok(index) = fan.parse::<u32>() else {
                    return Err(CommandError::invalid("no such fan"));
                };
                self.shared
                    .gcode(&format!("M106 P{index} S{:.2}", f64::from(percent) / 100.0))
                    .await
            }
            FineControl::Home => {
                self.require(|s| !s.has_job(), "can't home while a Job is running")?;
                self.shared.gcode("G28").await
            }
            FineControl::Jog { axis, distance_mm } => {
                self.require(|s| !s.has_job(), "can't jog while a Job is running")?;
                let (axis, feed) = match axis {
                    Axis::X => ("X", 6000),
                    Axis::Y => ("Y", 6000),
                    Axis::Z => ("Z", 600),
                };
                self.shared
                    .gcode(&format!("G91\nG1 {axis}{distance_mm} F{feed}\nG90"))
                    .await
            }
            _ => Err(CommandError::Unsupported),
        }
    }

    /// Uploads the Plate's G-code to `0:/gcodes/` and starts it with `M32`.
    /// A `.gcode.3mf` sends just the Plate's G-code.
    async fn upload_and_start(&self, upload: PrintUpload) -> Result<Option<String>, CommandError> {
        self.require(
            |s| s == PrinterState::Idle,
            "the Printer isn't Idle, so it can't start a Job",
        )?;
        let (name, gcode) = gantry_driver_support::plate::gcode_for(&upload)?;
        let path = format!("{GCODES}{}", safe_name(&name));
        let response = self
            .shared
            .request(reqwest::Method::POST, "/rr_upload")
            .query(&[("name", path.as_str())])
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(Bytes::from(gcode))
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
            return Err(CommandError::Refused {
                message: format!("The Duet answered {status} to the upload"),
            });
        }
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if body["err"].as_i64() != Some(0) {
            return Err(CommandError::Refused {
                message: "The Duet couldn't store the Print File".into(),
            });
        }
        self.shared.gcode(&format!("M32 \"{path}\"")).await?;
        tracing::info!(printer = %self.shared.name, file = %name, "sent a Print File");
        // The Fleet takes the next print this Printer reports to be this Job.
        Ok(None)
    }

    fn model(&self) -> Option<String> {
        self.shared.model.clone()
    }

    /// Stops polling and closes the session, so the board has it free.
    async fn shutdown(&self) {
        let task = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                self.shared
                    .request(reqwest::Method::GET, "/rr_disconnect")
                    .send(),
            )
            .await;
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn settings_are_checked() {
        let settings = json!({ "host": "duet3.local/rr_connect", "port": 0 });
        let errors = DuetDriver.validate(settings.as_object().unwrap(), &Secrets::default());
        let fields: Vec<_> = errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, ["port", "host"], "the password is optional");
        let fine = json!({ "host": "192.168.1.60", "port": 80 });
        assert!(
            DuetDriver
                .validate(fine.as_object().unwrap(), &Secrets::default())
                .is_empty()
        );
    }

    #[test]
    fn rr_connect_errors_are_read() {
        assert!(connect_error(&json!({ "err": 0, "sessionTimeout": 8000 })).is_none());
        assert_eq!(
            connect_error(&json!({ "err": 1 })).unwrap().reason,
            DisconnectReason::AuthenticationRejected
        );
        assert_eq!(
            connect_error(&json!({ "err": 2 })).unwrap().reason,
            DisconnectReason::ProtocolError
        );
        assert!(connect_error(&json!({})).is_some());
    }

    #[test]
    fn file_names_are_made_safe_for_m32() {
        assert_eq!(safe_name("a \"b\"/c.gcode"), "a 'b'_c.gcode");
    }
}
