# The desktop app

Gantry's desktop app shows the same dashboard as a browser, in a native window, with desktop Alerts as native notifications. It has two modes ([ADR-0008](architectural-decision-record/0008-desktop-app-remote-or-embedded.md)):

- **Connect to a Gantry server**: the app is a client of the Gantry server that already runs your Fleet (on a Raspberry Pi, a NAS, in Docker). You log in on the server's own login page, as in a browser.
- **Run Gantry on this computer** (embedded mode): the app runs Gantry itself, with no separate server, for a single machine. It listens only on `127.0.0.1`, so nothing else on the network can reach it, and it runs only while the app is open.

The app is in `desktop/`: a Tauri 2 shell (`desktop/src-tauri`) and the small page it opens with (`desktop/ui`), where you choose a mode. The dashboard itself is never bundled into the app: the window always shows the one the server embeds, so it can't drift from the server it talks to.

## Installing

Every [release](https://github.com/adrianeyre/gantry/releases/latest) has the installers attached, with the release's version ([installing](installing.md)). They are built by the **Desktop** workflow (`.github/workflows/desktop.yml`), which Release calls for each release; it also runs on every pull request that changes `desktop/`, and by hand from the Actions tab, attaching the installers to the run as artifacts:

| Platform | Files |
|---|---|
| Windows x64 | `Gantry_<version>_x64-setup.exe` (NSIS, installs for the current user, no administrator needed) and `Gantry_<version>_x64_en-US.msi` |
| macOS (Apple silicon) | `Gantry_<version>_aarch64.dmg` |
| Linux x64 | `gantry_<version>_amd64.deb` and `Gantry_<version>_amd64.AppImage` |

**The installers are not code-signed yet** (PRD §11), so each system warns before running them:

- **Windows**: SmartScreen says "Windows protected your PC". Choose **More info**, then **Run anyway**.
- **macOS**: Gatekeeper says the app "cannot be opened because the developer cannot be verified" (or that it is damaged, for a downloaded file). Right-click the app in Applications and choose **Open**, then **Open** again; or run `xattr -dr com.apple.quarantine /Applications/Gantry.app`.
- **Linux**: the `.deb` installs with `sudo apt install ./gantry_<version>_amd64.deb`. The AppImage needs `chmod +x` first. Both need WebKitGTK 4.1 (`libwebkit2gtk-4.1-0`), which current Ubuntu and Debian desktops have.

Only download installers from this repository's releases (or its workflow runs), and check them against the release's `SHA256SUMS` ([verifying a download](installing.md#verifying-a-download)).

## Connecting to a server

On first start, type the server's address, such as `http://192.168.1.20:8080` or `https://gantry.example.com` (a bare `host:port` means `http://`). The app checks it really is Gantry before going there: `/healthz` must answer `ok` and `/api/session` must ask for a login. Then the window shows the server's login page, and the session cookie stays in the app like a browser's.

The app remembers the server and goes straight back to it next time. To pick another, use **Server → Change Server…** (Ctrl+Shift+S, or Cmd+Shift+S on macOS). **Server → Reload** reloads the dashboard.

For an HTTPS server, the certificate must be one the operating system trusts, as for any browser on it. Links to other sites open in your normal browser, never inside the app.

## Running Gantry on this computer

Choose **Run Gantry here**. The first time, the app:

1. generates a secret key and keeps it in the system keychain (Credential Manager on Windows, the Keychain on macOS, GNOME Keyring or KWallet on Linux), never beside the database. It encrypts Printer credentials, as `GANTRY_SECRET_KEY` does for a server ([ADR-0005](architectural-decision-record/0005-printer-credentials-encrypted-with-secret-key.md)). If the system has no keychain, set the `GANTRY_SECRET_KEY` environment variable (at least 16 characters, such as `openssl rand -base64 32`) before starting the app; without either, embedded mode refuses to start;
2. asks for the first Admin account, the equivalent of `gantry user create --admin`. It asks only while there is no account at all;
3. opens the dashboard's login page.

Every Driver that `gantry` has by default is there (Simulated, Bambu Lab, Moonraker, OctoPrint), so add a Simulated Printer to try it without hardware.

Its data (the SQLite database and Print Files) is in the app's data folder, in `server/`:

| Platform | Folder |
|---|---|
| Windows | `%APPDATA%\io.github.adrianeyre.gantry\server` |
| macOS | `~/Library/Application Support/io.github.adrianeyre.gantry/server` |
| Linux | `~/.local/share/io.github.adrianeyre.gantry/server` |

`server/gantry.toml` there takes the same server settings as `gantry.toml` (login limits, for example), except the bind address, data folder and secret key, which the app sets itself. The port is chosen on first start and kept while it stays free.

To move to a separate server later, export the configuration from the dashboard and import it on the server. Back up the data folder and the secret key together: without the key, Printer credentials must be re-entered. On Windows the key is in Credential Manager under `io.github.adrianeyre.gantry`.

## Desktop Alerts

With a **desktop** Alert channel and an Alert Rule that sends to it, each matching Alert shows as a native notification on every desktop app connected to the server, while the app is open. The dashboard in the app opens its WebSocket as a desktop client (`/api/ws?client=desktop`), and the server sends desktop Alerts only to those connections, never to a browser.

Only the server you chose (or the embedded one) may show notifications: the app grants Tauri's notification plugin to that origin alone, when it connects, and gives no other site any access to the app. If notifications don't appear, check that the system allows them for Gantry (on Windows: Settings → System → Notifications).

## Building it yourself

You need what Tauri needs on your platform ([prerequisites](https://v2.tauri.app/start/prerequisites/)): on Windows, the MSVC build tools and WebView2 (part of Windows 11); on Linux, the packages listed in `desktop.yml`.

```sh
pnpm install
cd desktop
pnpm tauri dev                  # run it
pnpm tauri build                # installers for this platform, in src-tauri/target/release/bundle/
pnpm tauri build --bundles nsis # only one kind
```

The desktop app is its own Cargo workspace, outside the server's, so the server builds and tests without a webview toolkit. Its checks run in `desktop/src-tauri`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` (which start real embedded and in-process Gantry servers).
