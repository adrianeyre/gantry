//! The `Driver` trait (PRD §6.1, §10.1, ADR-0003).
//!
//! A Driver speaks one protocol and serves many Printers. For each Printer it
//! starts a connection that runs in its own tokio task, reports status through
//! an event stream, and takes commands through a [`PrinterHandle`].
use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::job::JobOutcome;
use crate::status::{Capability, DisconnectReason, LightId, PrinterStatus};

/// A Printer's identity within the Fleet.
pub type PrinterId = String;

/// The kind of value a connection field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Number,
    Boolean,
    /// A credential: encrypted at rest and never sent back to the browser
    /// (ADR-0005).
    Secret,
}

/// One setting a Driver needs for each Printer, such as an address or an
/// access code. The UI builds its form from these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSpec {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

impl FieldSpec {
    pub fn new(key: &str, label: &str, kind: FieldKind) -> Self {
        Self {
            key: key.to_owned(),
            label: label.to_owned(),
            kind,
            required: false,
            help: None,
            default: None,
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn help(mut self, help: &str) -> Self {
        self.help = Some(help.to_owned());
        self
    }

    pub fn default_value(mut self, value: serde_json::Value) -> Self {
        self.default = Some(value);
        self
    }
}

/// Why a field's value was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

/// Credentials, kept apart from settings so they are only ever handled
/// deliberately. `Debug` never shows the values.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secrets(BTreeMap<String, String>);

impl Secrets {
    pub fn new(values: BTreeMap<String, String>) -> Self {
        Self(values)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn into_inner(self) -> BTreeMap<String, String> {
        self.0
    }
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

/// Everything a Driver is given to connect one Printer.
#[derive(Debug, Clone)]
pub struct PrinterConfig {
    pub id: PrinterId,
    pub name: String,
    /// Non-secret settings, by [`FieldSpec::key`].
    pub settings: serde_json::Map<String, serde_json::Value>,
    /// Decrypted credentials, by [`FieldSpec::key`].
    pub secrets: Secrets,
}

impl PrinterConfig {
    pub fn text(&self, key: &str) -> Option<&str> {
        self.settings
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
    }

    pub fn boolean(&self, key: &str) -> Option<bool> {
        self.settings.get(key).and_then(serde_json::Value::as_bool)
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        self.settings.get(key).and_then(serde_json::Value::as_f64)
    }
}

/// What a Printer's connection reports.
#[derive(Debug, Clone, PartialEq)]
pub enum DriverEvent {
    /// The Printer's whole status as of now.
    Status(Box<PrinterStatus>),
    /// The Job with this key has ended.
    JobEnded { key: String, outcome: JobOutcome },
}

/// Where a Printer's connection sends its events. Sending never blocks, so a
/// Driver can't be stalled by a slow consumer.
#[derive(Debug, Clone)]
pub struct EventSender(mpsc::UnboundedSender<DriverEvent>);

/// The receiving end of an [`EventSender`].
pub type EventReceiver = mpsc::UnboundedReceiver<DriverEvent>;

impl EventSender {
    /// A connected pair.
    pub fn channel() -> (Self, EventReceiver) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }

    pub fn status(&self, status: PrinterStatus) {
        let _ = self.0.send(DriverEvent::Status(Box::new(status)));
    }

    pub fn job_ended(&self, key: impl Into<String>, outcome: JobOutcome) {
        let _ = self.0.send(DriverEvent::JobEnded {
            key: key.into(),
            outcome,
        });
    }

    /// Whether nobody is listening any more.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

/// Why a command didn't happen. Commands are never held for later.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandError {
    #[error("the Printer is disconnected ({})", .reason.describe())]
    Disconnected { reason: DisconnectReason },
    #[error("this Printer doesn't support that")]
    Unsupported,
    /// The Printer isn't in a state where the command makes sense, such as
    /// pausing when nothing is printing.
    #[error("{message}")]
    InvalidState { message: String },
    #[error("the Printer refused: {message}")]
    Refused { message: String },
    #[error("the Printer didn't answer in time")]
    TimedOut,
    /// The command itself is wrong, such as a temperature out of range.
    #[error("{message}")]
    Invalid { message: String },
}

impl CommandError {
    pub fn invalid_state(message: impl Into<String>) -> Self {
        Self::InvalidState {
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid {
            message: message.into(),
        }
    }
}

/// A heater a fine control can set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Heater {
    Nozzle,
    Bed,
}

/// An axis to jog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    X,
    Y,
    Z,
}

/// The hottest target any fine control may set, in °C. A Driver may refuse
/// less, for what its Printer can take.
pub const MAX_TARGET_TEMPERATURE: f64 = 350.0;

/// The longest single jog, in mm.
pub const MAX_JOG_MM: f64 = 100.0;

/// A fine control (PRD §7.2). Each needs its own [`Capability`], and reaches
/// the Driver through [`PrinterHandle::fine`]: nothing above a Driver knows
/// how a Printer does it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum FineControl {
    /// One of the Printer's own [`speed_profiles`](PrinterStatus::speed_profiles).
    SetSpeedProfile {
        profile: String,
    },
    /// A target temperature, in °C. 0 turns the heater off.
    SetTemperature {
        heater: Heater,
        target: f64,
    },
    SetLight {
        light: LightId,
        on: bool,
    },
    /// A fan's speed, 0 to 100, by [`Fan::id`](crate::Fan::id).
    SetFan {
        fan: String,
        percent: u8,
    },
    /// Homes every axis.
    Home,
    /// Moves one axis by `distance_mm` (negative for the other way).
    Jog {
        axis: Axis,
        distance_mm: f64,
    },
    /// Loads the filament in a Filament Slot into the toolhead.
    LoadFilament {
        slot: String,
    },
    /// Unloads whatever filament is in the toolhead.
    UnloadFilament,
}

impl FineControl {
    /// The capability the Printer must have for it.
    pub fn capability(&self) -> Capability {
        match self {
            Self::SetSpeedProfile { .. } => Capability::SpeedProfile,
            Self::SetTemperature {
                heater: Heater::Nozzle,
                ..
            } => Capability::NozzleTemperature,
            Self::SetTemperature {
                heater: Heater::Bed,
                ..
            } => Capability::BedTemperature,
            Self::SetLight {
                light: LightId::Chamber,
                ..
            } => Capability::ChamberLight,
            Self::SetLight {
                light: LightId::Work,
                ..
            } => Capability::WorkLight,
            Self::SetFan { .. } => Capability::Fans,
            Self::Home => Capability::Homing,
            Self::Jog { .. } => Capability::Jogging,
            Self::LoadFilament { .. } | Self::UnloadFilament => Capability::FilamentLoad,
        }
    }

