//! The PrusaLink Driver (PRD §10.2): Prusa MK4, MK3.9, Mini and XL (and an
//! MK3 with PrusaLink on a Raspberry Pi), through PrusaLink's REST API v1.
//!
//! PrusaLink has no push channel, so the Driver polls `/api/v1/status` about
//! once a second and folds it into the normalised status model ([`status`]).
//! Pause, resume and Cancel are `PUT /api/v1/job/{id}/pause|resume` and
//! `DELETE /api/v1/job/{id}`. A Plate's G-code is sent with
//! `PUT /api/v1/files/{storage}/{name}` and `Print-After-Upload: ?1`, which
//! also starts it. A print's key is PrusaLink's own job id with the Print
//! File's name; its ending is the printer's `FINISHED`, `STOPPED` or `ERROR`
//! state (Succeeded, Cancelled, Failed). A dropped connection is retried
//! with backoff for as long as the Printer is in the Fleet.
//!
//! PrusaLink asks for HTTP Digest authentication (the user `maker` and the
//! password on the printer's network screen) on current firmware, and for an
//! `X-Api-Key` header on older versions: the Driver does whichever the
//! Printer's settings give it ([`digest`] is the Digest side).
//!
//! PrusaLink has no fine controls in its API, and the Driver claims none. The
//! camera is not offered: PrusaLink's cameras belong to Prusa Connect.
//!
//! **Unverified against a real printer.** No PrusaLink simulator exists (PRD
//! §5.6), so the Driver is tested against a mock HTTP server written from
//! Prusa's OpenAPI description of the API (`tests/mock`), and stays
//! unproven until a Prusa printer is available.
use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use gantry_core::{
    CommandError, Connection, DisconnectReason, Driver, EventSender, FieldError, FieldKind,
    FieldSpec, PrintUpload, PrinterConfig, PrinterHandle, PrinterState, PrinterStatus, Secrets,
};
use reqwest::Method;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, WWW_AUTHENTICATE};
use serde_json::Value;
use tokio::task::JoinHandle;

pub mod digest;
pub mod status;

use digest::Challenge;
use status::{Phase, Track};

/// The Driver id stored with each PrusaLink Printer.
pub const DRIVER_ID: &str = "prusalink";

/// PrusaLink's port.
pub const DEFAULT_PORT: u16 = 80;

/// The Digest user PrusaLink's firmware sets.
pub const DEFAULT_USERNAME: &str = "maker";

/// Where a Print File goes when PrusaLink doesn't list its storage.
pub const DEFAULT_STORAGE: &str = "usb";

/// How often PrusaLink is asked for the printer's status.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);

/// The PrusaLink Driver.
#[derive(Debug, Clone, Copy, Default)]
pub struct PrusaLinkDriver;

