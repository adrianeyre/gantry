//! The Bambu Lab Driver (PRD §10.3): X1, P1 and A1 printers in LAN Mode.
//!
//! Each Printer gets one MQTT-over-TLS connection to `mqtts://<ip>:8883`,
//! username `bblp` and the access code as password. Status arrives on
//! `device/<serial>/report`; commands go to `device/<serial>/request`, and the
//! printer answers each on the report topic with the same sequence id. A
//! dropped connection is retried with backoff for as long as the Printer is in
//! the Fleet.
//!
//! When firmware refuses local control (Bambu's Authorization Control, or
//! Developer Mode off), the Printer stays connected with live status and the
//! Driver withdraws every control capability, saying how to fix it (PRD §6.1).
//! Control comes back when the printer reconnects with control allowed.
//!
//! A Print File is sent by uploading it over FTPS (port 990, see [`ftps`]),
//! then starting the chosen Plate with a command over MQTT. A failed upload
//! starts nothing.
//!
//! The serial number and access code are credentials here: they are stored
//! encrypted, and never logged (topics name the serial, so topics aren't
//! logged either).
//!
//! Tested against `gantry-mock-bambu` only. Real firmware is unverified (tracked in #27) until
//! a Bambu printer is owned (PRD §13).
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use gantry_core::{
    CameraSource, Capability, CommandError, DisconnectReason, Driver, EventSender, FieldKind,
    FieldSpec, FineControl, JobOutcome, PrintUpload, PrinterConfig, PrinterHandle, PrinterStatus,
};
use rumqttc::{
    AsyncClient, ConnectReturnCode, ConnectionError, Event, MqttOptions, Packet, QoS,
    TlsConfiguration, Transport,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub mod camera;
pub mod control;
mod filament_and_faults;
pub mod ftps;
pub mod model;
pub mod probe;
pub mod report;
mod tls;

use control::Answer;
use model::{CameraKind, Model};
use report::{GcodeState, Report};

/// The Driver id stored with each Bambu Printer.
pub const DRIVER_ID: &str = "bambu";

/// The port a Bambu printer's broker listens on.
pub const MQTT_PORT: u16 = 8883;

/// The username every Bambu printer expects.
const USERNAME: &str = "bblp";

/// The first wait before reconnecting, doubled after each failure.
const BACKOFF_MIN: Duration = Duration::from_millis(500);
/// The longest wait between reconnection attempts.
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// How long to wait for the printer to answer a command. Firmware that
/// doesn't answer at all is taken to have accepted it, as before answers
/// were read.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(3);

/// Log filter directives Gantry must always apply with this Driver.
///
/// The MQTT client logs topic names at debug level, and every topic names the
/// printer's serial number, which is a credential here. Capping it keeps the
/// serial out of the log even under `RUST_LOG=trace`.
pub const LOG_DIRECTIVES: &[&str] = &["rumqttc=warn"];

/// The Bambu Lab Driver.
#[derive(Debug, Clone, Copy, Default)]
pub struct BambuDriver;

impl Driver for BambuDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn name(&self) -> &'static str {
        "Bambu Lab (LAN Mode)"
    }

    fn fields(&self) -> Vec<FieldSpec> {
        vec![
            FieldSpec::new("host", "IP address", FieldKind::Text)
                .required()
                .help("The printer's address on your network. LAN Mode must be on."),
            FieldSpec::new("serial", "Serial number", FieldKind::Secret)
                .required()
                .help("On the printer's network or device screen."),
            FieldSpec::new("access_code", "Access code", FieldKind::Secret)
                .required()
                .help("On the printer's network screen, under LAN Mode."),
            FieldSpec::new("model", "Model", FieldKind::Text).help(
                "X1, X1C, X1E, P1P, P1S, A1 or A1 mini. Leave blank to read it from the serial \
                 number; it decides which camera the printer has.",
            ),
            FieldSpec::new("camera_port", "Camera port", FieldKind::Number)
                .help("Leave blank for the model's own: 6000 on P1 and A1, 322 on X1."),
            FieldSpec::new("ftp_port", "FTPS port", FieldKind::Number)
                .help("Leave blank for the printer's own, 990. Print Files are uploaded here."),
        ]
    }

    fn validate(
        &self,
        settings: &serde_json::Map<String, serde_json::Value>,
        secrets: &gantry_core::Secrets,
    ) -> Vec<gantry_core::FieldError> {
        let mut errors = gantry_core::driver::validate_fields(&self.fields(), settings, secrets);
        if let Some(host) = settings.get("host").and_then(|h| h.as_str())
            && !host.trim().is_empty()
            && parse_host(host).is_none()
        {
            errors.push(gantry_core::FieldError {
                field: "host".into(),
                message: "Must be an address, optionally with :port".into(),
            });
        }
        if let Some(model) = settings.get("model").and_then(|m| m.as_str())
            && !model.trim().is_empty()
            && Model::parse(model).is_none()
        {
            errors.push(gantry_core::FieldError {
                field: "model".into(),
                message: "Must be X1, X1C, X1E, P1P, P1S, A1 or A1 mini".into(),
            });
        }
        for field in ["camera_port", "ftp_port"] {
            if let Some(port) = settings.get(field).and_then(|p| p.as_f64())
                && !(1.0..=65535.0).contains(&port)
            {
                errors.push(gantry_core::FieldError {
                    field: field.into(),
                    message: "Must be a port, 1 to 65535".into(),
                });
            }
        }
        errors
    }

    fn connect(&self, printer: PrinterConfig, events: EventSender) -> Box<dyn PrinterHandle> {
        Box::new(BambuPrinter::spawn(printer, events))
    }
}

