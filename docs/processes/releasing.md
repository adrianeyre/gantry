# Releasing Gantry

Gantry is released by **semantic-release** whenever a merge to `main` passes CI ([`.github/workflows/release.yml`](../../.github/workflows/release.yml), configured in [`.releaserc.json`](../../.releaserc.json)). Nobody chooses a version number: it comes from the [conventional commits](https://www.conventionalcommits.org/) merged since the last release (PRD §11).

| Commits since the last release | Release |
|---|---|
| Any `feat!:`, or a `BREAKING CHANGE:` footer | major (`2.0.0`) |
| Any `feat:` | minor (`1.3.0`) |
| Any `fix:` or `perf:` | patch (`1.2.4`) |
| Only `docs:`, `test:`, `ci:`, `chore:`, `refactor:`, `style:` or `build:` | nothing |

So write each commit (or each squash-merge title) as a conventional commit: it becomes the release note and decides the version. The first release, with no `v*` tag on `main` yet, is `1.0.0`. To start lower, push a tag such as `v0.1.0` on `main` first.

## How it runs

1. A pull request runs **CI**: formatting, clippy, the Rust tests, and the frontend's lint, tests and build. Playwright never runs in Actions, so run it locally before merging (README, "The local gate").
2. The merge to `main` runs CI again. When it passes, **Release** starts (`workflow_run`) on the exact commit CI passed.
3. `version` runs semantic-release in dry-run mode, which works out the next version and publishes nothing. If nothing calls for a release, every later job is skipped.
4. `image`, `linux-binaries`, `native-binaries` and `desktop` build the release with that version compiled in (`GANTRY_VERSION`, so `gantry --version` and the dashboard's footer say it). `desktop` calls [`desktop.yml`](../../.github/workflows/desktop.yml), which runs the desktop app's checks and builds its installers with that version.
5. `release` runs semantic-release for real. It adds the release's notes to [`CHANGELOG.md`](../../CHANGELOG.md) and commits that to `main` as `chore(release): <version> [skip ci]`, tags that commit `v<version>`, and creates the GitHub Release with generated notes, the archives, the installers and `SHA256SUMS`. If `main` has moved on and the next version would now be different, it stops rather than mislabel the binaries; the newer commit's own run releases it.
6. `pages` calls [`pages.yml`](../../.github/workflows/pages.yml), which rebuilds the website so its download buttons link to the new release's files.

The only commit made back to `main` is that `CHANGELOG.md` update. It is pushed with the workflow's own token and marked `[skip ci]`, so it starts neither CI nor another release; the main ruleset allows it, since it blocks only force-pushes and deletion. There is no version-bump commit: the workspace version in `Cargo.toml` stays as it is, and is used only by builds outside a release. The GitHub Releases carry the same notes as the changelog, plus the downloads.

To retry a release whose build failed part-way, fix the cause and merge the fix: the next run releases everything since the last tag. Or run **Release** by hand (**Actions → Release → Run workflow** on `main`, or `gh workflow run release.yml --ref main`); with nothing new since the last tag, it does nothing. A failed run may already have pushed the image, and the next run overwrites that tag.

The desktop installers' workflow also runs on its own, on pull requests that change `desktop/` and by hand ([the desktop app](../desktop.md)). The installers are released unsigned until code signing is set up (#77).

## What it produces

| Job | Output |
|---|---|
| `image` | `ghcr.io/<owner>/gantry:<version>` for `linux/amd64` and `linux/arm64`, and `:latest` unless it is a pre-release. The owner is the repository owner, in lower case. |
| `linux-binaries` | `gantry-<version>-linux-amd64.tar.gz` and `gantry-<version>-linux-arm64.tar.gz`, from the Dockerfile's `binaries` stage. They are the binaries the image runs. |
| `native-binaries` | `gantry-<version>-windows-x64.zip` (built on Windows, MSVC) and `gantry-<version>-macos-arm64.tar.gz` (built on an Apple silicon runner). |
| `desktop` | The desktop app: `Gantry_<version>_x64-setup.exe` and `Gantry_<version>_x64_en-US.msi` (Windows), `Gantry_<version>_aarch64.dmg` (macOS, Apple silicon), `Gantry_<version>_amd64.AppImage` and `gantry_<version>_amd64.deb` (Linux x64). |
| `release` | The `CHANGELOG.md` commit, the tag `v<version>` and the GitHub Release, with every archive, every installer and a `SHA256SUMS` file covering them all. |
| `pages` | The website on GitHub Pages, linking to this release's downloads. |

Each binary has the frontend embedded and needs nothing else at runtime beyond what the OS ships. The Linux binaries are linked against glibc 2.36 (Debian 12) and libstdc++, so they run on Debian 12, Ubuntu 22.10 or any newer glibc-based distribution. For anything else, use the Docker image.

The binaries and installers are not code-signed yet (#77). On macOS, clear the quarantine flag after downloading: `xattr -d com.apple.quarantine gantry` (or `xattr -dr com.apple.quarantine /Applications/Gantry.app`). On Windows, SmartScreen may warn on first run. [Installing](../installing.md) has the details.

## Verifying a download

Download `SHA256SUMS` with the archives, into the same folder:

```bash
sha256sum --check --ignore-missing SHA256SUMS      # Linux
shasum -a 256 --check --ignore-missing SHA256SUMS  # macOS
```

On Windows, compare by eye:

```powershell
Get-FileHash gantry-1.2.0-windows-x64.zip -Algorithm SHA256
Select-String windows-x64 SHA256SUMS
```

## Building the release artefacts locally

The Linux binaries, for both architectures, without Rust or Node installed:

```bash
docker buildx build --platform linux/amd64,linux/arm64 --target binaries --output type=local,dest=dist .
# dist/linux_amd64/gantry and dist/linux_arm64/gantry
```

Windows and macOS build natively, as in the README ("Running Gantry"), with `cargo build --release --locked -p gantry`. On Windows, use the MSVC toolchain (Visual Studio Build Tools): if `rustup` picks the GNU one, set `RUSTUP_TOOLCHAIN=1.99.1-x86_64-pc-windows-msvc` first. There is no cross-build to macOS: it needs Apple's SDK, so it is built only on the macOS runner.
