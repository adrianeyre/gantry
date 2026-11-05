# Supported printers

Each printer protocol is handled by a **Driver**: a crate compiled into the binary, so there are no plugins to install ([ADR-0003](architectural-decision-record/0003-drivers-statically-linked.md)). One Driver serves any number of Printers.

| Driver | Covers | Status |
|---|---|---|
| **Simulated** | Imitates a Printer in software, for trying Gantry, demos and tests | Ships in every build |
| **Bambu Lab (LAN MQTT)** | X1, P1 and A1 series | Built, tested against mock Bambu servers |
| **Moonraker (Klipper)** | Voron, Creality K1, Sovol, Qidi, Elegoo Neptune 4, DIY Klipper | Built, tested against simulated Klipper |
| **OctoPrint** | Almost anything, including old Marlin machines via a Raspberry Pi | Built, tested against OctoPrint's virtual printer |
| **PrusaLink** | Prusa MK4, MK3.9, Mini, XL | Built, tested against a mock PrusaLink |
| **Duet (RepRapFirmware)** | Duet-based machines (RepRapFirmware 3, standalone) | Built, tested against a mock Duet |
| **Elegoo (SDCP)** | Newer Elegoo machines, such as the Centauri Carbon | Built, tested against a mock SDCP printer |
| Anycubic, stock Creality, others | | As demand dictates |

> **No real printer has tried any of the Drivers yet.** What a simulator or mock can't prove (real firmware behaviour, real camera streams, FTPS upload) is called out as unverified in each pull request. [Validating the Bambu Driver on a real printer](processes/validating-bambu-on-real-hardware.md) is the checklist for the first one.

## Adding a Printer

Admins add Printers under **Printers → Add a Printer**, or from the list of Printers Gantry found on the network (Bambu, Moonraker and OctoPrint, by SSDP and mDNS). Each Driver asks for what it needs, below. Credentials are encrypted with the secret key before they are stored ([ADR-0005](architectural-decision-record/0005-printer-credentials-encrypted-with-secret-key.md)).

A Print File is only sent to the model it was sliced for, so a Printer's model must be the name your slicer uses (for example `Bambu Lab X1 Carbon` or `MK4`). Bambu Printers report their model; for the others, type it.

### Bambu Lab

- **LAN Mode** must be on, and newer firmware may also need **Developer Mode** for full control.
- You need each Printer's IP address, serial number and access code (on the printer's screen).
- If the firmware blocks control, Gantry keeps showing live status and hides the controls it can't use.
- Cameras: the P1 and A1 series' JPEG camera, and the X1 series' RTSPS camera, decoded by a compiled-in OpenH264 ([ADR-0007](architectural-decision-record/0007-x1-camera-pure-rust-rtsps-with-openh264.md)).
- Upload-and-start uses the printer's FTPS server.
- `gantry bambu-probe` checks a real Bambu printer against what the Driver expects, changing nothing on it.

### Moonraker and OctoPrint

Give the address (and port, if not the usual one) and, for OctoPrint, an API key (Moonraker's only if it asks for one). Neither can say which printer model it drives, so enter the model as your slicer names it. The camera is the stream you name, or else the webcam Moonraker or OctoPrint is set up with (MJPEG or snapshots).

### PrusaLink, Duet and Elegoo SDCP

Written from their protocols' documentation and tested against mock servers. Give PrusaLink the address and its password (user `maker` by default) or, on older firmware, its API key; Duet the address and its password, if it has one; Elegoo SDCP just the address. Enter the model as your slicer names it. PrusaLink and Duet have no camera Gantry can read; an Elegoo printer's camera is used when it has one.

### Simulated

Add a **Simulated** Printer to try everything without hardware. Its settings choose the model it pretends to be, how many Filament Slots it has, whether it can be controlled, and whether it has a camera (a test pattern). Admins drive it from the **Simulation** panel on its card: start and fail prints, change its speed, raise and clear Faults, drop and restore the connection, and load filament.

## Testing the Drivers

[Testing Drivers against printer simulators](processes/testing-drivers-with-simulators.md) covers simulated Klipper and OctoPrint's virtual printer in Docker, the mock PrusaLink, Duet and SDCP servers, and what none of them can prove.