/// `host` or `host:port`.
fn parse_host(value: &str) -> Option<(String, u16)> {
    let value = value.trim();
    if value.is_empty() || value.contains(char::is_whitespace) {
        return None;
    }
    match value.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            let port = port.parse().ok().filter(|p| *p != 0)?;
            (!host.is_empty()).then(|| (host.to_owned(), port))
        }
        Some(_) => Some((value.trim_matches(['[', ']']).to_owned(), MQTT_PORT)),
        None => Some((value.to_owned(), MQTT_PORT)),
    }
}

/// The printer's camera, chosen by model, or `None` when the model is unknown.
fn camera_for(printer: &PrinterConfig) -> Option<Arc<dyn CameraSource>> {
    let (host, _) = printer.text("host").and_then(parse_host)?;
    let serial = printer.secrets.get("serial")?;
    let access_code = printer.secrets.get("access_code")?;
    let model = model::detect(printer.text("model"), serial)?;
    let port = printer
        .number("camera_port")
        .filter(|p| (1.0..=65535.0).contains(p))
        .map(|p| p as u16);
    Some(match model.camera() {
        CameraKind::Jpeg => camera::p1(&host, port.unwrap_or(camera::P1_CAMERA_PORT), access_code),
        CameraKind::Rtsps => camera::x1(&host, port.unwrap_or(camera::X1_CAMERA_PORT), access_code),
    })
}

/// Where to upload Print Files, or `None` without an address or access code.
fn ftps_for(printer: &PrinterConfig) -> Option<ftps::Target> {
    let (host, _) = printer.text("host").and_then(parse_host)?;
    let access_code = printer.secrets.get("access_code")?.to_owned();
    let port = printer
        .number("ftp_port")
        .filter(|p| (1.0..=65535.0).contains(p))
        .map_or(ftps::FTPS_PORT, |p| p as u16);
    Some(ftps::Target {
        host,
        port,
        access_code,
    })
}

/// A failed upload, as a command error. Never names the access code.
fn upload_failed(error: ftps::UploadError) -> CommandError {
    match error {
        ftps::UploadError::TimedOut => CommandError::TimedOut,
        other => CommandError::Refused {
            message: other.to_string(),
        },
    }
}

/// Where this connection's sequence ids start: high, and different each
/// time, so answers to Bambu Studio's or the phone app's commands (numbered
/// from 1) aren't mistaken for answers to Gantry's.
fn first_sequence() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::from(d.subsec_nanos()));
    20_000_000 + nanos % 10_000_000
}

/// What the handle and the connection task share.
struct Shared {
    client: AsyncClient,
    request_topic: String,
    events: EventSender,
    /// `None` while connected; the reason otherwise.
    down: Mutex<Option<DisconnectReason>>,
    sequence: AtomicU64,
    /// Gantry asked the current print to stop, so its end is a Cancel.
    cancel_requested: AtomicBool,
    /// Commands waiting for the printer's answer, by sequence id.
    pending: Mutex<HashMap<String, oneshot::Sender<Answer>>>,
    /// Why firmware refuses local control, while it does.
    blocked: Mutex<Option<String>>,
    /// The latest status, before control was withdrawn, so it can be sent
    /// again at once when that changes.
    latest: Mutex<Option<PrinterStatus>>,
    /// Whether the printer has a camera Gantry knows how to read.
    has_camera: bool,
}

