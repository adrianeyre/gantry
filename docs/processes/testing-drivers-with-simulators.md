# Testing Drivers against printer simulators

The Moonraker and OctoPrint Drivers are tested end to end against real printer software running in Docker, with no printer attached (PRD §5.6). These tests run on a developer's machine, never in GitHub Actions: `cargo test` alone skips them (they are `#[ignore]`d), and CI only runs the unit gate.

The PrusaLink, Duet and Elegoo SDCP Drivers have no simulator, so they are tested against mock servers written from their protocols' documentation instead ([below](#the-prusalink-driver-a-mock-http-server)). Those tests need no Docker and run with plain `cargo test`, in CI too.

| Simulator | What it runs | Address |
|---|---|---|
| `klipper` | [virtual-klipper-printer](https://github.com/mainsail-crew/virtual-klipper-printer): real Klipper on simulavr (a simulated AVR MCU), behind real Moonraker, with a test-pattern MJPEG camera | Moonraker on `127.0.0.1:7125`, camera on `http://127.0.0.1:8110/?action=stream` |
| `octoprint` | OctoPrint 1.11.8 with its built-in virtual printer, connected at start, with a test Admin and API key | `127.0.0.1:5080`, API key `GANTRYTESTKEY0123456789ABCDEFGHIJ` |

Both are defined in [`docker/simulators/compose.yaml`](../../docker/simulators/compose.yaml).

## Running them

```bash
docker compose -f docker/simulators/compose.yaml up -d --build

# The Moonraker Driver (#61, #62)
GANTRY_MOONRAKER_CONTAINER=gantry-klipper \
  cargo test -p gantry-driver-moonraker --test simulated_klipper -- --ignored --test-threads 1

# The OctoPrint Driver (#63, #64)
GANTRY_OCTOPRINT_CONTAINER=gantry-octoprint \
  cargo test -p gantry-driver-octoprint --test virtual_printer -- --ignored --test-threads 1

# Each Driver's camera through the server's camera relay
cargo test -p gantry-server --lib camera_http -- --include-ignored

docker compose -f docker/simulators/compose.yaml down
```

`--test-threads 1` matters: each simulator is one printer, and the tests take turns with it.

The `*_CONTAINER` variables are optional. With them, the tests also restart the container and check the Driver reports the lost Connection and reconnects by itself. The other variables, should the simulators run elsewhere, are `GANTRY_MOONRAKER` (`host:port`), `GANTRY_MOONRAKER_CAMERA` (a stream URL), `GANTRY_OCTOPRINT` (`host:port`) and `GANTRY_OCTOPRINT_API_KEY`.

## What is covered

- **Status:** Connection, Printer State (Idle, Preparing, Printing, Paused), temperatures, the part fan (Klipper), position once homed (Klipper), and a print started outside Gantry arriving as an External Job.
- **Connection reasons:** nothing answering is *unreachable*; a wrong OctoPrint API key is *authentication rejected*; a restarted container is a lost Connection that recovers.
- **Control:** pause, resume and Cancel, ending in Awaiting Clear with the right Job Outcome; temperature targets, homing, jogging, and the part fan (Klipper).
- **Upload and start:** a Plate sent from Gantry starts, is recognised as the Job Gantry recorded, and (OctoPrint) runs to a Succeeded Outcome from OctoPrint's own `PrintDone` event.
- **Camera:** each Driver's camera, from its settings and from the webcam Moonraker or OctoPrint names, gives JPEG frames, and they reach the browser through the camera relay.

## What is not

- **Real cameras are unverified.** The Klipper simulator's camera is mjpg-streamer playing still images, and the OctoPrint test points OctoPrint at a local test streamer. Crowsnest, ustreamer, WebRTC-only and HLS setups have not been tried. Gantry reads MJPEG streams and JPEG snapshots only.
- **Real printers are unverified**: real firmware's timing, real homing (simulavr's endstops never trigger, so the Klipper test's G-code uses `SET_KINEMATIC_POSITION`), filament runout sensors, chamber sensors, and Moonraker or OctoPrint behind authentication proxies.

## Notes on the Klipper simulator

simulavr is CPU-hungry, and if it can't keep up with real time, Klipper shuts down with **"Timer too close"**. Gantry's [`printer.cfg`](../../docker/simulators/klipper/printer.cfg) leaves out the image's software-PWM fans and pins for that reason. If it still happens (on a busy machine, say), the tests restart Klipper's firmware and wait, and Gantry shows the shutdown as a Fault with control withdrawn, which is what a real printer does too. Pausing and cancelling take tens of seconds on the simulator, because Klipper finishes the moves it has already queued and the simulated MCU is slow.

The configuration only takes effect on a fresh container, because the image copies it into its config folder on first start. After changing it, run `docker compose -f docker/simulators/compose.yaml up -d --force-recreate klipper`.

## The PrusaLink Driver: a mock HTTP server

No PrusaLink simulator exists (PRD §5.6), so the PrusaLink Driver is tested against a mock PrusaLink instead: an in-process HTTP server on `127.0.0.1`, written from Prusa's OpenAPI description of the REST API v1 (`spec/openapi.yaml` in [prusa3d/Prusa-Link-Web](https://github.com/prusa3d/Prusa-Link-Web)), in [`crates/drivers/prusalink/tests/mock`](../../crates/drivers/prusalink/tests/mock/mod.rs). It needs no Docker, so these tests run under plain `cargo test`, in CI too:

```bash
cargo test -p gantry-driver-prusalink
```

The mock answers `/api/version`, `/api/v1/status`, `/api/v1/job`, `/api/v1/storage`, the job commands (`PUT /api/v1/job/{id}/pause|resume`, `DELETE /api/v1/job/{id}`) and uploads (`PUT /api/v1/files/{storage}/{name}` with `Print-After-Upload`), behind HTTP Digest authentication (`qop=auth`, MD5) or, as older versions use, an `X-Api-Key` header. Each test moves the mock printer along (a print started at its screen, its progress, its end, a state such as `BUSY`, `ERROR` or `ATTENTION`) and checks what the Driver reports through the Fleet and what requests reached the mock. The mock can be stopped and started again on the same port, closing every connection.

Covered: the status mapping (Idle, Preparing, Printing, Paused, Busy, Awaiting Clear, temperatures, error and warning Faults), an External Job paused, resumed and Cancelled to a Cancelled Outcome, a Plate uploaded byte for byte and started, running to a Succeeded Outcome, a wrong password or API key as *authentication rejected*, nothing listening as *unreachable*, reconnection after the mock restarts, and that neither the password nor the API key reaches a log at any level.

**Real printers are unverified.** The mock is only as right as the OpenAPI description and our reading of it: real firmware's timing, the Digest variant a given firmware sends (the Driver also answers challenges without `qop`, which the mock doesn't send), storage names other than `usb`, `.bgcode` files, and the states a real print passes through have not been seen on a real Prusa.

