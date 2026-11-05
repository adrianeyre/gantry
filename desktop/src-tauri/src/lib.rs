//! Gantry's desktop app (PRD §5.2, ADR-0008): the same React UI the server
//! embeds, in a native window. It is either a client of a Gantry server the
//! person chooses, or runs the core and server itself on 127.0.0.1.
//!
//! Everything but the private `shell` module is plain Rust with no window,
//! so it is tested without one.

pub mod desktop_config;
pub mod drivers;
pub mod embedded;
pub mod navigation;
pub mod probe;
pub mod secret_key;
pub mod server_url;
mod shell;

pub use shell::run;