impl Shared {
    fn next_sequence(&self) -> String {
        self.sequence.fetch_add(1, Ordering::SeqCst).to_string()
    }

    /// The status to report: the printer's own, with the control
    /// capabilities and the camera added, then control withdrawn if firmware
    /// refuses it.
    fn present(&self, mut status: PrinterStatus) -> PrinterStatus {
        status.capabilities.extend([
            Capability::Pause,
            Capability::Resume,
            Capability::Cancel,
            Capability::UploadAndStart,
        ]);
        if self.has_camera {
            status.capabilities.insert(Capability::Camera);
        }
        if let Some(why) = self.blocked.lock().expect("lock").clone() {
            status.withdraw_control(why);
        }
        status
    }

    /// Reports `status` (as the printer sent it) and remembers it.
    fn report(&self, status: PrinterStatus) {
        *self.latest.lock().expect("lock") = Some(status.clone());
        self.events.status(self.present(status));
    }

    /// Firmware refused control: withdraw it and tell the Fleet now.
    fn block(&self, why: String) {
        let already = self.blocked.lock().expect("lock").replace(why).is_some();
        if !already {
            tracing::warn!("Bambu printer refused local control; control withdrawn");
        }
        if let Some(latest) = self.latest.lock().expect("lock").clone() {
            self.events.status(self.present(latest));
        }
    }

    /// Refuses a command while disconnected or while firmware blocks control.
    fn ready(&self) -> Result<(), CommandError> {
        if let Some(reason) = *self.down.lock().expect("lock") {
            return Err(CommandError::Disconnected { reason });
        }
        if let Some(why) = self.blocked.lock().expect("lock").clone() {
            return Err(CommandError::Refused { message: why });
        }
        Ok(())
    }

    /// Whether the printer last reported any filament unit's slots.
    fn has_ams(&self) -> bool {
        self.latest
            .lock()
            .expect("lock")
            .as_ref()
            .is_some_and(|s| s.filament_slots.iter().any(|slot| slot.id != "ext"))
    }

    /// Publishes one request and waits for the printer's answer. Refused at
    /// once while disconnected, so nothing is queued for later (PRD §6.4).
    async fn request(
        &self,
        sequence: String,
        payload: serde_json::Value,
    ) -> Result<(), CommandError> {
        self.ready()?;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("lock")
            .insert(sequence.clone(), tx);
        // The connection may have dropped (and cleared what was waiting)
        // between the check above and now.
        if let Some(reason) = *self.down.lock().expect("lock") {
            self.pending.lock().expect("lock").remove(&sequence);
            return Err(CommandError::Disconnected { reason });
        }
        let published = self.client.try_publish(
            self.request_topic.as_str(),
            QoS::AtMostOnce,
            false,
            payload.to_string(),
        );
        if published.is_err() {
            self.pending.lock().expect("lock").remove(&sequence);
            return Err(CommandError::Refused {
                message: "the connection is busy; try again".into(),
            });
        }
        let answer = tokio::time::timeout(ANSWER_TIMEOUT, rx).await;
        self.pending.lock().expect("lock").remove(&sequence);
        match answer {
            Ok(Ok(Answer::Accepted)) | Err(_) => Ok(()),
            Ok(Err(_)) => Err(CommandError::Refused {
                message: "the connection dropped before the printer answered".into(),
            }),
            Ok(Ok(Answer::Refused(message))) => Err(CommandError::Refused { message }),
            Ok(Ok(Answer::ControlBlocked(why))) => {
                self.block(why.clone());
                Err(CommandError::Refused { message: why })
            }
        }
    }

    /// A `print` command with no parameter (pause, resume, stop).
    async fn command(&self, command: &str) -> Result<(), CommandError> {
        let sequence = self.next_sequence();
        let payload = control::print(&sequence, command, serde_json::json!({ "param": "" }));
        self.request(sequence, payload).await
    }
}

/// One connected Bambu Printer.
pub struct BambuPrinter {
    shared: Option<Arc<Shared>>,
    task: Option<JoinHandle<()>>,
    camera: Option<Arc<dyn CameraSource>>,
    /// From the settings, or the serial number.
    model: Option<Model>,
    /// Where Print Files are uploaded.
    ftps: Option<ftps::Target>,
}