    /// Checks what can be checked without the Printer: ranges and blanks.
    pub fn validate(&self) -> Result<(), CommandError> {
        match self {
            Self::SetSpeedProfile { profile } if profile.trim().is_empty() => {
                Err(CommandError::invalid("choose a speed profile"))
            }
            Self::SetTemperature { target, .. }
                if !(target.is_finite() && (0.0..=MAX_TARGET_TEMPERATURE).contains(target)) =>
            {
                Err(CommandError::invalid(format!(
                    "the target must be 0 to {MAX_TARGET_TEMPERATURE} °C"
                )))
            }
            Self::SetFan { percent, .. } if *percent > 100 => {
                Err(CommandError::invalid("a fan's speed is 0 to 100%"))
            }
            Self::Jog { distance_mm, .. }
                if !(distance_mm.is_finite()
                    && *distance_mm != 0.0
                    && distance_mm.abs() <= MAX_JOG_MM) =>
            {
                Err(CommandError::invalid(format!(
                    "a jog moves 0.1 to {MAX_JOG_MM} mm either way"
                )))
            }
            Self::LoadFilament { slot } if slot.trim().is_empty() => {
                Err(CommandError::invalid("choose a Filament Slot"))
            }
            _ => Ok(()),
        }
    }
}

/// A Plate to send to a Printer and start (PRD §7.3).
#[derive(Clone)]
pub struct PrintUpload {
    /// The Print File's name, as the Printer will store and report it.
    pub file_name: String,
    /// Which Plate to print, from 1. `None` for a plain `.gcode`, which is
    /// one Plate.
    pub plate: Option<u32>,
    /// The whole Print File.
    pub data: Bytes,
    /// The slicer's estimate for this Plate, in seconds, when known.
    pub estimated_secs: Option<u64>,
    /// The Plate's layer count, when known.
    pub layers: Option<u32>,
}

impl std::fmt::Debug for PrintUpload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrintUpload")
            .field("file_name", &self.file_name)
            .field("plate", &self.plate)
            .field("bytes", &self.data.len())
            .field("estimated_secs", &self.estimated_secs)
            .field("layers", &self.layers)
            .finish()
    }
}

/// How long an upload-and-start may take, transfer included, before the
/// Fleet gives up on it. Print Files can be tens of megabytes.
pub const UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// A camera's frames could not be read.
#[derive(Debug, Clone, thiserror::Error)]
#[error("camera: {0}")]
pub struct CameraError(pub String);

/// A Printer's camera. Opening it starts pulling frames from the Printer;
/// dropping the stream stops.
pub trait CameraSource: Send + Sync {
    /// JPEG frames, as they arrive.
    fn open(&self) -> BoxStream<'static, Result<Bytes, CameraError>>;
}