impl Driver for PrusaLinkDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn name(&self) -> &'static str {
        "PrusaLink"
    }

    fn fields(&self) -> Vec<FieldSpec> {
        vec![
            FieldSpec::new("host", "Address", FieldKind::Text)
                .required()
                .help("The printer's IP address or hostname, from its network screen, such as 192.168.1.50."),
            FieldSpec::new("port", "Port", FieldKind::Number)
                .help("PrusaLink's port, normally 80.")
                .default_value(DEFAULT_PORT.into()),
            FieldSpec::new("username", "Username", FieldKind::Text)
                .help("PrusaLink's user, maker unless you changed it.")
                .default_value(DEFAULT_USERNAME.into()),
            FieldSpec::new("password", "Password", FieldKind::Secret).help(
                "The PrusaLink password from the printer's network screen (Settings → Network → PrusaLink).",
            ),
            FieldSpec::new("api_key", "API key", FieldKind::Secret).help(
                "Only for older PrusaLink versions that show an API key instead of a password.",
            ),
            FieldSpec::new("model", "Printer model", FieldKind::Text).help(
                "The printer's model as your slicer names it, such as MK4. A Print File is only \
                 ever sent to the model it was sliced for.",
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
        let given = |key: &str| secrets.get(key).is_some_and(|v| !v.is_empty());
        if !given("password") && !given("api_key") {
            errors.push(FieldError {
                field: "password".into(),
                message: "Give the PrusaLink password, or the API key on older versions".into(),
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
        let password = printer.secrets.get("password").filter(|p| !p.is_empty());
        let credentials = match password {
            Some(password) => Credentials::Digest {
                username: printer
                    .text("username")
                    .unwrap_or(DEFAULT_USERNAME)
                    .to_owned(),
                password: password.to_owned(),
            },
            None => Credentials::ApiKey(
                printer
                    .secrets
                    .get("api_key")
                    .unwrap_or_default()
                    .to_owned(),
            ),
        };
        let shared = Arc::new(Shared {
            name: printer.name.clone(),
            base: format!("http://{host}:{port}"),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            credentials,
            challenge: Mutex::new(None),
            model: printer.text("model").map(str::to_owned),
            events,
            state: Mutex::new(State::default()),
        });
        let task = tokio::spawn(run(shared.clone()));
        Box::new(PrusaLinkPrinter {
            shared,
            task: Mutex::new(Some(task)),
        })
    }
}

/// How the Driver proves who it is. `Debug` never shows the values.
enum Credentials {
    Digest { username: String, password: String },
    ApiKey(String),
}

impl Credentials {
    fn what(&self) -> &'static str {
        match self {
            Self::Digest { .. } => "the username and password",
            Self::ApiKey(_) => "the API key",
        }
    }
}

#[derive(Default)]
struct State {
    track: Option<Track>,
    status: Option<PrinterStatus>,
    /// The storage Print Files are uploaded to, such as `usb`.
    storage: Option<String>,
}

struct Shared {
    name: String,
    base: String,
    client: reqwest::Client,
    credentials: Credentials,
    /// The last Digest challenge, and how many answers have been made to it.
    challenge: Mutex<Option<(Challenge, u32)>>,
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

/// A path segment, percent-encoded.
fn encode(segment: &str) -> String {
    let mut out = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// PrusaLink's error message, from `{"title": …, "message": …}`.
fn error_message(body: &Value) -> Option<String> {
    body.get("message")
        .or_else(|| body.get("title"))
        .and_then(Value::as_str)
        .map(|m| m.lines().next().unwrap_or(m).to_owned())
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn authorization(&self, method: &Method, path: &str) -> Option<HeaderValue> {
        match &self.credentials {
            Credentials::ApiKey(key) => HeaderValue::from_str(key).ok(),
            Credentials::Digest { username, password } => {
                let mut challenge = self
                    .challenge
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (challenge, nc) = challenge.as_mut()?;
                *nc += 1;
                let cnonce = format!("{:016x}", rand::random::<u64>());
                let answer =
                    challenge.answer(username, password, method.as_str(), path, *nc, &cnonce);
                let mut value = HeaderValue::from_str(&answer).ok()?;
                value.set_sensitive(true);
                Some(value)
            }
        }
    }

    /// Sends a request, answering a Digest challenge once if the Printer
    /// asks (or says the last nonce went stale).
    async fn send(
        &self,
        method: Method,
        path: &str,
        headers: HeaderMap,
        body: Option<Bytes>,
        timeout: Duration,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let digest = matches!(self.credentials, Credentials::Digest { .. });
        let mut answered = false;
        loop {
            let mut request = self
                .client
                .request(method.clone(), format!("{}{path}", self.base))
                .headers(headers.clone())
                .timeout(timeout);
            if let Some(body) = &body {
                request = request.body(body.clone());
            }
            if let Some(mut value) = self.authorization(&method, path) {
                value.set_sensitive(true);
                let name = if digest {
                    AUTHORIZATION.as_str()
                } else {
                    "X-Api-Key"
                };
                request = request.header(name, value);
            }
            let response = request.send().await?;
            if response.status() != reqwest::StatusCode::UNAUTHORIZED || !digest || answered {
                return Ok(response);
            }
            let Some(fresh) = response
                .headers()
                .get_all(WWW_AUTHENTICATE)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .find_map(|v| Challenge::parse(v).ok())
            else {
                return Ok(response);
            };
            {
                let mut held = self
                    .challenge
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let retry = held
                    .as_ref()
                    .is_none_or(|(old, _)| fresh.stale || old.nonce != fresh.nonce);
                *held = Some((fresh, 0));
                if !retry {
                    return Ok(response);
                }
            }
            answered = true;
        }
    }

    /// A read, as JSON (`Null` for an empty answer).
    async fn get(&self, path: &str) -> Result<Value, Down> {
        let response = self
            .send(Method::GET, path, HeaderMap::new(), None, REQUEST_TIMEOUT)
            .await
            .map_err(|e| {
                down(
                    DisconnectReason::Unreachable,
                    format!("PrusaLink didn't answer: {}", e.without_url()),
                )
            })?;
        match response.status().as_u16() {
            401 | 403 => Err(down(
                DisconnectReason::AuthenticationRejected,
                format!("PrusaLink refused: check {}", self.credentials.what()),
            )),
            204 => Ok(Value::Null),
            s if s >= 400 => Err(down(
                DisconnectReason::ProtocolError,
                format!("PrusaLink answered {s} to {path}"),
            )),
            _ => {
                let text = response.text().await.unwrap_or_default();
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                serde_json::from_str(&text).map_err(|_| {
                    down(
                        DisconnectReason::ProtocolError,
                        format!("PrusaLink's answer to {path} wasn't JSON"),
                    )
                })
            }
        }
    }

    /// A job command. A slow answer is taken as accepted: PrusaLink answers
    /// once the printer has the command, and the next status shows it.
    async fn command(&self, method: Method, path: &str) -> Result<(), CommandError> {
        let sent = self
            .send(method, path, HeaderMap::new(), None, COMMAND_TIMEOUT)
            .await;
        let response = match sent {
            Ok(r) => r,
            Err(e) if e.is_timeout() => return Ok(()),
            Err(_) => {
                return Err(CommandError::Disconnected {
                    reason: DisconnectReason::Unreachable,
                });
            }
        };
        refusal(response, &self.credentials).await
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

    /// The storage to upload to: the first writable one PrusaLink lists.
    async fn find_storage(&self) -> String {
        let listed = self.get("/api/v1/storage").await.ok();
        listed
            .as_ref()
            .and_then(|l| l["storage_list"].as_array())
            .and_then(|list| {
                list.iter().find_map(|s| {
                    let usable = s["available"].as_bool() != Some(false)
                        && s["read_only"].as_bool() != Some(true);
                    let name = s["path"]
                        .as_str()
                        .or_else(|| s["name"].as_str())?
                        .trim_matches('/');
                    (usable && !name.is_empty()).then(|| name.to_owned())
                })
            })
            .unwrap_or_else(|| DEFAULT_STORAGE.to_owned())
    }

    /// One poll: the printer's status, the print it is running, and how one
    /// that has gone ended.
    async fn poll(&self) -> Result<(), Down> {
        let now = self.get("/api/v1/status").await?;
        if !now.is_object() {
            return Err(down(
                DisconnectReason::ProtocolError,
                "PrusaLink's status was empty",
            ));
        }
        let tracked = self.lock().track.clone();
        let mut ended = None;
        match status::phase(&now) {
            Phase::Running => {
                if let Some(id) = status::job_id(&now)
                    && tracked.as_ref().is_none_or(|t| t.id != id)
                {
                    let job = self.get("/api/v1/job").await?;
                    let track = Track::new(id, status::job_file(&job), &now);
                    tracing::info!(printer = %self.name, key = %track.key, "a print is running");
                    self.lock().track = Some(track);
                }
            }
            Phase::Ended(outcome) => {
                if let Some(track) = tracked {
                    ended = Some((track.key, outcome));
                }
            }
            Phase::None => {
                if let Some(track) = tracked {
                    // The printer has moved on: the job's own record may
                    // still say how it ended.
                    let job = self.get("/api/v1/job").await?;
                    let outcome = (job["id"].as_u64() == Some(track.id))
                        .then(|| status::job_outcome(&job))
                        .flatten();
                    match outcome {
                        Some(outcome) => ended = Some((track.key, outcome)),
                        None => {
                            tracing::info!(printer = %self.name, "a print ended without PrusaLink saying how");
                            self.lock().track = None;
                        }
                    }
                }
            }
        }
        if let Some((key, outcome)) = ended {
            self.lock().track = None;
            tracing::info!(printer = %self.name, %key, outcome = outcome.as_str(), "a print ended");
            self.events.job_ended(key, outcome);
        }
        let track = self.lock().track.clone();
        self.report(status::status(&now, track.as_ref()));
        Ok(())
    }
}

/// A command's answer: `Ok` for success, else why it was refused.
async fn refusal(
    response: reqwest::Response,
    credentials: &Credentials,
) -> Result<(), CommandError> {
    let status = response.status().as_u16();
    if status < 400 {
        return Ok(());
    }
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let message = error_message(&body).unwrap_or_else(|| format!("PrusaLink answered {status}"));
    Err(match status {
        401 | 403 => CommandError::Refused {
            message: format!("PrusaLink refused {}", credentials.what()),
        },
        404 | 409 => CommandError::invalid_state(message),
        _ => CommandError::Refused { message },
    })
}

async fn run(shared: Arc<Shared>) {
    let mut backoff = BACKOFF_MIN;
    loop {
        if shared.events.is_closed() {
            return;
        }
        let Down { reason, detail } = session(&shared, &mut backoff).await;
        tracing::info!(printer = %shared.name, reason = reason.describe(), %detail, "PrusaLink connection down");
        shared.lock().track = None;
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

/// One connection: check it is PrusaLink, then poll until something goes
/// wrong. A good first poll resets the backoff.
async fn session(shared: &Shared, backoff: &mut Duration) -> Down {
    shared.events.status(PrinterStatus::connecting());
    if let Err(down) = shared.get("/api/version").await {
        return down;
    }
    let storage = shared.find_storage().await;
    shared.lock().storage = Some(storage);
    let mut first = true;
    loop {
        if let Err(down) = shared.poll().await {
            return down;
        }
        if first {
            tracing::info!(printer = %shared.name, "connected to PrusaLink");
            *backoff = BACKOFF_MIN;
            first = false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One PrusaLink Printer.
pub struct PrusaLinkPrinter {
    shared: Arc<Shared>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl PrusaLinkPrinter {
    /// The running job's id, once the Printer is in a state `ok` allows.
    fn job(&self, ok: impl Fn(PrinterState) -> bool, what: &str) -> Result<u64, CommandError> {
        self.shared.require_connected()?;
        match self.shared.printer_state() {
            Some(state) if ok(state) => {}
            _ => return Err(CommandError::invalid_state(what)),
        }
        self.shared
            .lock()
            .track
            .as_ref()
            .map(|t| t.id)
            .ok_or_else(|| CommandError::invalid_state(what))
    }
}

#[async_trait]
impl PrinterHandle for PrusaLinkPrinter {
    async fn pause(&self) -> Result<(), CommandError> {
        let id = self.job(
            |s| matches!(s, PrinterState::Printing | PrinterState::Preparing),
            "nothing is printing",
        )?;
        self.shared
            .command(Method::PUT, &format!("/api/v1/job/{id}/pause"))
            .await
    }

    async fn resume(&self) -> Result<(), CommandError> {
        let id = self.job(|s| s == PrinterState::Paused, "the print is not paused")?;
        self.shared
            .command(Method::PUT, &format!("/api/v1/job/{id}/resume"))
            .await
    }

    async fn cancel(&self) -> Result<(), CommandError> {
        let id = self.job(PrinterState::has_job, "nothing is printing")?;
        self.shared
            .command(Method::DELETE, &format!("/api/v1/job/{id}"))
            .await
    }

    /// Uploads the Plate's G-code to the printer's storage and starts it
    /// (`Print-After-Upload`). A `.gcode.3mf` sends just the Plate's G-code.
    async fn upload_and_start(&self, upload: PrintUpload) -> Result<Option<String>, CommandError> {
        self.shared.require_connected()?;
        if self.shared.printer_state() != Some(PrinterState::Idle) {
            return Err(CommandError::invalid_state(
                "the Printer isn't Idle, so it can't start a Job",
            ));
        }
        let (name, gcode) = gantry_driver_support::plate::gcode_for(&upload)?;
        let name = name.replace(['/', '\\'], "_");
        let storage = self
            .shared
            .lock()
            .storage
            .clone()
            .unwrap_or_else(|| DEFAULT_STORAGE.to_owned());
        let path = format!("/api/v1/files/{}/{}", encode(&storage), encode(&name));
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        headers.insert("Print-After-Upload", HeaderValue::from_static("?1"));
        headers.insert("Overwrite-File", HeaderValue::from_static("?1"));
        let response = self
            .shared
            .send(
                Method::PUT,
                &path,
                headers,
                Some(Bytes::from(gcode)),
                gantry_core::UPLOAD_TIMEOUT,
            )
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
        refusal(response, &self.shared.credentials).await?;
        tracing::info!(printer = %self.shared.name, file = %name, "sent a Print File");
        // The Fleet takes the next print this Printer reports to be this Job.
        Ok(None)
    }

    fn model(&self) -> Option<String> {
        self.shared.model.clone()
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
    use serde_json::json;

    use super::*;

    fn secrets(pairs: &[(&str, &str)]) -> Secrets {
        Secrets::new(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn settings_are_checked() {
        let settings = json!({ "host": "http://prusa.local/", "port": 70000 });
        let errors = PrusaLinkDriver.validate(settings.as_object().unwrap(), &Secrets::default());
        let fields: Vec<_> = errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, ["port", "host", "password"]);
    }

    #[test]
    fn a_password_or_an_api_key_will_do() {
        let settings = json!({ "host": "192.168.1.50" });
        let settings = settings.as_object().unwrap();
        assert!(
            PrusaLinkDriver
                .validate(settings, &secrets(&[("password", "pw")]))
                .is_empty()
        );
        assert!(
            PrusaLinkDriver
                .validate(settings, &secrets(&[("api_key", "key")]))
                .is_empty()
        );
        assert_eq!(
            PrusaLinkDriver
                .validate(settings, &secrets(&[("password", "")]))
                .len(),
            1
        );
    }

    #[test]
    fn names_are_encoded_for_the_path() {
        assert_eq!(encode("my cube #2.gcode"), "my%20cube%20%232.gcode");
        assert_eq!(encode("usb"), "usb");
    }

    #[test]
    fn errors_are_read_from_prusalink() {
        assert_eq!(
            error_message(&json!({ "title": "Conflict", "message": "Printer is busy" })).as_deref(),
            Some("Printer is busy")
        );
    }
}
