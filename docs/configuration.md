# Configuration

Gantry has very little configuration: only **server settings** live in a file. Printers, users, Alerts, the Queue and everything else live in SQLite and are managed in the dashboard ([ADR-0004](architectural-decision-record/0004-sqlite-is-the-source-of-truth.md)).

Server settings come from `gantry.toml` in the working directory (or the file named by `--config` / `GANTRY_CONFIG`), and **every setting can be overridden by an environment variable**. [`gantry.example.toml`](../gantry.example.toml) documents each one; copy it to `gantry.toml` to start.

## Settings

| Setting | Environment variable | Default | What it does |
|---|---|---|---|
| `bind` | `GANTRY_BIND` | `127.0.0.1:8080` | The address and port to listen on. `0.0.0.0:8080` for every interface (the Docker image's default) |
| `data_dir` | `GANTRY_DATA_DIR` | `data` | Where the SQLite database, Print Files and timelapses are kept (`/data` in Docker) |
| `secret_key` | `GANTRY_SECRET_KEY` | none: required | Encrypts Printer credentials, TOTP secrets and Alert channel credentials. At least 16 characters; prefer the environment variable, so the key stays out of files. `openssl rand -base64 32` |
| `log_format` | `GANTRY_LOG_FORMAT` | `text` | `text`, or `json` for containers (the Docker image's default) |
| `secure_cookies` | `GANTRY_SECURE_COOKIES` | `false` | Always mark session cookies `Secure`. They already are whenever the connection is HTTPS |
| `tls_cert`, `tls_key` | `GANTRY_TLS_CERT`, `GANTRY_TLS_KEY` | none | A PEM certificate chain and key, to serve HTTPS directly. Both or neither |
| `trusted_proxies` | `GANTRY_TRUSTED_PROXIES` (comma-separated) | none | Proxy addresses or CIDR ranges whose `X-Forwarded-For` and `X-Forwarded-Proto` are believed. For cloudflared on the same host: `["127.0.0.1", "::1"]` |
| `public_url` | `GANTRY_PUBLIC_URL` | none | The address people reach Gantry at, for link previews and absolute URLs |
| `login_slow_after` | `GANTRY_LOGIN_SLOW_AFTER` | `3` | Failed logins in a row before each further attempt waits 1 s, 2 s, 4 s… |
| `login_max_failures` | `GANTRY_LOGIN_MAX_FAILURES` | `5` | Failed logins before the account is locked |
| `login_lockout_minutes` | `GANTRY_LOGIN_LOCKOUT_MINUTES` | `15` | How long a lockout lasts, unless an Admin unlocks it |
| `login_client_attempts_per_minute` | `GANTRY_LOGIN_CLIENT_ATTEMPTS_PER_MINUTE` | `20` | Failed logins one client address may make per minute, whichever accounts it tries |
| `metrics_token` | `GANTRY_METRICS_TOKEN` | none | Turns on `/metrics`, with this bearer token (at least 16 characters) |
| `metrics_bind` | `GANTRY_METRICS_BIND` | none | Serves `/metrics` on an address of its own instead of beside the dashboard |

`RUST_LOG` sets the log level (`info` by default), for example `RUST_LOG=debug` or `RUST_LOG=gantry_driver_bambu=debug`.

The metrics settings are explained in [Prometheus metrics](metrics.md), and the TLS and proxy settings in [reaching Gantry remotely](remote-access.md).

## The command line

```
gantry [--config <file>] [serve]                 run the server (the default)
gantry user create --username <name> --admin     create an Admin (the first account must be one)
gantry user create --username <name> --role viewer|operator|admin
gantry user create ... --password-stdin          read the password from standard input instead of prompting
gantry bambu-probe                               check a real Bambu printer against what the Driver expects
gantry --version
```

In Docker, prefix them with `docker exec -it gantry`, for example `docker exec -it gantry gantry user create --admin --username admin`.

`gantry bambu-probe` reads the printer's serial number and access code from `GANTRY_PROBE_SERIAL` and `GANTRY_PROBE_ACCESS_CODE`, or asks for them, and changes nothing on the printer ([validating the Bambu Driver](processes/validating-bambu-on-real-hardware.md)).

## Fleet settings

Everything else is set in the dashboard and kept in the database: under **Settings** (appearance, automation, thresholds, the Print File library's size limit), **Printers**, **Users**, **Alerts** and **Webhooks**. Admins export all of it under **Settings** as a Git-friendly TOML file, credentials only if asked for, and import one with a preview of every change.

## Backups

Back up the data directory and the secret key together. The data directory holds the SQLite database (stop Gantry, or use `sqlite3 gantry.db ".backup backup.db"`, for a consistent copy) and the stored Print Files. Without the key, Printer and Alert channel credentials must be entered again. Docker backups are in [Running Gantry in Docker](running-in-docker.md).
