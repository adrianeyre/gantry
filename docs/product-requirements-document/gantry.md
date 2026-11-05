# Product Requirements Document: Gantry

**Product name:** Gantry (a multi-printer fleet dashboard)
**Status:** Draft v0.4
**Date:** 3 November 2026
**Primary stack:** Rust (core, server, optional Tauri desktop shell), React + TypeScript frontend embedded in the binary

Capitalised domain terms (Printer, Driver, Job, Plate, …) are defined in [`GLOSSARY.md`](../../GLOSSARY.md). Architectural decisions are recorded in [`docs/architectural-decision-record/`](../architectural-decision-record/).

---

## 1. Overview

A cross-platform application, written in Rust, that lets one machine monitor and control a Fleet of 3D printers. It is designed from day one to support any make and model through Drivers. **Bambu Lab is the first make**, developed against a mock Bambu broker until a Bambu printer is owned, followed by Moonraker and OctoPrint. It runs as an always-on headless server that serves a website, so the whole Fleet can be viewed and controlled from anywhere after logging in. A Tauri desktop shell is a later option.

## 2. Goals

1. Show live status of many Printers on a single screen.
2. Allow control (pause, resume, Cancel, temperatures, lights, speed) from that one screen.
3. Ship as a single binary per OS, with no runtime dependencies such as Node.
4. Provide a secure, login-protected website for remote access.
5. Preview what is about to be printed, in 2D thumbnails and interactive 3D.
6. Support any printer brand through Drivers.
7. Be buildable, testable and demonstrable without physical printers, using the Simulated Driver.

## 3. Non-Goals

- Slicing models (STL/OBJ/STEP are inputs to a slicer, not handled by the app).
- Bambu cloud integration (the app uses local LAN access only).
- Multi-tenancy, a hosted service or billing. Gantry is a personal, single-Fleet tool ([ADR-0001](../architectural-decision-record/0001-personal-single-fleet-scope.md)).
- Runtime-loaded driver plugins. Drivers are compiled in ([ADR-0003](../architectural-decision-record/0003-drivers-statically-linked.md)).
- Support for printers with only USB serial and no network API, unless bridged through OctoPrint or Klipper.

## 4. Target Users

- One person or a small workshop running several printers: makers, small print farms and small businesses. A few trusted users at most.
- The primary user is a developer with a DevOps background (Docker, CI/CD, AWS), who is comfortable self-hosting and expects things like metrics export and containerised deployment.
- The primary user does not own any 3D printers yet (as of November 2026), so the app must be buildable, testable and demonstrable without hardware (see section 5.6).

---

## 5. Architecture

### 5.1 Why a backend is required

A pure website cannot talk to printers directly. Browsers cannot, for example, speak MQTT over TLS to a Bambu printer's local broker. A small backend on a machine in the same network talks to the Printers locally and exposes a web API and WebSocket to the UI.

### 5.2 Deployment shapes

| Shape | Description | When |
|---|---|---|
| **Headless server (axum)** | One binary serving the dashboard (HTTP) and live updates (WebSocket). Run on a Pi, mini PC, NAS or any machine and open from any browser. | First release |
| **Tauri desktop app** | Native window around the same UI. Can embed the core or act as a client to a remote server. | Phase 6 |

### 5.3 Components

- **Core crate:** the `Driver` trait, normalised status model, capabilities, state management, Queue, History.
- **Server crate:** axum HTTP and WebSocket layer, authentication, API, embedded frontend.
- **Frontend:** React + TypeScript built with Vite and embedded in the server binary ([ADR-0002](../architectural-decision-record/0002-react-frontend-embedded-in-binary.md)). Node is a build-time dependency only.
- **Driver crates:** one crate per protocol, linked in at compile time behind a Cargo feature ([ADR-0003](../architectural-decision-record/0003-drivers-statically-linked.md)).
- **Camera relay:** proxies each Printer's camera into a browser-friendly stream.

### 5.4 Storage and configuration