impl BambuPrinter {
    fn spawn(printer: PrinterConfig, events: EventSender) -> Self {
        let camera = camera_for(&printer);
        let ftps = ftps_for(&printer);
        let model = printer
            .secrets
            .get("serial")
            .and_then(|serial| model::detect(printer.text("model"), serial));
        match Connection::prepare(&printer) {
            Ok((options, report_topic, request_topic)) => {
                let (client, eventloop) = AsyncClient::new(options, 32);
                let shared = Arc::new(Shared {
                    client,
                    request_topic,
                    events: events.clone(),
                    down: Mutex::new(Some(DisconnectReason::Unreachable)),
                    sequence: AtomicU64::new(first_sequence()),
                    cancel_requested: AtomicBool::new(false),
                    pending: Mutex::new(HashMap::new()),
                    blocked: Mutex::new(None),
                    latest: Mutex::new(None),
                    has_camera: camera.is_some(),
                });
                let task = tokio::spawn(run(
                    printer.name.clone(),
                    shared.clone(),
                    eventloop,
                    report_topic,
                ));
                Self {
                    shared: Some(shared),
                    task: Some(task),
                    camera,
                    model,
                    ftps,
                }
            }
            Err(status) => {
                events.status(*status);
                Self {
                    shared: None,
                    task: None,
                    camera: None,
                    model,
                    ftps: None,
                }
            }
        }
    }

    fn shared(&self) -> Result<&Shared, CommandError> {
        self.shared.as_deref().ok_or(CommandError::Disconnected {
            reason: DisconnectReason::AuthenticationRejected,
        })
    }
}

#[async_trait]
impl PrinterHandle for BambuPrinter {
    async fn pause(&self) -> Result<(), CommandError> {
        self.shared()?.command("pause").await
    }

    async fn resume(&self) -> Result<(), CommandError> {
        self.shared()?.command("resume").await
    }

    async fn cancel(&self) -> Result<(), CommandError> {
        let shared = self.shared()?;
        // Set first: the print may end before the answer arrives.
        shared.cancel_requested.store(true, Ordering::SeqCst);
        let result = shared.command("stop").await;
        if result.is_err() {
            shared.cancel_requested.store(false, Ordering::SeqCst);
        }
        result
    }

    /// Uploads the Print File over FTPS, then starts its Plate over MQTT.
    /// The printer's print key isn't known until it reports the print, so
    /// none is returned: the Fleet takes the next print it reports.
    async fn upload_and_start(&self, upload: PrintUpload) -> Result<Option<String>, CommandError> {
        let shared = self.shared()?;
        // Nothing is uploaded that couldn't be started.
        shared.ready()?;
        let target = self.ftps.as_ref().ok_or_else(|| CommandError::Refused {
            message: "no address or access code is set for uploads".into(),
        })?;
        let name = ftps::file_name(&upload.file_name)
            .ok_or_else(|| CommandError::invalid("the Print File has no usable name"))?;
        // Checked first, so a file that can't be started isn't sent.
        control::start("0", &name, upload.plate, &[], false).map_err(CommandError::invalid)?;

        target
            .upload(&name, &upload.data)
            .await
            .map_err(upload_failed)?;

        let sequence = shared.next_sequence();
        let payload = control::start(
            &sequence,
            &name,
            upload.plate,
            &upload.data,
            shared.has_ams(),
        )
        .map_err(CommandError::invalid)?;
        shared.request(sequence, payload).await?;
        Ok(None)
    }

    async fn fine(&self, control: FineControl) -> Result<(), CommandError> {
        let shared = self.shared()?;
        // Moving the toolhead or changing filament would wreck a print.
        if matches!(
            control,
            FineControl::Home
                | FineControl::Jog { .. }
                | FineControl::LoadFilament { .. }
                | FineControl::UnloadFilament
        ) && shared
            .latest
            .lock()
            .expect("lock")
            .as_ref()
            .and_then(|s| s.state)
            .is_some_and(gantry_core::PrinterState::has_job)
        {
            return Err(CommandError::invalid_state(
                "not while a Job is running: pause or finish it first",
            ));
        }
        let sequence = shared.next_sequence();
        let payload = control::fine(&sequence, &control).map_err(CommandError::invalid)?;
        shared.request(sequence, payload).await
    }

    fn camera(&self) -> Option<Arc<dyn CameraSource>> {
        self.camera.clone()
    }

    fn model(&self) -> Option<String> {
        self.model.map(|m| m.name().to_owned())
    }

