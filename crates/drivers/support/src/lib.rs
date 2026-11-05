//! What several Drivers share, kept out of the core crate (ADR-0003): the
//! core knows nothing of any protocol, and these are protocol details.
//!
//! - [`mjpeg`]: a [`CameraSource`](gantry_core::CameraSource) for the MJPEG
//!   (or plain JPEG snapshot) cameras Moonraker, OctoPrint and others serve
//!   over HTTP, which plugs into the server's camera relay like any other.
//! - [`plate`]: the G-code of one Plate of a Print File, for printers that
//!   print plain G-code and not a `.gcode.3mf`.
pub mod mjpeg;
pub mod plate;