- **SQLite** holds all state: Printers, Tags, users, Jobs, Queue, History, Alert Rules and the audit log ([ADR-0004](../architectural-decision-record/0004-sqlite-is-the-source-of-truth.md)). It is accessed through `sqlx`, with migrations embedded in the binary and queries checked at runtime ([ADR-0006](../architectural-decision-record/0006-sqlx-for-sqlite.md)).
- The **config file** (TOML, overridable by environment variables) holds only server settings: bind address, data directory and secret key.
- **Printer credentials** are encrypted with `GANTRY_SECRET_KEY`, and Gantry refuses to start without it ([ADR-0005](../architectural-decision-record/0005-printer-credentials-encrypted-with-secret-key.md)).
- Config export and import provide declarative, Git-friendly setup. Exports leave credentials out unless the export explicitly asks for them.

### 5.5 Suggested Rust crates

- `tokio` for async, with one task per Printer.
- `axum` for HTTP and WebSocket. `rust-embed` (or similar) for the frontend assets.
- `sqlx` for SQLite (ADR-0006).
- `serde` / `serde_json` for status reports.
- `rumqttc` with `rustls` for Bambu MQTT. Bambu printers use self-signed certificates, so a custom certificate verifier that skips validation is needed (acceptable on a trusted LAN). `tokio-rustls` for the Bambu camera TLS connection.
- `reqwest` / `tokio-tungstenite` for the REST and WebSocket Drivers (Moonraker, OctoPrint).

### 5.6 Development without hardware

No physical printers are available at the start of the project, so development relies on simulation.

- **Simulated Driver:** ships in release builds and is selected like any other Driver. It imitates a Printer with controllable behaviour: speed, progress, temperatures, layer changes, Faults, Filament Slots, dropouts, and a looping test camera stream. This lets the UI, server, authentication, Queue, Alerts and History be developed, tested and demonstrated end to end. Each Simulated Printer has a **simulation panel** on its card, visible to Admins and backed by an HTTP endpoint that end-to-end tests also use: start a simulated print, change speed, raise a Fault, drop the Connection, change filament. A simulated print reaches Gantry as an External Job. In the code, "mock" is reserved for test doubles.
- **Mock Bambu broker:** a local MQTT-over-TLS broker that replays sample `report` payloads and accepts `request` commands, so the Bambu Driver's connection, parsing, commands and reconnection can be tested in CI before hardware is owned. It is a Rust test-support crate in the workspace (an embedded `rumqttd` broker with a self-signed certificate) that `cargo test` starts in-process, so it needs no Docker. Its `report` fixtures are written by hand from the documented community schema, and a README beside them records where each field comes from. The Driver stays unproven against real firmware until a Bambu printer is owned.
- **Moonraker simulation:** a simulated-Klipper container lets the Moonraker Driver be tested without hardware.
- **OctoPrint virtual printer:** OctoPrint has a built-in virtual printer that runs in Docker, so the OctoPrint Driver can be tested end to end in CI.
- **PrusaLink:** no simulator exists, so the PrusaLink Driver is tested against a mock HTTP server and stays unproven until a Prusa printer is available.
- **Not testable without a real machine:** real camera streams, file upload to real printers, and Bambu's local-control restrictions (LAN Mode, Developer Mode, firmware authorisation).

---

## 6. Status and Control Model

### 6.1 Printer, Driver and capabilities

