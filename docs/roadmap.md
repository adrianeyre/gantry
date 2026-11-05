# Roadmap

> **Status: development complete; real-printer validation to come.** Every phase is built and works end to end with Simulated Printers. Seven Drivers are built: Bambu (tested against mock Bambu servers), Moonraker and OctoPrint (against their simulators in Docker), and PrusaLink, Duet and Elegoo SDCP (against mock servers written from their documentation). **No real printer has tried any of them yet.** CI runs on every pull request, and each merge to `main` is released automatically.

| Phase | Scope | Needs hardware? |
|---|---|---|
| **0: Foundations** | Core crate, `Driver` trait, status model, Simulated Driver with a simulation panel, SQLite, server with login, React UI, Jobs, pause/resume/Cancel, Mark Clear, simulated camera | No |
| **1: Bambu MVP** | Bambu Driver (status, control, Filament Slots, Faults), fine controls, capability override, Bambu camera, Job re-attachment | Built against a mock Bambu broker. Validated once a Bambu printer is owned |
| **2: Secure remote access** | Roles, audit log, lockout, bulk actions, config export, HTTPS and tunnel guidance, multi-arch Docker image, remote camera | No |
| **3: Files, preview, Alerts** | Print File library and upload, Plates, thumbnails, 3D viewer, Tags, ntfy and Discord Alerts | Upload needs a printer |
| **4: Queue and History** | Compatible-Printer Queue, Auto-start, Auto-Clear, History, analytics, retention, toolpath preview, Prometheus, wall display | No |
| **5: More brands** | Moonraker, OctoPrint, PrusaLink, then others | No (simulators) |
| **6: Polish** | LAN discovery, TOTP and passkeys, timelapse, REST API and webhooks, more Alert channels, Tauri desktop app, then CI switched on | Built. Code signing for the installers needs a certificate (#77) |

The full plan and the reasoning behind it are in the [PRD](product-requirements-document/gantry.md). The work is tracked as [GitHub issues](https://github.com/adrianeyre/gantry/issues), one milestone per phase. Each issue is labelled `ready-for-agent` (Sandcastle can do all of it) or `human-in-the-loop` (it needs a decision, a real printer or a credential), and `needs-hardware` marks work that can only be proven on a real printer.

## What's left

What is left needs real hardware or a person:

- validating the Drivers on real printers, starting with Bambu ([the checklist](processes/validating-bambu-on-real-hardware.md));
- deciding the X1 camera approach on real hardware;
- code signing for the binaries and desktop installers (#77).

GitHub Actions runs the CI gate on every pull request and push to `main`, and each merge to `main` that passes is released by semantic-release from its conventional commits ([Releasing](processes/releasing.md)). Playwright still runs only locally.
