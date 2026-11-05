# Installing Gantry

Every release on [GitHub Releases](https://github.com/adrianeyre/gantry/releases) has the server for every platform, the desktop app's installers, a Docker image and a `SHA256SUMS` file. The [website](https://gantry.adrianeyre.co.uk) links straight to the latest of each. Releases are made automatically from `main` ([Releasing](processes/releasing.md)).

## What to download

### The server

Run it on an always-on machine on your printers' network. Each archive holds one `gantry` binary with the dashboard embedded.

| Platform | File |
|---|---|
| Linux x86-64 | `gantry-<version>-linux-amd64.tar.gz` |
| Linux ARM64 (64-bit Raspberry Pi OS, most ARM NAS) | `gantry-<version>-linux-arm64.tar.gz` |
| Windows x64 | `gantry-<version>-windows-x64.zip` |
| macOS, Apple silicon | `gantry-<version>-macos-arm64.tar.gz` |
| Docker, linux/amd64 and linux/arm64 | `ghcr.io/adrianeyre/gantry:<version>` (and `:latest`) |

The Linux binaries are linked against glibc 2.36 (Debian 12) and libstdc++, so they run on Debian 12, Ubuntu 22.10 or any newer glibc-based distribution. For anything else (Alpine, older systems), use the Docker image. Then see [getting started](getting-started.md).

### The desktop app

A native window on the dashboard, that connects to your server or runs Gantry itself ([the desktop app](desktop.md)).

| Platform | File |
|---|---|
| Windows x64 | `Gantry_<version>_x64-setup.exe` (installs for the current user, no administrator needed) or `Gantry_<version>_x64_en-US.msi` |
| macOS, Apple silicon | `Gantry_<version>_aarch64.dmg` |
| Linux x64 | `Gantry_<version>_amd64.AppImage` or `gantry_<version>_amd64.deb` |

On Linux both need WebKitGTK 4.1 (`libwebkit2gtk-4.1-0`), which current Ubuntu and Debian desktops have. Install the `.deb` with `sudo apt install ./gantry_<version>_amd64.deb`; make the AppImage executable with `chmod +x` and run it.

## Unsigned downloads

**The binaries and installers aren't code-signed yet** (#77), so each system warns before running them:

- **Windows:** SmartScreen says "Windows protected your PC". Choose **More info**, then **Run anyway**.
- **macOS:** Gatekeeper says the app "cannot be opened because the developer cannot be verified", or that it is damaged. For the desktop app, right-click it in Applications and choose **Open**, then **Open** again, or run `xattr -dr com.apple.quarantine /Applications/Gantry.app`. For the server binary, run `xattr -d com.apple.quarantine gantry`.
- **Linux:** no warning, but check the download as below.

Only download from this repository's Releases.

## Verifying a download

Download `SHA256SUMS` with the files, into the same folder:

```bash
sha256sum --check --ignore-missing SHA256SUMS      # Linux
shasum -a 256 --check --ignore-missing SHA256SUMS  # macOS
```

On Windows, compare by eye:

```powershell
Get-FileHash Gantry_1.2.0_x64-setup.exe -Algorithm SHA256
Select-String x64-setup SHA256SUMS
```

## Upgrading

- **Docker:** pull the new image and recreate the container; the `/data` volume keeps everything ([Running Gantry in Docker](running-in-docker.md#upgrading)).
- **A binary:** stop Gantry, replace the binary, start it again with the same data directory and secret key. The database is migrated on start.
- **The desktop app:** install the new version over the old one.

Back up the data directory and the secret key before upgrading.
