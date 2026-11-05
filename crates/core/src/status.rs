//! The normalised status model (PRD §6.2, §10.1). Every Driver translates its
//! protocol into these types, so nothing above a Driver knows which protocol a
//! Printer speaks.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Whether Gantry can currently talk to a Printer. Separate from
/// [`PrinterState`]: firmware blocking control is *not* a Connection problem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Connection {
    Connected,
    Connecting,
    Disconnected {
        reason: DisconnectReason,
        /// A human-readable detail, never containing a credential.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

impl Connection {
    /// A disconnection with no further detail.
    pub fn disconnected(reason: DisconnectReason) -> Self {
        Self::Disconnected {
            reason,
            detail: None,
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected)
    }
}

/// Why a Printer is disconnected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisconnectReason {
    /// Nothing answered at the Printer's address.
    Unreachable,
    /// The Printer refused the credentials.
    AuthenticationRejected,
    /// The secure connection could not be set up.
    TlsFailure,
    /// The Printer answered, but not in a way the Driver understands.
    ProtocolError,
    /// Gantry is not trying to connect (the Printer is being removed, or
    /// Gantry is shutting down).
    Stopped,
}

impl DisconnectReason {
    /// How the UI names the reason.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::AuthenticationRejected => "authentication rejected",
            Self::TlsFailure => "TLS failure",
            Self::ProtocolError => "protocol error",
            Self::Stopped => "stopped",
        }
    }
}

/// What a connected Printer is doing. There is no "error" state: Faults sit
/// alongside any state (PRD §6.2). Unknown (`None` in [`PrinterStatus`])
/// while the Printer is disconnected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrinterState {
    /// Ready for a Job.
    Idle,
    /// A Job has started but isn't printing yet (heating, levelling).
    Preparing,
    Printing,
    Paused,
    /// A Job has ended and the part may still be on the bed. Gantry, not the
    /// Driver, puts a Printer here, and only Mark Clear takes it out.
    AwaitingClear,
    /// Doing something that isn't a Job (homing, loading filament, calibrating).
    Busy,
}

impl PrinterState {
    /// Whether a Job is running (in any phase).
    pub fn has_job(self) -> bool {
        matches!(self, Self::Preparing | Self::Printing | Self::Paused)
    }
}

/// One temperature reading, in °C.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Temperature {
    pub current: f64,
    /// The target, when the heater is set to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<f64>,
}

/// The temperatures a Printer reports. Absent sensors are `None`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Temperatures {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle: Option<Temperature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bed: Option<Temperature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chamber: Option<Temperature>,
}

/// One fan and its speed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fan {
    /// Stable within the Printer, such as `part`, and how a fan control names it.
    #[serde(default)]
    pub id: String,
    /// What the fan is for, such as "part cooling" or "auxiliary".
    pub name: String,
    /// 0 to 100.
    pub percent: u8,
    /// Whether Gantry can set its speed (with [`Capability::Fans`]).
    #[serde(default)]
    pub controllable: bool,
}

/// A light Gantry can switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightId {
    /// Lights the build chamber (and the camera's view).
    Chamber,
    /// A separate light over the work, such as the X1's toolhead light.
    Work,
}

/// A light and whether it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Light {
    pub id: LightId,
    pub on: bool,
}

/// Where the toolhead is, in mm, when the Printer says.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// The filament in a Filament Slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Filament {
    /// The material, such as PLA or PETG.
    pub kind: String,
    /// `#RRGGBB`.
    pub colour: String,
    /// 0 to 100, when the Printer knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_percent: Option<u8>,
}

/// One position in a Printer's filament supply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilamentSlot {
    /// Stable within the Printer, such as `0` to `3` for one unit of four, or
    /// `ext` for an external spool.
    pub id: String,
    /// How the UI names it, such as "Unit 1 · Slot 2".
    pub label: String,
    /// `None` when the slot is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filament: Option<Filament>,
    /// Whether its filament is at or below the low-filament threshold. The
    /// Fleet sets this, not the Driver (PRD §7.1).
    #[serde(default)]
    pub low: bool,
}

/// How serious a Fault is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultSeverity {
    Warning,
    Error,
}

/// A problem a Printer reports about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fault {
    /// The Driver's code for it, stable while the Fault lasts.
    pub code: String,
    pub severity: FaultSeverity,
    pub message: String,
}

/// Something a Printer can do. The UI hides controls whose capability is
/// absent (PRD §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Pause,
    Resume,
    Cancel,
    UploadAndStart,
    Camera,
    FilamentSlots,
    ChamberTemperature,
    /// Choosing a speed profile, such as silent or sport (PRD §7.2).
    SpeedProfile,
    /// Setting the nozzle's target temperature.
    NozzleTemperature,
    /// Setting the bed's target temperature.
    BedTemperature,
    ChamberLight,
    WorkLight,
    /// Setting the speed of the fans marked controllable.
    Fans,
    Homing,
    Jogging,
    /// Loading and unloading filament.
    FilamentLoad,
}

