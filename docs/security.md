# Security

Gantry controls machines that heat to 300 °C, so it is locked down by default: a login is always required, and nothing is exposed but the dashboard.

## Logins and roles

- **A login is always required**, even on the LAN: Argon2-hashed passwords and secure session cookies. There is no first-run setup page: the first Admin is created on the command line (`gantry user create --admin`).
- **Three roles**, enforced by the server on every API route, not only hidden in the UI:

  | Role | Can |
  |---|---|
  | **Viewer** | See everything, change nothing |
  | **Operator** | Also control Printers, Mark Clear, upload Print Files, and manage Jobs and the Queue |
  | **Admin** | Also manage Printers, users, Alerts, webhooks and settings, and see the audit log |

- **Failed logins are slowed and then locked out.** After `login_slow_after` failures in a row, each attempt waits longer; after `login_max_failures` the account is locked for `login_lockout_minutes`, even against the right password, unless an Admin unlocks it under **Users**. One client address may only fail `login_client_attempts_per_minute` times a minute, whichever accounts it tries ([configuration](configuration.md)).
- **The audit log** (**Audit log**, Admins only) records every state-changing action, logins and lockouts included, with who, what, which Printer and the result. Admins filter it.

## Two-factor authentication and passkeys

- **TOTP:** anyone can turn on two-factor authentication (any authenticator app) under **Account**, with single-use recovery codes. An Admin can require it for everyone, and reset it for someone who has lost their device.
- **Passkeys** log in without a password. Browsers only allow them when Gantry is reached by a name, such as `gantry.local` or your tunnel's domain, not by an IP address.

## Secrets

- **`GANTRY_SECRET_KEY`** encrypts Printer credentials, TOTP secrets and every Alert channel's credentials, and Gantry refuses to start without it ([ADR-0005](architectural-decision-record/0005-printer-credentials-encrypted-with-secret-key.md)). Keep it with your backups: without it, those credentials must be entered again.
- **API tokens** act with their owner's role, are stored only as hashes, can be revoked at any time, and everything they change is audited with the token's name ([the REST API](rest-api.md)).
- **Webhooks** are signed, so the receiver can check they came from Gantry ([webhooks](webhooks.md)).
- Config export leaves credentials out unless an Admin asks for them.

## The network

- **Printer ports never leave the LAN.** Only the dashboard is exposed.
- **Gantry doesn't depend on a particular way of reaching it.** It serves HTTP behind any TLS-terminating proxy or tunnel, or HTTPS directly (`tls_cert` and `tls_key`). The recommended route is **Cloudflare Tunnel**, and **Tailscale** is the private alternative. Don't forward router ports. [Reaching Gantry remotely](remote-access.md) walks through each route with example configuration.
- **Forwarded headers are believed only from trusted proxies.** Behind a tunnel or proxy, list its address in `trusted_proxies`, and Gantry takes the real client's address (for login limits and the audit log) and HTTPS (for `Secure` cookies) from its `X-Forwarded-For` and `X-Forwarded-Proto`. From anywhere else those headers are ignored.
- **`/metrics`** is off by default. When it's on, it needs a bearer token, or it is served on an address of its own ([Prometheus metrics](metrics.md)).
- **Remote camera viewing** only streams while someone is watching, downscaled to fit home upload bandwidth.
- **The Docker image** runs as a non-root user on distroless, with no shell or package manager.

## Downloads

The release binaries and installers aren't code-signed yet (#77). Download them only from this repository's Releases, and check them against `SHA256SUMS` ([installing](installing.md#verifying-a-download)).

## Reporting a vulnerability

Please report security problems privately to the maintainer through GitHub (the repository's **Security** tab), not in a public issue.
