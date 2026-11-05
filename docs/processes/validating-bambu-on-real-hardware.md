# Validating the Bambu Driver on a real printer

The Bambu Driver, its fine controls and both camera sources were built against
mocks (PRD §5.6): the mock broker's hand-written fixtures, and mock P1/A1 and
X1 cameras. This is the checklist for the first real printer. It covers
issues **#27** (status, control, Filament Slots, Faults, reconnection and
firmware-blocked control), **#28** (the camera) and **#43** (FTPS upload,
once the Print File library lands), and closes **#25**'s open question: which
model came first.

Work through it on the LAN with the printer in **LAN Mode**. Record what you
find in the [results table](#results) at the end of this file, and in the
fixtures README where a payload differs.

> **Never commit a serial number or access code.** The probe replaces both in
> what it saves, but read every file before committing it. Commits, issue
> comments and screenshots must not show them either.

## 0. Before you start

- Note the printer's **model** and **firmware version** (Settings → Firmware on
  the printer). The probe reads both too.
- Turn on **LAN Mode**. If the firmware has Bambu's Authorization Control, leave
  **Developer Mode off** for step 6, then turn it on.
- Build Gantry: `cargo build --release` (see the README for prerequisites).

## 1. Probe the printer

The probe connects as the Driver does, asks for the whole state and the
firmware versions, records every payload, and reads the camera. It sends
nothing that changes the printer.

```bash
export GANTRY_PROBE_SERIAL=…        # or leave unset to be asked
export GANTRY_PROBE_ACCESS_CODE=…   # never pass these as arguments
gantry bambu-probe --host 192.168.1.50 --seconds 60 --camera-seconds 15 --out probe-idle
```

Run it three times: **idle**, **printing** (start a print from the printer's
screen or Bambu Studio first) and **paused**. Each run prints the model,
firmware, the normalised status, and the camera result, and saves
`payload-NNNN.json` files plus `camera-first-frame.jpg`.

- [ ] The model is detected from the serial (if not, add the prefix to
      `crates/drivers/bambu/src/model.rs` and say so in the results).
- [ ] Firmware versions are reported (`ota`, `mc`, …).
- [ ] The first status report arrives within a few seconds (PRD §15).
- [ ] Every field in the printed status looks right against the printer's
      screen: Printer State, Job, progress, layer, time left, temperatures,
      fans, Filament Slots, Faults, speed profile, lights, loaded slot.

## 2. Compare the payloads with the fixtures

For each run, compare the saved payloads with the fixture they correspond to
in `crates/test-support/mock-bambu/fixtures/` (`idle.json`, `printing.json`,
`paused.json`, and the `x1c_idle`, `p1s_idle` or `a1_idle` model fixture).

- [ ] Field names, types (string or number) and units match the README table.
- [ ] `print_error` for a print stopped from the printer's screen (the README
      marks `0x0300400C` as an assumption).
- [ ] `lights_report`, `ams.tray_now`, `spd_lvl`, `gcode_start_time` are present.
- [ ] Wherever a real payload differs, update the fixture **from the redacted
      file**, update the README row (set its confidence to *Verified on <model>
      <firmware>*), and fix the Driver until `cargo test` passes.

## 3. Add the printer to Gantry and check live status

Add it under **Printers** with the Bambu Driver (address, serial, access code).

- [ ] The card shows live status within seconds of starting Gantry (PRD §15).
- [ ] Progress, layer and time left move during a print.
- [ ] Filament Slots match the AMS (or external spool), with colours and
      remaining amounts; an empty slot shows as empty.
- [ ] Faults: trigger one you can (open the door on an X1 with the door sensor
      on, or run out of filament on a test spool). It shows with the right
      severity and message, and clears when resolved.

## 4. Pause, resume and Cancel

On a throwaway print:

- [ ] Pause asks for confirmation, then the printer pauses and the card says
      Paused.
- [ ] Resume carries on.
- [ ] Cancel ends the print; the Job's Outcome is **Cancelled** and the printer
      is **Awaiting Clear**. Mark Clear returns it to Idle.
- [ ] A print stopped from the **printer's screen** is recorded as Cancelled too
      (this checks the `print_error` assumption).
- [ ] A print that finishes is **Succeeded**; one that fails is **Failed**.

## 5. Fine controls

Open **Controls** on the card. For each, check the printer does it and the card
reflects it:

- [ ] Speed profile (silent, standard, sport, ludicrous) during a print.
- [ ] Nozzle and bed target temperatures.
- [ ] Chamber light, and the work light on an X1.
- [ ] Each fan offered (part, auxiliary, chamber).
- [ ] Home, then jog X, Y and Z by 1 and 10 mm (watch the toolhead).
- [ ] Load and unload filament from an AMS slot and the external spool. The
      README marks the temperatures sent here as **to verify**.
- [ ] Capabilities the model lacks are not offered (no work light on P1/A1, no
      chamber fan on A1).

## 6. Firmware-blocked control

With Authorization Control on and Developer Mode **off**:

