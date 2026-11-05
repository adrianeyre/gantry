# Drivers are compiled-in crates, not runtime plugins

Each Driver is its own crate, linked into the binary at compile time and selected with a Cargo feature. Adding a Driver needs no change to the core crate, only a one-line registration in the binary. We rejected runtime plugin loading (dynamic libraries, WASM modules or out-of-process drivers) because Rust has no stable ABI, and the WASM or IPC alternatives add a sandboxing and versioning surface that a personal tool doesn't need (see ADR-0001). A community plugin system (PRD §14) would need a new ADR to replace this one.
