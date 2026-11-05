# Mock Bambu broker fixtures

These `report` payloads are **written by hand** (PRD §5.6). None of them were
captured from a printer: none is owned yet. Every serial number, access code
and identifier in them, and in the tests that use them, is made up.

They follow the community-documented schema of the `device/<serial>/report`
topic. The main sources:

- **OpenBambuAPI**, `mqtt.md`: <https://github.com/Doridian/OpenBambuAPI>. Topic
  names, the `print.push_status` shape, `pushall`, and the `pause`, `resume` and
  `stop` commands.
- **ha-bambulab** (`pybambu`): <https://github.com/greghesp/ha-bambulab>. How
  each field is read: `gcode_state` values, the units of `mc_remaining_time`, the
  fan speed scale, AMS trays, `vt_tray` and HMS decoding.
- **Bambu Lab wiki, HMS codes**: <https://wiki.bambulab.com/en/x1/troubleshooting/hmscode>.
  The one HMS code given a message here.

Treat the Bambu Driver as unproven against real firmware until a printer is
owned (PRD §13). When one is, capture real payloads and correct these files,
and this table, to match.

## The login

The broker refuses a wrong username or access code with a CONNACK return
code 5 ("not authorised"). Community clients (ha-bambulab, and the Bambu
integrations built on paho-mqtt) report a wrong access code that way. The
Driver also treats code 4 ("bad user name or password") as an authentication
failure, in case firmware differs. Unverified against a real printer.

## Fields