- The core talks to a generic `Driver` trait. A Driver speaks one protocol and serves many Printers.
- Operations include status, pause, resume, Cancel, upload-and-start, and the camera source.
- **Capabilities are per Printer**, reported by the Driver when it connects (chamber heater, Filament Slots, file upload, camera, control, …). An Admin can override them when detection is not possible. The UI hides controls a Printer does not support.
- When firmware blocks control (as Bambu's restrictions can), the Printer stays **connected** and the Driver withdraws its control capabilities. Status stays live, control buttons hide, and the UI says why.

### 6.2 Connection and Printer State

These are two separate fields:

- **Connection:** connected, connecting or disconnected. A disconnected Printer has a reason, such as unreachable, authentication rejected or TLS failure. The UI shows the last known state, for example "Disconnected (last seen: printing at 43%)".
- **Printer State** (only meaningful while connected):
  - **Idle:** ready for a Job.
  - **Preparing:** a Job has started but isn't printing yet (heating, levelling).
  - **Printing**
  - **Paused**
  - **Awaiting Clear:** a Job has ended and the part may still be on the bed.
  - **Busy:** doing something that isn't a Job (homing, loading filament, calibrating).

There is no "Error" state. Faults (warning or error severity) are shown alongside any state.

### 6.3 Clearing the bed

- After any Job ends, the Printer enters **Awaiting Clear**. The Queue never assigns a Job to a Printer in this state.
- An Operator uses **Mark Clear** to confirm the bed is empty, which returns the Printer to Idle. At that point they can overrule a Succeeded Job Outcome to Failed (spaghetti, warping and so on).
- **Auto-Clear** (per Printer, for machines that eject parts themselves) skips Awaiting Clear after a Succeeded Job only. A Failed or Cancelled Job still needs Mark Clear.

### 6.4 Commands

- Commands to a disconnected Printer are **rejected immediately** and never held for later.
- Pause, resume and Cancel ask for confirmation.
- **Bulk actions** (for example "pause all") are independent per-Printer commands, each with its own result: done, rejected (for example disconnected) or skipped (for example not printing).

---

## 7. Functional Requirements

### 7.1 Monitoring

- One card per Printer showing Connection, Printer State, progress, layer, time remaining, Job, Print File and Plate thumbnail.
- Nozzle, bed and chamber temperatures, and fan speeds.
- Filament Slots: loaded filament, colour, type and remaining amount per slot.
- Live camera view per Printer, plus a grid view of all cameras.
- Fault display (clogs, filament runout, door open, …).

### 7.2 Control

- Pause, resume and Cancel, with confirmation.
- Speed profile selection where supported (for example silent, standard, sport, ludicrous on Bambu).
- Chamber and work lights, nozzle and bed temperature, fan control.
- Homing, jogging and filament load/unload.
- Bulk actions (see 6.4).

### 7.3 Print management and the Queue

- Upload `.3mf` / `.gcode` Print Files to the library, send a Plate to a Printer and start the Job.
- **A Job prints exactly one Plate.** Queueing a 3-plate Print File creates 3 Jobs.
- A queued Job targets either **a specific Printer** or **any Compatible Printer**. Compatible means the Printer's model matches the model the Print File was sliced for (read from `.3mf` metadata, or set by hand for plain `.gcode`) and its Filament Slots hold the filament types needed. A colour mismatch is a warning, not a block. The Queue never sends a Print File to a model it was not sliced for.
- **Ordering:** first in, first out, and Operators can reorder by hand. If the Job at the front cannot run on any free Printer, the next one that can goes ahead of it. Priority levels and scheduling optimisation are future work (§14).
- **Auto-start** (per Printer, on by default): when a Printer becomes Idle (after Mark Clear or Auto-Clear), the Queue starts the next compatible Job. With it off, the Queue suggests the Job and an Operator starts it.
- A file library with Plate thumbnails and estimated print times.

### 7.4 Alerts

- **Alert Rules** apply to the whole Fleet and are set by an Admin: which events, on which Printers or Tags, go to which channel.
- Events: Job ended (by Job Outcome), Job paused, Fault raised, Connection lost, low filament.
- Channels: **ntfy and Discord first**. Email, Telegram and desktop notifications come later.

### 7.5 Jobs and History

- **Job Outcomes:** Succeeded, Failed, Cancelled, and Lost (Gantry never saw the Job end). An Operator can overrule Succeeded to Failed at Mark Clear.
- Jobs started outside Gantry are recorded as Jobs whose Print File may be unknown.
- After a Gantry restart or reconnect, a running print is **re-attached** to its open Job when the file and start time match. Otherwise it is recorded as a new external Job. History is never duplicated or broken.
- Per-Printer History: success and failure rates, durations, filament used.
- Filament inventory and cost tracking.
- Printer utilisation and uptime charts.
- Optional timelapse capture from the camera.
- **Retention:** Job History is kept forever. Timelapses and uploaded Print Files have size-based cleanup settings.

### 7.6 Fleet and administration

- Add, edit and remove Printers in the UI.
- **Tags** for filtering (room, purpose, …). A Printer can have many Tags. The model a Printer reports is a built-in attribute, not a Tag.
- Roles: **Viewer** (sees everything, changes nothing), **Operator** (also controls Printers and manages Jobs and the Queue), **Admin** (also manages Printers, Tags, users, Alert Rules and settings).
- Auto-discovery of Printers on the LAN (for example SSDP for Bambu, mDNS for Moonraker and OctoPrint).
- Config export and import (see 5.4).

### 7.7 Extras

- Prometheus metrics export (see 9.2).
- Read-only wall display mode for a workshop monitor.
- REST API and webhooks so other tools can trigger prints.

---

## 8. File Support and 3D Preview

### 8.1 Supported files

- **`.3mf`** is the richest format. Bambu Studio, OrcaSlicer and PrusaSlicer export it. A sliced `.gcode.3mf` contains one or more Plates (each with gcode and a thumbnail), filament mapping and print settings, including the printer model it was sliced for.
- **Plain `.gcode`** is a Print File with one Plate. It loses multi-plate and filament mapping. Many slicers embed a PNG thumbnail in the header. The target model is set by hand if it cannot be read.
- **`.stl`, `.obj`, `.step`** cannot be printed directly and need slicing first. Gantry does not handle them.

### 8.2 Preview levels

1. **Plate thumbnails (easy).** A `.gcode.3mf` is a zip containing pre-rendered PNGs (for example `Metadata/plate_1.png`). Shown in the file library and on the live card.
2. **Interactive 3D model.** The `.3mf` contains model meshes in `3D/3dmodel.model` (XML with vertices and triangles). Parsed in Rust and rendered in the web UI with Three.js, with orbit, zoom and per-object colours mapped to Filament Slot colours.
3. **Toolpath preview (most advanced).** Parse the gcode and render actual layers in 3D, with a layer slider and colouring by feature (walls, infill, supports) or by filament. During a live Job, highlight the current layer. Large files (tens of MB) are parsed in Rust and streamed to the frontend in simplified form.

### 8.3 Practical notes

- Print Files uploaded through Gantry's library are the easiest case, since Gantry already has them.
- For Jobs started outside Gantry, the file may be fetched from the Printer's storage where the Driver supports it, but this is not guaranteed.

---

## 9. Remote Access and Security

### 9.1 Reaching Gantry remotely

Gantry does not depend on any particular remote-access method. It always enforces its own login, and serves plain HTTP behind a TLS-terminating proxy or tunnel (or HTTPS directly with built-in `rustls`).

- **Recommended: Cloudflare Tunnel.** Outbound-only connection, no router ports opened, HTTPS and a domain included. Cloudflare Access can add a second layer of login.
- **Private alternative: Tailscale or WireGuard.** Nothing is public, but every device needs the VPN client.
- Gantry does not build a cloud relay (ADR-0001). Direct port forwarding works but is not recommended.

### 9.2 Authentication and security requirements

- **Login is required from the MVP onwards**, even on the LAN: Argon2-hashed passwords and secure session cookies.
- **The first account is created from the command line** (`gantry user create --admin`, run with `docker exec` in a container). There is no first-run setup page, so a freshly started Gantry behind a tunnel cannot be claimed by a stranger. Until roles arrive in Phase 2, that single account is an Admin.
- Roles Viewer, Operator and Admin (see 7.6). Optional 2FA (TOTP) and passkeys later.
- **Audit log** of every state-changing action: Printer commands (including each part of a bulk action), Queue and Job changes, Mark Clear and outcome overrules, Admin changes to Printers, Tags, users, Alert Rules and settings, and logins (successful and failed). Page views and reads are not logged.
- Rate limiting and lockout on failed logins.
- Printer credentials are encrypted at rest (5.4).
- **`/metrics`** is off by default. When it is on, it requires a bearer token from config (or binds to a separate internal port) and is never exposed through the tunnel without auth.
- **Printer ports stay private.** Only the dashboard is exposed. Printer protocol and camera ports never leave the LAN.

### 9.3 Operational considerations

- **Camera bandwidth:** remote viewing streams video over home upload. Streams flow only while someone is watching, and are downscaled and frame-rate-limited for remote sessions.
- **Always-on host:** the server runs 24/7 on something like a Raspberry Pi, mini PC or NAS.
- **Deployment:** Docker on a home server is the main route.

---

## 10. Multi-Brand Support

### 10.1 Driver architecture

- Each protocol is a Driver crate implementing the `Driver` trait, compiled in behind a Cargo feature (ADR-0003). Adding a Driver needs no change to the core crate, only a one-line registration in the binary.
- Each Driver translates its protocol into the normalised status model (Connection, Printer State, progress, temperatures, layer, time remaining, Filament Slots, Faults) and reports per-Printer capabilities.

### 10.2 Target protocols

| Protocol | Transport | Covers | Order |
|---|---|---|---|
| **Simulated** | In-process | Development, demos, tests | Phase 0 |
| **Bambu Lab (LAN MQTT)** | MQTT over TLS | X1, P1, A1 series | Phase 1 (first make) |
| **Moonraker (Klipper)** | REST + WebSocket | Voron, Creality K1 series, Sovol, Qidi, Elegoo Neptune 4, DIY Klipper builds | Phase 5 |
| **OctoPrint** | REST + WebSocket | Almost any printer, including old Marlin machines via a Raspberry Pi | Phase 5 |
| **PrusaLink** | REST | Prusa MK4, MK3.9, Mini, XL | Phase 5 |
| **Duet / RepRapFirmware** | HTTP API | Duet-based machines | As demand dictates |
| **Elegoo SDCP** | WebSocket | Newer Elegoo machines such as the Centauri Carbon | As demand dictates |
| **Anycubic, stock Creality, others** | Varies | Least open. Some local, some cloud-only, some need an OctoPrint/Klipper bridge | As demand dictates |

### 10.3 Bambu Lab specifics

**Connection.** Per Printer, the user supplies the IP address, serial number and access code (shown on the printer's network screen).

- MQTT over TLS at `mqtts://<printer-ip>:8883`, username `bblp`, password is the access code.
- Subscribe to `device/<serial>/report` for status. Publish to `device/<serial>/request` for commands.

**Prerequisites and risks.**

- LAN Mode must be enabled. Developer Mode may be needed on newer firmware for full control.
- Bambu's authorisation requirements can restrict local control, while status reading generally still works. This is handled by withdrawing control capabilities (6.1), not by disconnecting.
- The Driver must be tested against the user's model and firmware version.

**Camera.**

- P1 / A1 series: TLS connection to port 6000 with an authentication packet, then a stream of JPEG frames. Achievable in pure Rust.
- X1 series: RTSP on port 322. Likely needs `ffmpeg` or GStreamer bindings.

**File transfer.** Print Files upload over FTPS (port 990). Jobs started from the cloud may not have a retrievable file.

### 10.4 Known difficulties

- **Camera:** every brand differs (Bambu's custom stream, MJPEG or WebRTC from Moonraker and OctoPrint, none on some). Each Driver provides a camera source that the server relays.
- **Thumbnails:** extraction needs per-slicer parsing, though many slicers embed a PNG in the gcode header.
- **Mixed fleets:** a Print File is sliced for one printer model, so the Queue only uses Compatible Printers (7.3).
- **Maintenance:** vendors change firmware and APIs, so each Driver needs ongoing upkeep.

---

## 11. Non-Functional Requirements

- **Portability:** single binary per OS, with no runtime dependencies.
- **Release targets:** a Docker image for linux/amd64 and linux/arm64 (the main artefact), plus standalone binaries for Linux (amd64, arm64), Windows (x64) and macOS (arm64). Code signing is deferred to the Tauri phase.
- **Performance:** low memory and CPU footprint, one async task per Printer, suitable for a Raspberry Pi.
- **Reliability:** automatic reconnection, clear Connection states, Job re-attachment after restarts, graceful handling of firmware-blocked control.
- **Security:** see section 9.2.
- **Observability:** Prometheus metrics, structured logs.
- **Deployability:** Docker image, a small config file for server settings, everything else in SQLite.
- **Build and CI:** a Cargo workspace for Gantry, and the React app in `web/` as a member of the root pnpm workspace (beside the Sandcastle harness), tested with Vitest and React Testing Library. GitHub Actions workflows are written from the first issue but have only a manual `workflow_dispatch` trigger until all development is complete, to save Actions minutes. Releases (the Docker image to GHCR, and release binaries) also run by hand, with a version input. The final issue turns on `pull_request` and `push` triggers, and semantic-release on each merge to `main`. CI runs only `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` and the frontend lint, test and build.
- **Verification:** each issue is verified by running the full local gate and pasting its real output on the issue. Playwright end-to-end tests (one per demoable flow, against the binary running Simulated Printers) always run locally, never in Actions.

---

## 12. Phased Roadmap

### Phase 0: Foundations (no hardware needed)
- Core crate: `Driver` trait, normalised status model (Connection, Printer State, Faults, Filament Slots), per-Printer capabilities.
- Simulated Driver with controllable behaviour, a simulation panel and a test camera stream.
- SQLite storage, config file, encrypted credentials.
- Headless axum server with WebSocket, embedded React UI, and login (one Admin account, created from the CLI).
- Status cards and Printer management in the UI.
- Jobs and Job Outcomes, with simulated prints recorded as External Jobs.
- Pause, resume and Cancel, Awaiting Clear, and Mark Clear with the Succeeded-to-Failed overrule.
- Camera relay against the simulated camera, streaming only while watched, with a camera grid.
- GitHub Actions workflow for the CI gate (manual trigger only, see §11).

### Phase 1: MVP on Bambu Lab
*Built and tested in CI against the mock Bambu broker. Validated against real firmware once a Bambu printer is owned.*
- Bambu Driver: MQTT over TLS, status, pause, resume and Cancel, Filament Slots and Faults, firmware-blocked control handled as withdrawn capabilities.
- Admin override of capabilities.
- Fine controls (speed profile, temperatures, lights, fans, homing, jogging, filament load/unload) on the Simulated Driver, then the Bambu Driver.
- Bambu camera (P1/A1 TLS stream first, X1 RTSP once its approach is decided).
- Job re-attachment after restarts and reconnects, and the Lost Job Outcome.

### Phase 2: Secure remote access
- Viewer, Operator and Admin roles, audit log, rate limiting and lockout.
- Bulk actions, with each part audited.
- Config export and import.
- HTTPS options and Cloudflare Tunnel (recommended) and Tailscale guidance.
- Multi-arch Docker image and release binaries (manual release workflow).
- Remote-friendly camera streaming.

### Phase 3: Files, preview and Alerts
- Print File library, upload (FTPS for Bambu) and start, Plates.
- `.3mf` Plate thumbnails, then the interactive 3D model viewer.
- Tags.
- Fault and Filament Slot detail, including a low-filament threshold.
- Alert Rules with ntfy and Discord.

### Phase 4: Queue, History and analytics
- Queue with Compatible Printers, first-in-first-out with manual reorder, Auto-start and Auto-Clear.
- History, utilisation charts, filament inventory and cost.
- Retention and size-based cleanup.
- Toolpath preview with live layer tracking.
- Prometheus `/metrics` and wall display mode.

### Phase 5: More brands
- Moonraker Driver, tested against a simulated-Klipper container. This proves "a second brand needs no core change".
- OctoPrint Driver, tested in CI against OctoPrint's virtual printer.
- PrusaLink Driver, tested against a mock HTTP server.
- Further Drivers (Duet, Elegoo SDCP) as demand dictates. They sit in the backlog, outside the phases.

### Phase 6: Polish
- LAN auto-discovery, TOTP and passkeys, timelapse, REST API and webhooks, more Alert channels (email, Telegram, desktop), Tauri desktop shell and installers, code signing.
- Enable CI on pull requests and pushes, and semantic-release on merge. This is the last piece of development work.

## 13. Risks and Open Questions

**Risks**
- No hardware is owned yet, and the first make (Bambu) is the least predictable. The Bambu Driver, camera and FTPS upload cannot be validated against real firmware until a Bambu printer is bought. Mitigated by the Simulated Driver, the mock Bambu broker, and keeping the core free of Bambu-specific assumptions.
- Bambu firmware changes may restrict or break local control.
- X1 RTSP camera support adds an external dependency (ffmpeg or GStreamer).
- Remote video bandwidth is limited by home upload speed.
- Security exposure if remote access is misconfigured. Mitigated by login being mandatory from the MVP.

**Open questions**
- Which Bambu model will be bought first? This decides which camera path (P1/A1 TLS or X1 RTSP) is validated first.

**Resolved in v0.4:** basic Jobs and Mark Clear's overrule move into Phase 0, with simulated prints arriving as External Jobs; the features the roadmap had not placed (capability override, fine controls, bulk actions, Tags, config export, retention, camera grid) now have phases; the first account is created from the CLI; `sqlx` for SQLite (ADR-0006); CI and releases stay manual until development is complete; the mock Bambu broker is an in-process Rust crate.

**Resolved in v0.3:** start with Phase 0 against the Simulated Driver; Bambu Lab is the first make, then Moonraker and OctoPrint; control included in the MVP; headless server first, Tauri later; React frontend; Cloudflare Tunnel recommended; ntfy and Discord first; personal tool (ADR-0001).

## 14. Possible Future Directions

These are outside the current scope. Each would need a new ADR to replace ADR-0001 or ADR-0003.

- Hosted relay service so other users can access their own fleets without self-hosting.
- Mobile-friendly PWA or native companion app.
- Print farm features: Queue priority and scheduling optimisation, operator assignment, cost quoting.
- Plugin system for community-contributed Drivers.

---

## 15. Success Criteria

- The full dashboard, login, control, Mark Clear and Alerts flow can be demonstrated using only Simulated Printers.
- Configured Bambu Printers appear with live status within seconds of startup and recover automatically after network drops (against the mock Bambu broker in CI, then against a real printer once owned).
- A user can pause, resume and Cancel any Printer from the dashboard.
- A second brand (Moonraker) is added by writing a Driver crate plus a one-line registration, without changes to the core crate.
- The dashboard is reachable from outside the home network behind a login, with no printer ports exposed.

---

## 16. Naming

**Chosen name: Gantry.** The gantry is the frame that moves the print head, so it fits a tool that oversees printers, and it is short and memorable.

**Other options considered**
- 3D printing terms: Brim, Raft, Layerline, Spoolside.
- Fleet and monitoring: Farmhand, PrintHive, Layerwatch, Overseer.
- Rust nods: Rustbed, Oxbed, Ferrite.

**Naming guidance**
- Avoid third-party brand names (Bambu, Prusa, Klipper, OctoPrint) in the product name, to avoid trademark problems and because the app supports multiple brands.
- Before committing, check availability of the domain, the GitHub org or repo name, and the crates.io name, since short common words are often taken.