    async fn shutdown(&self) {
        if let Some(shared) = &self.shared {
            let _ = shared.client.try_disconnect();
        }
        if let Some(task) = &self.task {
            task.abort();
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Setting up a connection from a Printer's config.
struct Connection;

impl Connection {
    /// The MQTT options and topics, or the status that says why there are none.
    #[allow(clippy::type_complexity)]
    fn prepare(
        printer: &PrinterConfig,
    ) -> Result<(MqttOptions, String, String), Box<PrinterStatus>> {
        let Some((host, port)) = printer.text("host").and_then(parse_host) else {
            return Err(Box::new(disconnected(
                DisconnectReason::Unreachable,
                "no valid address is set",
            )));
        };
        let (Some(serial), Some(access_code)) = (
            printer.secrets.get("serial"),
            printer.secrets.get("access_code"),
        ) else {
            return Err(Box::new(disconnected(
                DisconnectReason::AuthenticationRejected,
                "the serial number or access code is missing; re-enter them",
            )));
        };
        let client_id = format!(
            "gantry-{}-{}",
            printer.id.chars().take(8).collect::<String>(),
            std::process::id()
        );
        let mut options = MqttOptions::new(client_id, host, port);
        options
            .set_credentials(USERNAME, access_code)
            .set_keep_alive(Duration::from_secs(30))
            // Full reports run to tens of kilobytes.
            .set_max_packet_size(1 << 20, 64 << 10)
            .set_transport(Transport::Tls(TlsConfiguration::Rustls(
                tls::client_config(),
            )));
        Ok((
            options,
            format!("device/{serial}/report"),
            format!("device/{serial}/request"),
        ))
    }
}

/// `print_error` when a print was stopped by its user.
const PRINT_ERROR_CANCELLED: u64 = 0x0300_400C;

/// How a print the printer has stopped running ended.
fn outcome(report: &Report, cancel_requested: bool) -> JobOutcome {
    match report.gcode_state() {
        GcodeState::Finished => JobOutcome::Succeeded,
        GcodeState::Failed if cancel_requested || report.print_error() == PRINT_ERROR_CANCELLED => {
            JobOutcome::Cancelled
        }
        GcodeState::Failed => JobOutcome::Failed,
        _ if cancel_requested => JobOutcome::Cancelled,
        // It stopped without saying how.
        _ => JobOutcome::Lost,
    }
}

/// Why a connection attempt failed, in the status model's terms.
fn classify(error: &ConnectionError) -> (DisconnectReason, String) {
    match error {
        ConnectionError::ConnectionRefused(
            ConnectReturnCode::NotAuthorized | ConnectReturnCode::BadUserNamePassword,
        ) => (
            DisconnectReason::AuthenticationRejected,
            "the printer refused the access code".into(),
        ),
        ConnectionError::ConnectionRefused(code) => (
            DisconnectReason::ProtocolError,
            format!("the printer refused the connection ({code:?})"),
        ),
        ConnectionError::Tls(e) => (DisconnectReason::TlsFailure, format!("TLS: {e}")),
        ConnectionError::Io(e) => (DisconnectReason::Unreachable, e.kind().to_string()),
        ConnectionError::NetworkTimeout | ConnectionError::FlushTimeout => {
            (DisconnectReason::Unreachable, "timed out".into())
        }
        ConnectionError::MqttState(e) => (DisconnectReason::Unreachable, e.to_string()),
        other => (DisconnectReason::ProtocolError, other.to_string()),
    }
}

fn disconnected(reason: DisconnectReason, detail: impl Into<String>) -> PrinterStatus {
    PrinterStatus {
        connection: gantry_core::Connection::Disconnected {
            reason,
            detail: Some(detail.into()),
        },
        ..PrinterStatus::disconnected(reason)
    }
}

async fn run(
    name: String,
    shared: Arc<Shared>,
    mut eventloop: rumqttc::EventLoop,
    report_topic: String,
) {
    eventloop.network_options.set_connection_timeout(10);
    let events = shared.events.clone();
    let mut report = Report::default();
    let mut backoff = BACKOFF_MIN;
    let mut connected = false;
    // The key of the print the printer was last seen running.
    let mut running: Option<String> = None;
    // Whether this connection's first status report is still to come.
    let mut first_report = true;
    events.status(PrinterStatus::connecting());

    loop {
        if events.is_closed() {
            break;
        }
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                backoff = BACKOFF_MIN;
                first_report = true;
                // A new connection: control is tried afresh. Firmware that
                // still refuses it says so at the next command.
                if shared.blocked.lock().expect("lock").take().is_some() {
                    tracing::info!(printer = %name, "reconnected; offering control again");
                }
                let _ = shared
                    .client
                    .try_subscribe(report_topic.as_str(), QoS::AtMostOnce);
                // Ask for everything at once rather than waiting for the next
                // full report.
                let pushall = serde_json::json!({
                    "pushing": { "sequence_id": "0", "command": "pushall" }
                });
                let _ = shared.client.try_publish(
                    shared.request_topic.as_str(),
                    QoS::AtMostOnce,
                    false,
                    pushall.to_string(),
                );
                tracing::info!(printer = %name, "connected to a Bambu printer");
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&publish.payload)
                else {
                    tracing::debug!(printer = %name, "ignored a report that isn't JSON");
                    continue;
                };
                if let Some((sequence, answer)) = control::answer(&payload) {
                    let waiting = shared.pending.lock().expect("lock").remove(&sequence);
                    // Only answers to Gantry's own commands count: another
                    // app's refusal says nothing about Gantry's control.
                    if let Some(reply) = waiting {
                        let _ = reply.send(answer);
                    }
                    continue;
                }
                if !report.apply(&payload) {
                    continue;
                }
                connected = true;
                *shared.down.lock().expect("lock") = None;

                let now = report.job().map(|j| j.key);
                // A partial update may come before the full one: wait for a
                // report that says what the printer is doing.
                let first = first_report && report.knows_gcode_state();
                if first {
                    first_report = false;
                }
                if first
                    && running.is_none()
                    && now.is_none()
                    && matches!(
                        report.gcode_state(),
                        GcodeState::Finished | GcodeState::Failed
                    )
                {
                    // A print that ended while Gantry wasn't watching (it was
                    // restarting, or the printer was off the network): say how,
                    // so its open Job gets its real Outcome rather than Lost.
                    // The Fleet ignores an ending it has already recorded.
                    events.job_ended(report.job_key(), outcome(&report, false));
                }
                if running.is_some() && running != now {
                    // The print we saw running has stopped (or another
                    // replaced it, which the Fleet records as Lost).
                    let key = running.take().expect("checked above");
                    if now.is_none() {
                        let cancelled = shared.cancel_requested.swap(false, Ordering::SeqCst);
                        events.job_ended(key, outcome(&report, cancelled));
                    }
                }
                if now.is_some() && running.is_none() {
                    shared.cancel_requested.store(false, Ordering::SeqCst);
                }
                running = now;

                shared.report(report.status());
            }
            Ok(_) => {}
            Err(error) => {
                let (reason, detail) = classify(&error);
                if connected || backoff == BACKOFF_MIN {
                    tracing::warn!(printer = %name, reason = reason.describe(), "Bambu printer disconnected");
                }
                connected = false;
                *shared.down.lock().expect("lock") = Some(reason);
                // Nothing will answer the commands still waiting.
                shared.pending.lock().expect("lock").clear();
                events.status(disconnected(reason, detail));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
    let _ = shared.client.try_disconnect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addresses() {
        assert_eq!(
            parse_host("192.168.1.20"),
            Some(("192.168.1.20".into(), 8883))
        );
        assert_eq!(
            parse_host("printer.lan:1883"),
            Some(("printer.lan".into(), 1883))
        );
        assert_eq!(parse_host("10.0.0.5:0"), None);
        assert_eq!(parse_host("a b"), None);
        assert_eq!(parse_host(""), None);
    }

    #[test]
    fn outcomes_follow_the_reported_state() {
        use gantry_mock_bambu::fixtures;
        let ended = |name: &str| {
            let mut r = Report::default();
            r.apply(&fixtures::load("printing"));
            r.apply(&fixtures::load(name));
            r
        };
        assert_eq!(outcome(&ended("finished"), false), JobOutcome::Succeeded);
        assert_eq!(outcome(&ended("failed"), false), JobOutcome::Failed);
        assert_eq!(outcome(&ended("cancelled"), false), JobOutcome::Cancelled);
        assert_eq!(
            outcome(&ended("failed"), true),
            JobOutcome::Cancelled,
            "Gantry asked it to stop"
        );
        assert_eq!(outcome(&ended("idle"), false), JobOutcome::Lost);
    }

    #[test]
    fn the_serial_and_access_code_are_credentials() {
        let secret: Vec<String> = BambuDriver
            .fields()
            .into_iter()
            .filter(|f| f.kind == FieldKind::Secret)
            .map(|f| f.key)
            .collect();
        assert_eq!(secret, ["serial", "access_code"]);
    }
}