## The Duet Driver: a mock HTTP server

There is no RepRapFirmware simulator to run either, so the Duet Driver is tested against a mock Duet: an in-process HTTP server on `127.0.0.1`, written from Duet3D's documentation of RepRapFirmware 3's standalone HTTP API and object model, in [`crates/drivers/duet/tests/mock`](../../crates/drivers/duet/tests/mock/mod.rs). Like the PrusaLink tests, these run under plain `cargo test`:

```bash
cargo test -p gantry-driver-duet
```

The mock answers `rr_connect` (checking the password and handing out an RRF 3.5 session key, which every other request must carry), `rr_model` (the whole object model, or one `key` of it), `rr_gcode`, `rr_upload` and `rr_disconnect`. It acts on `M25`, `M24`, `M0` (Cancel, only once paused) and `M32` the way the firmware does, sets `job.lastFileName`, `lastFileCancelled` and `lastFileAborted` when a print ends, and records every G-code line it is sent. It can forget its sessions (as the board does after its session timeout) and be stopped and started again on the same port.

Covered: the status mapping (Idle, Preparing, Printing, Paused, Busy, Awaiting Clear, temperatures, fans, layers, time left, a halted board as an error Fault), an External Job paused, resumed and Cancelled (`M25`, `M24`, then `M25` and `M0`) to a Cancelled Outcome, a Plate uploaded byte for byte to `0:/gcodes/` and started with `M32`, running to a Succeeded Outcome, the fine controls as G-code (`M104`, `M140`, `M106`, `G28`, relative `G1`), a wrong or missing password as *authentication rejected*, nothing listening as *unreachable*, an expired session reopened without losing the Connection, reconnection after the mock restarts, and that the password (which travels in `rr_connect`'s query) never reaches a log at any level.

**Real boards are unverified.** The mock is only as right as the documentation and our reading of it: which object-model fields `rr_model`'s `f` flag leaves out (the Driver reads the job separately, with `v`, to be safe), real firmware's timing (a real pause finishes its queued moves, which Cancel waits up to 7 seconds for), RepRapFirmware before 3.0, and Duets in SBC mode (DSF), whose API is different, have not been tried.

## The Elegoo SDCP Driver and its mock printer

There is no simulator for Elegoo's SDCP v3 printers (the Centauri Carbon and its kin), so the Elegoo SDCP Driver (#80) is tested against a mock SDCP printer instead: an in-process WebSocket and HTTP server on `127.0.0.1`, written from Elegoo's [SDCP v3 spec](https://github.com/cbd-tech/SDCP-Smart-Device-Control-Protocol-V3.0.0), in [`crates/drivers/elegoo-sdcp/tests/mock_sdcp/`](../../crates/drivers/elegoo-sdcp/tests/mock_sdcp/mod.rs). It needs no Docker, so its tests run with plain `cargo test`:

```bash
cargo test -p gantry-driver-elegoo-sdcp
```

The mock answers requests on `/websocket` (and ignores any not addressed to its MainboardID, so the Driver has to learn it), pushes status, answers `ping` with `pong`, takes chunked uploads at `/uploadFile/upload` (checking the offsets and the MD5), and serves an MJPEG stream at the `VideoUrl` it gives for Cmd 386. The tests drive it through the Fleet: status mapping, a print run to a Succeeded Job, one Cancelled, one Failed, pause, resume and Cancel as Cmds 129, 131 and 130, upload and start (the bytes, the MD5 and Cmd 128), fine controls as Cmd 403, nothing listening as *unreachable*, and reconnecting after the mock restarts.

**Real hardware is unverified.** The mock is only as right as the spec and the community implementations it was written from. In particular, the Centauri Carbon's extra print status codes, whether its `CurrentTicks` and `TotalTicks` are milliseconds as the spec says, the Cmd 403 payloads for fine controls (not in the spec), and whether it pushes its status without a request addressed to its MainboardID have not been checked against a real printer.