impl Capability {
    /// Every capability, in the order the UI lists them.
    pub const ALL: [Capability; 16] = [
        Capability::Pause,
        Capability::Resume,
        Capability::Cancel,
        Capability::UploadAndStart,
        Capability::Camera,
        Capability::FilamentSlots,
        Capability::ChamberTemperature,
        Capability::SpeedProfile,
        Capability::NozzleTemperature,
        Capability::BedTemperature,
        Capability::ChamberLight,
        Capability::WorkLight,
        Capability::Fans,
        Capability::Homing,
        Capability::Jogging,
        Capability::FilamentLoad,
    ];

    /// The capabilities that control a Printer, withdrawn together when
    /// firmware blocks control (PRD §6.1).
    pub const CONTROL: [Capability; 13] = [
        Capability::Pause,
        Capability::Resume,
        Capability::Cancel,
        Capability::UploadAndStart,
        Capability::SpeedProfile,
        Capability::NozzleTemperature,
        Capability::BedTemperature,
        Capability::ChamberLight,
        Capability::WorkLight,
        Capability::Fans,
        Capability::Homing,
        Capability::Jogging,
        Capability::FilamentLoad,
    ];

    /// The fine controls (PRD §7.2).
    pub const FINE: [Capability; 9] = [
        Capability::SpeedProfile,
        Capability::NozzleTemperature,
        Capability::BedTemperature,
        Capability::ChamberLight,
        Capability::WorkLight,
        Capability::Fans,
        Capability::Homing,
        Capability::Jogging,
        Capability::FilamentLoad,
    ];
}

/// The Job a Printer is running, as the Driver sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveJob {
    /// The Driver's identity for this print, stable for its whole life and
    /// different for the next one.
    pub key: String,
    /// The Print File's name, when the Printer says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_file: Option<String>,
    /// The Plate within the Print File, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plate: Option<u32>,
    /// 0 to 100.
    pub progress: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_layers: Option<u32>,
    /// Seconds until the Job is expected to finish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_secs: Option<u64>,
    /// When the Printer says the print started, when it says. With the Print
    /// File's name, this re-attaches a print to its Job after a restart even
    /// if the Driver's key for it changed (PRD §7.5).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub started_at: Option<time::OffsetDateTime>,
}

/// Everything known about a Printer at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrinterStatus {
    pub connection: Connection,
    /// `None` while disconnected: the state is unknown.
    pub state: Option<PrinterState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<ActiveJob>,
    pub temperatures: Temperatures,
    #[serde(default)]
    pub fans: Vec<Fan>,
    #[serde(default)]
    pub filament_slots: Vec<FilamentSlot>,
    #[serde(default)]
    pub faults: Vec<Fault>,
    /// The speed profile in use, by name, when the Printer has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_profile: Option<String>,
    /// The speed profiles it offers, in order, such as `silent`, `standard`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speed_profiles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<Light>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// The Filament Slot whose filament is loaded in the toolhead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_slot: Option<String>,
    /// What this Printer can do right now, reported when the Driver connects.
    pub capabilities: BTreeSet<Capability>,
    /// Why control capabilities are missing, when the Driver withdrew them
    /// (for example, firmware blocking local control).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_withdrawn: Option<String>,
}

impl PrinterStatus {
    /// A Printer Gantry has started connecting to and knows nothing about.
    pub fn connecting() -> Self {
        Self {
            connection: Connection::Connecting,
            ..Self::disconnected(DisconnectReason::Stopped)
        }
    }

    /// A Printer that is disconnected, with nothing else known.
    pub fn disconnected(reason: DisconnectReason) -> Self {
        Self {
            connection: Connection::disconnected(reason),
            state: None,
            job: None,
            temperatures: Temperatures::default(),
            fans: Vec::new(),
            filament_slots: Vec::new(),
            faults: Vec::new(),
            speed_profile: None,
            speed_profiles: Vec::new(),
            lights: Vec::new(),
            position: None,
            loaded_slot: None,
            capabilities: BTreeSet::new(),
            control_withdrawn: None,
        }
    }

    /// Removes every control capability, saying why (PRD §6.1): the Printer
    /// stays connected and its status stays live.
    pub fn withdraw_control(&mut self, why: impl Into<String>) {
        for capability in Capability::CONTROL {
            self.capabilities.remove(&capability);
        }
        self.control_withdrawn = Some(why.into());
    }

    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_serialises_with_its_reason() {
        let json = serde_json::to_value(Connection::disconnected(
            DisconnectReason::AuthenticationRejected,
        ))
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "state": "disconnected", "reason": "authentication_rejected" })
        );
    }

    #[test]
    fn printer_states_have_no_error_state() {
        for state in [
            "idle",
            "preparing",
            "printing",
            "paused",
            "awaiting_clear",
            "busy",
        ] {
            let parsed: PrinterState = serde_json::from_value(serde_json::json!(state)).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), state);
        }
        assert!(serde_json::from_value::<PrinterState>(serde_json::json!("error")).is_err());
    }

    #[test]
    fn faults_sit_alongside_any_state() {
        let mut status = PrinterStatus::disconnected(DisconnectReason::Stopped);
        status.connection = Connection::Connected;
        status.state = Some(PrinterState::Printing);
        status.faults.push(Fault {
            code: "door".into(),
            severity: FaultSeverity::Error,
            message: "Door open".into(),
        });
        assert_eq!(status.state, Some(PrinterState::Printing));
    }
}