/// Commands to one connected Printer. Implementations must answer promptly or
/// time out themselves: the Fleet also bounds every command with a timeout.
#[async_trait]
pub trait PrinterHandle: Send + Sync {
    async fn pause(&self) -> Result<(), CommandError>;
    async fn resume(&self) -> Result<(), CommandError>;
    async fn cancel(&self) -> Result<(), CommandError>;

    /// Sends a Print File to the Printer and starts one of its Plates (PRD
    /// §6.1). Returns the Driver's key for the new print when it knows it at
    /// once; otherwise the Fleet takes the next print the Printer reports to
    /// be this one. An error means nothing was started.
    async fn upload_and_start(&self, _upload: PrintUpload) -> Result<Option<String>, CommandError> {
        Err(CommandError::Unsupported)
    }

    /// The printer model, such as `X1C`, when the Driver knows it. A Print
    /// File is only sent to a Printer of the model it was sliced for.
    fn model(&self) -> Option<String> {
        None
    }

    /// A fine control (PRD §7.2). The Fleet has already checked the Printer
    /// is connected, has the capability, and that the values are in range.
    async fn fine(&self, _control: FineControl) -> Result<(), CommandError> {
        Err(CommandError::Unsupported)
    }

    /// The camera, when the Printer has one.
    fn camera(&self) -> Option<Arc<dyn CameraSource>> {
        None
    }

    /// Stops the connection's task and frees its resources. The Printer is
    /// not told anything.
    async fn shutdown(&self);

    /// For Driver-specific extras the server exposes (such as the Simulated
    /// Driver's simulation panel), found by downcasting.
    fn as_any(&self) -> &dyn Any;
}

/// One protocol.
pub trait Driver: Send + Sync + 'static {
    /// A stable identifier, stored with each Printer, such as `simulated`.
    fn id(&self) -> &'static str;

    /// How the UI names the Driver.
    fn name(&self) -> &'static str;

    /// The settings each Printer needs.
    fn fields(&self) -> Vec<FieldSpec>;

    /// Checks settings beyond what [`fields`](Self::fields) can say. The
    /// default checks required fields are present and types match.
    fn validate(
        &self,
        settings: &serde_json::Map<String, serde_json::Value>,
        secrets: &Secrets,
    ) -> Vec<FieldError> {
        validate_fields(&self.fields(), settings, secrets)
    }

    /// Starts connecting one Printer, in its own task, sending events to
    /// `events`. Must be called within a tokio runtime.
    fn connect(&self, printer: PrinterConfig, events: EventSender) -> Box<dyn PrinterHandle>;
}

/// The checks every Driver gets: required fields present, values of the
/// declared kind.
pub fn validate_fields(
    fields: &[FieldSpec],
    settings: &serde_json::Map<String, serde_json::Value>,
    secrets: &Secrets,
) -> Vec<FieldError> {
    let mut errors = Vec::new();
    for field in fields {
        let error = |message: &str| FieldError {
            field: field.key.clone(),
            message: message.to_owned(),
        };
        if field.kind == FieldKind::Secret {
            if field.required && secrets.get(&field.key).is_none_or(str::is_empty) {
                errors.push(error("Required"));
            }
            continue;
        }
        match settings.get(&field.key) {
            None | Some(serde_json::Value::Null) => {
                if field.required {
                    errors.push(error("Required"));
                }
            }
            Some(serde_json::Value::String(s)) if field.kind == FieldKind::Text => {
                if field.required && s.trim().is_empty() {
                    errors.push(error("Required"));
                }
            }
            Some(serde_json::Value::Number(_)) if field.kind == FieldKind::Number => {}
            Some(serde_json::Value::Bool(_)) if field.kind == FieldKind::Boolean => {}
            Some(_) => errors.push(error(match field.kind {
                FieldKind::Text | FieldKind::Secret => "Must be text",
                FieldKind::Number => "Must be a number",
                FieldKind::Boolean => "Must be true or false",
            })),
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_debug_shows_only_keys() {
        let secrets = Secrets::new([("access_code".to_owned(), "12345678".to_owned())].into());
        let debug = format!("{secrets:?}");
        assert!(debug.contains("access_code"));
        assert!(!debug.contains("12345678"));
    }

    #[test]
    fn validates_required_fields_and_kinds() {
        let fields = vec![
            FieldSpec::new("host", "Address", FieldKind::Text).required(),
            FieldSpec::new("port", "Port", FieldKind::Number),
            FieldSpec::new("code", "Code", FieldKind::Secret).required(),
        ];
        let settings = serde_json::json!({ "host": " ", "port": "x" });
        let errors = validate_fields(&fields, settings.as_object().unwrap(), &Secrets::default());
        let fields: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, ["host", "port", "code"]);
    }
}
