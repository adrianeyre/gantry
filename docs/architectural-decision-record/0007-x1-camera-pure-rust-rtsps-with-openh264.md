# X1 cameras are read with a Rust RTSPS client and decoded with a compiled-in OpenH264

Bambu's X1 series streams its camera as H.264 over RTSP on TLS (`rtsps://<ip>:322/streaming/live/1`), unlike the P1 and A1, which send plain JPEG frames. Gantry's camera relay serves MJPEG, and Gantry must ship as one binary with no runtime dependencies (PRD goal 3, §11), which is why PRD §10.3 and §13 flagged X1 as needing a decision.

Gantry reads the X1 camera with its own small RTSPS client (RTSP 1.0 over `tokio-rustls`, Basic and Digest login, RTP interleaved on the same connection, RFC 6184 depacketizing), decodes the H.264 with the `openh264` crate, whose C++ source is compiled into the binary, and re-encodes frames as JPEG for the existing MJPEG relay. Every frame is decoded, because P-frames depend on the frames before them and the X1's keyframe interval is unknown, but colour conversion and JPEG encoding happen for at most 5 frames a second, which is most of the saving. Decoding runs on its own thread while someone watches, and stops when they leave.

We rejected:

- **Remuxing to fragmented MP4 for the browser (Media Source Extensions).** No decoding at all, so the lowest CPU, but a second stream format, a second player in the UI and a second path through the relay, for one camera family.
- **ffmpeg bundled in the Docker image only.** The standalone binaries would lose X1 cameras, and it adds a process to supervise.
- **Skipping X1 cameras.** Simplest, but the X1 is a likely purchase and the camera is a headline feature (PRD §7.1).

Consequences: building Gantry needs a C/C++ compiler (MSVC, gcc or clang), which Rust toolchains on every release target already have; the runtime needs nothing. Watching an X1 camera costs a full H.264 decode, which is noticeable on a Raspberry Pi at 1080p; the frame-rate limit and decoding only while watched bound it. OpenH264 is BSD-licensed and built from source, so Cisco's binary-licence terms don't apply. Changing approach later means replacing only the X1 camera source.

Status: tested against an in-process mock RTSPS camera only. Real X1 behaviour (the exact SDP, login scheme and keyframe interval) is unverified, and as of 4 November 2026 the first Bambu model to be bought is still undecided (#25), so whether X1 or P1/A1 is validated first is open.