| Field | Meaning, as documented | Source | Confidence |
|---|---|---|---|
| `print.command` = `push_status` | A status report | OpenBambuAPI | Documented |
| `print.msg` | `0` for a full report, `1` for a partial update with only changed fields (P1 and A1 send these) | ha-bambulab | Documented |
| `gcode_state` | `IDLE`, `PREPARE`, `RUNNING`, `PAUSE`, `FINISH`, `FAILED` (and `SLICING`) | OpenBambuAPI, ha-bambulab | Documented |
| `mc_percent` | Progress, 0 to 100 | OpenBambuAPI | Documented |
| `mc_remaining_time` | Minutes remaining | ha-bambulab | Documented |
| `layer_num`, `total_layer_num` | Current and total layers | ha-bambulab | Documented |
| `gcode_file`, `subtask_name`, `subtask_id` | The file on the printer, the project's name, and the task's id | ha-bambulab | Documented |
| `gcode_start_time` | When the print started, Unix seconds (a string; `0` when idle). With the file name it re-attaches a print to its Job after a restart (#21) | ha-bambulab | Documented |
| `nozzle_temper`, `nozzle_target_temper`, `bed_temper`, `bed_target_temper`, `chamber_temper` | °C | OpenBambuAPI | Documented |
| `cooling_fan_speed`, `big_fan1_speed`, `big_fan2_speed`, `heatbreak_fan_speed` | Strings, 0 to 15 (15 is 100%) | ha-bambulab | Documented |
| `spd_lvl` | Speed profile: 1 silent, 2 standard, 3 sport, 4 ludicrous | OpenBambuAPI | Documented |
| `lights_report[].node`, `.mode` | `chamber_light` (all models with a light) and `work_light` (X1), `on`, `off` or `flashing` | ha-bambulab | Documented |
| `ams.tray_now` | The loaded slot: unit × 4 + slot, `254` the external spool, `255` none | ha-bambulab | Documented |
| `ams.ams[].id`, `ams.ams[].tray[].id` | AMS unit and slot, `"0"` up | ha-bambulab | Documented |
| `tray_type`, `tray_color`, `remain` | Material, `RRGGBBAA` colour, percent left (`-1` when unknown). A tray with only an `id` is empty | ha-bambulab | Documented |
| `vt_tray` | The external spool (id `254`). An empty `tray_type` means nothing loaded | ha-bambulab | Documented |
| `hms[].attr`, `hms[].code` | 32-bit numbers. The code is `attr` high and low 16 bits, then `code` high and low 16 bits, shown as `HMS_AAAA_BBBB_CCCC_DDDD`. The high 16 bits of `code` are the level: 1 fatal, 2 serious, 3 common, 4 info | ha-bambulab | Documented |
| `HMS_07UU_2S00_0002_0001` | Filament has run out in unit `UU`+1, slot `S`+1. The wiki documents `0700_2000` (unit 1, slot 1) and `0700_2100` (slot 2). Units after the first are assumed to follow in `UU` | Bambu wiki | Unit 1 documented, other units **assumed** |
| `print_error` = `50348044` (`0x0300400C`) | The print was cancelled (stopped) by the user | ha-bambulab (handling of a stopped print) | **Assumption to verify**: the community reports a stop as `FAILED` plus this code |
| `print_error` = `117473282` | Any other non-zero error: a real failure | Made up, deliberately not a known code | Test value |

## Fixtures

| File | What it is | Used to test |
|---|---|---|
| `idle.json` | Full report: idle, one AMS with three loaded slots and one empty, no external spool | Idle mapping; one AMS unit |
| `printing.json` | Full report: printing *benchy* at 43%, layer 12 of 40, 65 min left | Printing mapping, temperatures and fans |
| `printing_delta.json` | Partial update (`msg: 1`): progress, layer, time left and nozzle only | Merging partial updates into the last full report |
| `paused.json` | Full report: paused at 44% | Paused mapping |
| `finished.json` | Full report: `FINISH` at 100% | A Succeeded Job |
| `failed.json` | Full report: `FAILED` with a non-cancel `print_error` | A Failed Job |
| `cancelled.json` | Full report: `FAILED` with `print_error` `0x0300400C` | A Cancelled Job (see the assumption above) |
| `ams_none.json` | No AMS (`ams_exist_bits` `0`), a PETG external spool | Zero AMS units |
| `ams_multiple.json` | Two AMS units, eight slots, one empty | Several AMS units |
| `hms_faults.json` | Three HMS entries: the wiki's `HMS_0700_2000_0002_0001` (AMS 1 Slot 1 has run out of filament, level 2), plus two made-up codes at level 3 and level 1 | Known and unknown codes, both severities |
| `hms_cleared.json` | Partial update with an empty `hms` | Faults clearing |
| `command_rejected.json` | A command answer with `result: failed` and the reason firmware gives when it refuses an unauthorised local command | Firmware-blocked control (#19) |
| `x1c_idle.json` | `idle` as an X1C would send it: a chamber light and a work light (`lights_report`), slot 2 of the first unit loaded (`ams.tray_now` `1`) | Capabilities by model (#23): work light |
| `p1s_idle.json` | `idle` as a P1S: a chamber light, no work light and no chamber temperature | Capabilities by model (#23) |
| `a1_idle.json` | `idle` as an A1: no chamber fans, no chamber temperature, no lights, no AMS, a PLA external spool, speed level 1 (silent) | Capabilities by model (#23) |

The model fixtures differ only where the community documents a difference
between the families (the X1's work light and chamber sensor, the A1's open
frame without auxiliary or chamber fans). Which fields each model really
sends is unverified until a printer is owned (#27).

`command_rejected.json` is the least certain. Firmware with Bambu's
"Authorization Control" (from January 2025) has been reported to answer local
commands it won't accept with `result` other than `success`, a `reason` such
as `mqtt message verify failed`, and an `err_code`. The exact `err_code` here
is a placeholder. Check it against a real printer before relying on it.

## Cameras

The mock cameras (`src/camera.rs` for P1 and A1, `src/rtsp.rs` for X1) follow
the community-documented protocols. Like the reports, **neither has been
checked against a real camera** (#28). Sources:

- **OpenBambuAPI**, `video.md`: <https://github.com/Doridian/OpenBambuAPI>. The
  P1/A1 port, login packet and frame header, and the X1 RTSPS URL.
- **ha-bambulab** (`pybambu`'s `ChamberImageThread`): the same P1/A1 protocol
  as working client code, including the JPEG start and end markers it checks.
- **RFC 2326** (RTSP 1.0), **RFC 2617** (Digest) and **RFC 6184** (H.264 over
  RTP) for the X1 side.

| Item | As documented | Confidence |
|---|---|---|
| P1/A1 port | 6000, TLS with a self-signed certificate | Documented |
| P1/A1 login | 80 bytes, little-endian: `0x40`, `0x3000`, 0, 0, then `bblp` and the access code, each zero-padded to 32 bytes | Documented |
| P1/A1 frame | 16-byte header, first 4 bytes the JPEG's length (little-endian), then the JPEG (`FF D8` … `FF D9`) | Documented |
| P1/A1 wrong access code | The camera closes the connection without a frame | **Assumed**: the Driver retries, and only reports a refusal after 3 logins in a row end that way |
| X1 URL | `rtsps://<ip>:322/streaming/live/1`, user `bblp`, password the access code | Documented |
| X1 login scheme | Digest or Basic; the Driver answers whichever the camera asks for | **Unknown**: the mock uses Digest by default and can use Basic |
| X1 SDP | One H.264 track, `packetization-mode=1`, with `sprop-parameter-sets` | **Assumed**: the Driver also takes SPS and PPS sent in-band |
| X1 resolution and keyframe interval | Reported as 1080p; interval unknown | **Unknown**: the mock sends 320×240 at 10 fps with a keyframe every 10 frames |

## Commands and their answers

Gantry reads each command's answer, which the printer publishes on the report
topic with the command's `sequence_id` and a `result` (`success`, or a
failure with a `reason`). The mock answers `success` by default; tests can make
it refuse every command the way firmware with Authorization Control is
reported to (the `command_rejected` fixture), or not answer at all.

| Control | Request | Source | Confidence |
|---|---|---|---|
| Pause, resume, Cancel | `print.command` `pause`, `resume`, `stop` | OpenBambuAPI | Documented |
| Speed profile | `print.command` `print_speed`, `param` `"1"` to `"4"` | OpenBambuAPI | Documented |
| Nozzle and bed temperature | `print.command` `gcode_line`, `param` `M104 S<t>` / `M140 S<t>` | OpenBambuAPI | Documented |
| Fans | `gcode_line` `M106 P<1 part, 2 auxiliary, 3 chamber> S<0-255>` | OpenBambuAPI | Documented |
| Homing | `gcode_line` `G28` | OpenBambuAPI | Documented |
| Jogging | `gcode_line`: `M211 S`, `M211 X1 Y1 Z1`, `M1002 push_ref_mode`, `G91`, `G1 <axis><mm> F3000`, `M1002 pop_ref_mode`, `M211 R` (what Bambu Studio sends) | Bambu Studio, via OpenBambuAPI | Documented |
| Lights | `system.command` `ledctrl`, `led_node` `chamber_light` / `work_light`, `led_mode` `on` / `off` | OpenBambuAPI | Documented |
| Filament load and unload | `print.command` `ams_change_filament`, `target` unit × 4 + slot (`254` external, `255` unload) | OpenBambuAPI | Documented; temperatures sent are a fixed 220 °C, **to verify** |
