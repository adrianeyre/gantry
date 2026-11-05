//! Gantry's core crate.
//!
//! It holds what every Driver and the server share: the [`Driver`] trait, the
//! normalised status model and the running [`Fleet`]. It knows nothing about
//! any particular printer protocol: Driver crates depend on it, never the
//! reverse (ADR-0003).

pub mod driver;
pub mod fleet;
pub mod job;
pub mod registry;
pub mod status;

pub use driver::{
    Axis, CameraError, CameraSource, CommandError, Driver, DriverEvent, EventReceiver, EventSender,
    FieldError, FieldKind, FieldSpec, FineControl, Heater, PrintUpload, PrinterConfig,
    PrinterHandle, PrinterId, Secrets, UPLOAD_TIMEOUT,
};
pub use fleet::{
    Command, Fleet, FleetError, FleetUpdate, LastSeen, Overrides, PrinterView, StartOptions,
};
pub use job::{
    JobId, JobLog, JobLogError, JobOutcome, JobRecord, JobStart, JobState, MemoryJobLog,
    START_GRACE,
};
pub use registry::{DriverInfo, DriverRegistry};
pub use status::{
    ActiveJob, Capability, Connection, DisconnectReason, Fan, Fault, FaultSeverity, Filament,
    FilamentSlot, Light, LightId, Position, PrinterState, PrinterStatus, Temperature, Temperatures,
};

/// The version of Gantry, shared by the server and the binary.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