- [ ] Pause (or any control) is refused, the printer stays **connected** with
      live status, every control disappears, and the card says to turn on
      Developer Mode.
- [ ] Record the exact answer the printer gave (from a probe run while you
      press Pause in Bambu Studio, or Gantry's log at debug level) and update
      `command_rejected.json` and its README row: `result`, `reason`, `err_code`.
- [ ] Turn on LAN Only Mode and Developer Mode. When the printer reconnects,
      the controls come back and work.

## 7. Reconnection after a network drop

- [ ] Unplug the printer's network (or turn off its Wi-Fi) mid-print: the card
      says Disconnected with the last state seen, and commands are refused at
      once.
- [ ] Reconnect it: Gantry recovers on its own within ~30 s and the same Job
      carries on (no new Job in History).
- [ ] Restart Gantry mid-print: the Job re-attaches (still one Job), and a print
      that finished while Gantry was down gets its real Outcome, not Lost.

## 8. The camera (#28)

- [ ] **Locally:** the card's camera and the camera grid show the printer's
      camera.
- [ ] **Remotely**, through the tunnel or VPN you use: the same, at a
      sensible frame rate.
- [ ] **Stops when unwatched:** close every page showing it. Within a few
      seconds Gantry's log says `camera relay stopped: no viewers` (at debug
      level), and the printer's camera connection closes (`ss -tn` / `netstat`
      shows nothing to port 6000 or 322).
- [ ] **X1 only:** note the CPU Gantry uses while one camera is watched, and on
      what hardware (ADR-0007 expects a noticeable cost on a Raspberry Pi).
- [ ] Anything that differs from the mock (login refusal, framing, SDP, the
      RTSP login scheme, keyframe interval) is fixed, or recorded in the
      fixtures README's camera section.

## 9. Upload and start (#43)

Sending a Plate uploads the Print File over FTPS (implicit TLS on port 990,
user `bblp`, the access code as password) to the root of the SD card, then
starts it over MQTT: `project_file` naming `Metadata/plate_<N>.gcode` for a
`.gcode.3mf`, or `gcode_file` for a plain `.gcode`. It was built against the
mock FTPS server and broker only, from community documentation (ha-bambulab,
OpenBambuAPI), so it is **unverified** until this section is done.
`gantry bambu-probe` does not upload anything: use Gantry itself.

The printer needs an SD card with some free space, and must be **Idle** with
a clear bed.

1. Slice a small two-Plate project in Bambu Studio for this model (a 20 mm
   cube on each Plate, different colours on an AMS printer), slice all
   Plates, and export the sliced file as one `.gcode.3mf`.
2. Add it to the Gantry library, choose **Plate 2**, and **Send to Printer**.
   (Until the library UI lands, call the Driver from a scratch test or the
   API it uses.)

- [ ] The upload finishes and no error is shown. The file appears on the SD
      card under its own name (on the printer's screen, or in Bambu Studio's
      device file list).
- [ ] The printer starts **Plate 2**, not Plate 1, and the card shows the Job
      linked to the Print File and Plate.
- [ ] On an AMS printer, the filaments come from the slots you expect. Gantry
      sends `use_ams` with no `ams_mapping`, so the printer maps them in
      order; note what it does. A printer with only the external spool
      should print from it.
- [ ] A plain `.gcode` starts with `gcode_file` (a P1 or A1 is the likeliest
      to need this).
- [ ] Upload with a wrong access code (edit the printer in Gantry): the error
      says the printer refused the access code for FTPS, no Job is recorded,
      and nothing starts.
- [ ] Fill the card, or try a file larger than the free space: the error
      shows the printer's own reply (such as `552 …`) and no Job is recorded.
- [ ] If the transfer fails with `522 SSL connection failed: session reuse
      required` (or similar), the data connection didn't resume the control
      connection's TLS session. Note the firmware and the TLS version
      negotiated (Wireshark shows it), and look at
      `crates/drivers/bambu/src/ftps.rs`.
- [ ] Note any differences between X1, P1 and A1: the port, the replies, the
      `EPSV` or `PASV` answer, the storage path (`/sdcard/` here), and fields
      in the start request the firmware ignores or needs.
- [ ] Record the printer's answer to `project_file` (from Gantry's log at
      debug level, or a probe running alongside), and any fields that differ,
      in the fixtures README.

## 10. Close the loop

- [ ] Fill in the results table below and commit it, with any fixture and
      Driver fixes, in a pull request that closes #27, #28 and #43 (and
      updates #25 with the model).
- [ ] If the model is an X1, re-read ADR-0007's consequences with the CPU
      numbers in hand.

## Results

| Check | Model / firmware | Result | Notes |
|---|---|---|---|
| Probe and payloads | | | |
| Live status, Filament Slots, Faults | | | |
| Pause, resume, Cancel, Outcomes | | | |
| Fine controls | | | |
| Firmware-blocked control | | | |
| Reconnection and re-attachment | | | |
| Camera, local and remote | | | |
| Upload and start (FTPS) | | | |
