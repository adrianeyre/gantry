# How Gantry works

```
                 ┌────────────── your network ──────────────┐
 Browser ──HTTPS──▶ tunnel / proxy ──▶ Gantry server ──LAN──▶ Printers
 (anywhere)       │ (e.g. Cloudflare)  ├ axum HTTP + WebSocket  (MQTT, REST,
                  │                    ├ embedded React UI       camera)
                  │                    ├ Drivers (one per protocol)
                  │                    └ SQLite                  │
                  └──────────────────────────────────────────────┘
```

- **Why there's a server:** browsers can't talk to printers directly (for example, MQTT over TLS to a Bambu printer), so a small backend on the same network does.
- **Rust, one async task per Printer**, light enough for a Raspberry Pi. Each Printer's task keeps its Driver connected, turns what the printer reports into Gantry's status model, and sends changes to every open dashboard over a WebSocket.
- **Drivers** are crates compiled into the binary, one per protocol, behind the `Driver` trait in `crates/core` ([ADR-0003](architectural-decision-record/0003-drivers-statically-linked.md)). `crates/core` has no protocol code.
- **Frontend:** React and TypeScript, built by Vite and compiled into the binary, so at runtime there's no Node and nothing else to install ([ADR-0002](architectural-decision-record/0002-react-frontend-embedded-in-binary.md)).
- **Storage:** one SQLite database (through sqlx, [ADR-0006](architectural-decision-record/0006-sqlx-for-sqlite.md)) holds Printers, users, Jobs, the Queue, History, Alert Rules and the audit log. A small TOML config file (or environment variables) holds only server settings ([ADR-0004](architectural-decision-record/0004-sqlite-is-the-source-of-truth.md), [configuration](configuration.md)).
- **Secrets:** Printer credentials, TOTP secrets and Alert channel credentials are encrypted with `GANTRY_SECRET_KEY`, and Gantry refuses to start without it ([ADR-0005](architectural-decision-record/0005-printer-credentials-encrypted-with-secret-key.md)).
- **Cameras:** Gantry relays each camera to the dashboard, only while someone is watching. The X1 series' RTSPS H.264 stream is decoded by a compiled-in OpenH264 ([ADR-0007](architectural-decision-record/0007-x1-camera-pure-rust-rtsps-with-openh264.md)).
- **The desktop app** is a Tauri shell that shows a server's own dashboard, or runs the server in-process ([ADR-0008](architectural-decision-record/0008-desktop-app-remote-or-embedded.md), [the desktop app](desktop.md)).
- **Release targets:** a Docker image for linux/amd64 and linux/arm64, binaries for Linux (amd64, arm64), Windows (x64) and macOS (arm64), and desktop installers for Windows, macOS and Linux, released automatically from `main` ([Releasing](processes/releasing.md)).

## Key concepts

[`GLOSSARY.md`](../GLOSSARY.md) defines the language used in the code, issues and docs. The essentials:

| Term | Meaning |
|---|---|
| **Fleet** | Every Printer one Gantry deployment manages |
| **Printer** | One physical machine configured in Gantry |
| **Driver** | The adapter for one protocol. One Driver serves many Printers |
| **Print File / Plate** | A sliced file in the library, and one printable layout inside it |
| **Job** | One attempt to print one Plate on one Printer. Ends with an Outcome: Succeeded, Failed, Cancelled or Lost |
| **Compatible Printer** | A Printer of the model a Plate was sliced for, loaded with the filament it needs |
| **Connection vs Printer State** | Whether Gantry can talk to the Printer, and what it's doing (Idle, Preparing, Printing, Paused, Awaiting Clear, Busy) |
| **Fault vs Alert** | A problem the Printer reports, and a notification Gantry sends |
| **Viewer / Operator / Admin** | Can only look / can also control and queue / can also manage Printers, users and settings |

## Repository layout

```
.
├── crates/
│   ├── core/                                the Driver trait, status model, Jobs and the Fleet (no protocol code)
│   ├── server/                              axum HTTP + WebSocket, login, SQLite, camera relay, embedded UI
│   ├── gantry/                              the binary: CLI, config, logging, one line per Driver
│   ├── drivers/
│   │   ├── simulated/                       the Simulated Driver
│   │   ├── bambu/                           the Bambu Lab Driver (LAN MQTT)
│   │   ├── moonraker/                       the Moonraker (Klipper) Driver
│   │   ├── octoprint/                       the OctoPrint Driver
│   │   ├── prusalink/                       the PrusaLink Driver, tested against a mock PrusaLink
│   │   ├── duet/                            the Duet (RepRapFirmware) Driver, tested against a mock Duet
│   │   ├── elegoo-sdcp/                     the Elegoo SDCP Driver, tested against a mock SDCP printer
│   │   └── support/                         what Drivers share: the HTTP (MJPEG) camera, a Plate's G-code
│   └── test-support/
│       ├── mock-bambu/                      the mock Bambu broker, FTPS server, P1/A1 and X1 cameras, and report fixtures
│       └── print-files/                     small generated Print Files (.gcode.3mf, .gcode) for tests and Playwright
├── web/                                     the React + TypeScript frontend (Vite, Vitest, Playwright)
│   └── scripts/screenshots.mjs              takes the screenshots in docs/images/
├── desktop/                                 the Tauri desktop app (its own Cargo workspace)
├── site/                                    the project website (Vite, Stageshow demos), published to GitHub Pages
├── docker/simulators/                       simulated Klipper and OctoPrint, for testing those Drivers locally
├── GLOSSARY.md                              domain language
├── docs/                                    this documentation, the PRD, ADRs and processes
│   └── images/                              screenshots, used by the README, these docs and the website
├── .github/workflows/ci.yml                 the CI gate, on every pull request and push to main
├── .github/workflows/release.yml            each green merge to main: version, tag, image, binaries, installers
├── .github/workflows/desktop.yml            the desktop app: checks and installers, on desktop changes and for releases
├── .github/workflows/pages.yml              the website, on changes to it and after each release
├── .releaserc.json                          semantic-release's configuration
├── Dockerfile                               the multi-arch image, and the Linux release binaries
├── .sandcastle/                             the Sandcastle agent harness
├── gantry.example.toml                      the server settings, documented
└── package.json                             the Sandcastle harness, and the pnpm workspace root
```
