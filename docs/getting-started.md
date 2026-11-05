# Getting started

Gantry runs on an always-on machine on the same network as your printers: a Raspberry Pi, a mini PC, a NAS or any computer that stays on. You then reach it from any browser, or from [the desktop app](desktop.md).

Every way of running it needs the same two things:

1. **A secret key** in `GANTRY_SECRET_KEY`. It encrypts Printer credentials, and Gantry refuses to start without it. Generate one with `openssl rand -base64 32` and **keep it**: credentials can't be read without it.
2. **The first Admin account**, created on the command line. There is no setup page.

## With Docker (recommended for a server)

```bash
openssl rand -base64 32 > gantry-secret-key     # keep it: Printer credentials can't be read without it
docker run -d --name gantry --restart unless-stopped -p 8080:8080 -v gantry-data:/data \
  -e GANTRY_SECRET_KEY="$(cat gantry-secret-key)" ghcr.io/adrianeyre/gantry:latest
docker exec -it gantry gantry user create --admin --username admin
```

The image is for linux/amd64 and linux/arm64 (a 64-bit Raspberry Pi OS), runs as a non-root user and keeps its data in the `/data` volume. Compose, upgrading and backups are in [Running Gantry in Docker](running-in-docker.md).

## With a release binary

Download the archive for your system from the [latest release](https://github.com/adrianeyre/gantry/releases/latest) ([installing](installing.md) lists them all), then:

```bash
tar -xzf gantry-<version>-linux-arm64.tar.gz       # or -linux-amd64, -macos-arm64; unzip on Windows
export GANTRY_SECRET_KEY="$(openssl rand -base64 32)"   # store it somewhere safe, and set it on every start
./gantry user create --admin --username admin
./gantry serve                                      # http://127.0.0.1:8080
```

By default Gantry listens only on `127.0.0.1:8080`. To reach it from other machines, set `GANTRY_BIND=0.0.0.0:8080` (or `bind` in `gantry.toml`, see [configuration](configuration.md)). Its data goes in `./data` unless `GANTRY_DATA_DIR` says otherwise.

To keep it running, run it as a service. A systemd unit, for example:

```ini
# /etc/systemd/system/gantry.service
[Unit]
Description=Gantry
After=network-online.target
Wants=network-online.target

[Service]
User=gantry
WorkingDirectory=/var/lib/gantry
Environment=GANTRY_BIND=0.0.0.0:8080
Environment=GANTRY_DATA_DIR=/var/lib/gantry
EnvironmentFile=/etc/gantry/secret.env          # GANTRY_SECRET_KEY=...
ExecStart=/usr/local/bin/gantry serve
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

## With the desktop app, on one computer

Install the desktop app ([installing](installing.md)) and choose **Run Gantry here**. It generates the secret key, keeps it in the system keychain, asks for the first Admin account, and opens the dashboard. It runs only while the app is open, so a separate server is better for a Fleet that prints overnight. See [the desktop app](desktop.md).

## From source

You need Rust 1.99.1 (pinned in `rust-toolchain.toml`), a C/C++ compiler (MSVC, gcc or clang: the X1 camera's H.264 decoder is compiled in, see [ADR-0007](architectural-decision-record/0007-x1-camera-pure-rust-rtsps-with-openh264.md)), and Node 26 with pnpm 12 to build the frontend. The binary embeds the frontend and the decoder, so nothing but the binary is needed at runtime.

```bash
pnpm install && pnpm --filter @gantry/web build     # the React app, into web/dist
export GANTRY_SECRET_KEY="$(openssl rand -base64 32)"   # required: encrypts Printer credentials
cargo run -- user create --admin --username admin       # the first account; there is no setup page
cargo run                                                # http://127.0.0.1:8080
```

On Windows, build with the MSVC toolchain (`RUSTUP_TOOLCHAIN=1.99.1-x86_64-pc-windows-msvc`) unless MinGW is installed. `cargo build --release --locked -p gantry` builds the release binary in `target/release/`.

## First steps

1. **Log in** at `http://<the machine>:8080` as the Admin you created.
2. **Add a Printer** under **Printers**, or pick one from the Printers Gantry found on the network. Choose **Simulated** to try everything without hardware. What each make needs is in [supported printers](supported-printers.md).
3. **Add the others who use Gantry** under **Users**, as Viewers, Operators or Admins, or with `gantry user create --role viewer|operator|admin --username <name>`.
4. **Upload a Print File** under **Library** and send a Plate to a Printer, or add it to the **Queue**.
5. **Set up Alerts** under **Alerts**: add an ntfy, Discord, email, Telegram or desktop channel, then the Alert Rules that use it.
6. **Reach it from anywhere** through Cloudflare Tunnel or Tailscale, never by forwarding router ports: [reaching Gantry remotely](remote-access.md).

[Using Gantry](using-gantry.md) goes through every page.
