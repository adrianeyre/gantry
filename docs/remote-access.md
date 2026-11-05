# Reaching Gantry remotely

Gantry runs on your home or workshop network, beside your printers. This guide covers the ways to reach it from elsewhere, safely (PRD §9.1, §9.2).

Gantry doesn't depend on any one of them. Whichever you choose, it always asks for its own login, even on the LAN.

| Route | What it needs | Public? | Recommended for |
|---|---|---|---|
| [Cloudflare Tunnel](#cloudflare-tunnel-recommended) | A free Cloudflare account and a domain | Yes, at a hostname you choose | Most people |
| [Cloudflare Access](#adding-cloudflare-access) | Cloudflare Tunnel | Behind a second login | Anyone who wants another layer |
| [Tailscale](#tailscale-private) or [WireGuard](#wireguard-private) | A VPN client on every device | No | A private setup |
| [Gantry's own HTTPS](#gantrys-own-https) | A certificate | Only on your LAN, or with port forwarding | HTTPS on the LAN |
| [Port forwarding](#why-not-port-forwarding) | A router setting | Yes, at your home address | Not recommended |

## What is exposed, and what never is

- **Only the dashboard is exposed.** Every route below reaches Gantry's one HTTP port (8080 by default), and nothing else.
- **Printer ports never leave the LAN.** Gantry talks to each printer over your network: MQTT on 8883, cameras on 6000 and 322, and FTPS on 990 for a Bambu printer. Don't forward, tunnel or publish any of them. Your browser never talks to a printer: it talks to Gantry, and Gantry talks to the printer.
- **`/metrics` is never exposed without auth.** Gantry's Prometheus endpoint (planned, see the roadmap) is off by default. When it's on, it needs a bearer token, and you should still keep it off the public hostname: scrape it on the LAN, or block the path at the tunnel (see [below](#keeping-metrics-private)).
- **Camera streams only flow while someone is watching**, and remote viewers get a smaller, slower stream by default, to fit home upload bandwidth. Each viewer can change the quality.

## Two settings that matter behind a tunnel or proxy

A tunnel or reverse proxy terminates HTTPS and passes requests on to Gantry as plain HTTP, so every request seems to come from the proxy. The proxy says who the real client is in `X-Forwarded-For`, and that it used HTTPS in `X-Forwarded-Proto`. Gantry believes those headers **only from the addresses you list in `trusted_proxies`**, because any client can send them.

```toml
# gantry.toml
# The proxy or tunnel's address, as Gantry sees it. Here: cloudflared on the same host.
trusted_proxies = ["127.0.0.1", "::1"]
```

Or `GANTRY_TRUSTED_PROXIES=127.0.0.1,::1`. With it set:

- **Login limits and the audit log** name the real client, not the proxy, so one person guessing passwords doesn't lock out everyone behind the tunnel.
- **Session cookies are marked `Secure`** whenever the client's own connection is HTTPS.
- **Remote viewers** are recognised as remote, so they get the reduced camera stream.

Leave `trusted_proxies` empty (the default) when nothing sits in front of Gantry. If you can't tell what address the proxy connects from, the audit log shows the client address for each login.

### Link previews

When someone pastes a link to Gantry into Slack, Discord, WhatsApp, X or LinkedIn, those sites fetch the login page to build a preview: Gantry's title, a description and a 1200×630 image. Their crawlers need absolute URLs, so set the address people use:

```toml
# gantry.toml
public_url = "https://gantry.example.com"
```

Without it, Gantry works the address out from each request's `Host`, and from `X-Forwarded-Host` only when the request came from a trusted proxy. The preview image, icons and `robots.txt` are public; everything else still needs a login. Crawlers can't log in, so a preview never shows anything about your Fleet.

## Cloudflare Tunnel (recommended)

A tunnel is an outbound connection from your network to Cloudflare. No router ports are opened, Cloudflare provides HTTPS and the certificate, and you reach Gantry at a hostname on your domain. The WebSocket and camera streams work through it.

You need a Cloudflare account with a domain on it (free is fine), and `cloudflared`.

### Option A: cloudflared on the same host, configured by file

```bash
cloudflared tunnel login                      # opens a browser to authorise your account
cloudflared tunnel create gantry              # prints the tunnel's UUID and writes its credentials
cloudflared tunnel route dns gantry gantry.example.com
```

`~/.cloudflared/config.yml` (or `/etc/cloudflared/config.yml` when run as a service):

```yaml
tunnel: <TUNNEL-UUID>
credentials-file: /home/pi/.cloudflared/<TUNNEL-UUID>.json

ingress:
  # Keep the metrics endpoint off the public hostname.
  - hostname: gantry.example.com
    path: ^/metrics
    service: http_status:404
  - hostname: gantry.example.com
    service: http://localhost:8080
  - service: http_status:404
```

```bash
cloudflared tunnel run gantry                 # try it
sudo cloudflared service install              # then run it at boot
```

Gantry keeps listening on `127.0.0.1:8080`, its default, so nothing else on the network can reach it directly. In `gantry.toml`:

```toml
bind = "127.0.0.1:8080"
trusted_proxies = ["127.0.0.1", "::1"]
```

### Option B: Docker Compose, configured in the Cloudflare dashboard

In the Cloudflare dashboard, under **Zero Trust → Networks → Tunnels**, create a tunnel, copy its token, and add a **public hostname** `gantry.example.com` with the service `http://gantry:8080`. Then:

```yaml
# compose.yaml
services:
  gantry:
    image: ghcr.io/adrianeyre/gantry:latest
    restart: unless-stopped
    env_file: gantry.env              # GANTRY_SECRET_KEY=...
    environment:
      # cloudflared reaches Gantry over this Compose network.
      GANTRY_TRUSTED_PROXIES: "172.30.0.0/24"
    volumes:
      - gantry-data:/data
    networks: [tunnel]
    # No `ports:` at all: only cloudflared can reach Gantry.

  cloudflared:
    image: cloudflare/cloudflared:latest
    restart: unless-stopped
    command: tunnel --no-autoupdate run
    environment:
      TUNNEL_TOKEN: ${TUNNEL_TOKEN}
    networks: [tunnel]

networks:
  tunnel:
    ipam:
      config:
        - subnet: 172.30.0.0/24

volumes:
  gantry-data:
```

Fixing the network's subnet means `GANTRY_TRUSTED_PROXIES` can name it. Add a second public hostname rule for the path `^/metrics` with the service `http_status:404` to keep metrics private.

## Adding Cloudflare Access

Cloudflare Access puts a second login in front of the tunnel, so a stranger can't even reach Gantry's login page.

1. In the Cloudflare dashboard, go to **Zero Trust → Access → Applications → Add an application → Self-hosted**.
2. Set the application domain to `gantry.example.com`.
3. Add a policy: **Action** Allow, **Include** Emails, and list the addresses allowed in (or use an email domain, or a GitHub or Google login).
4. Save. Visiting `https://gantry.example.com` now asks for the Access login first (a one-time code by email, by default), then Gantry's own.

Gantry's login still applies: Access decides who may reach Gantry, and Gantry's roles decide what they may do.

## Tailscale (private)

Tailscale joins your devices into a private network. Nothing is public, but every device that wants to reach Gantry needs the Tailscale app and your account.

1. Install Tailscale on the Gantry host and run `sudo tailscale up`.
2. Install it on your phone or laptop and log in to the same account.

### Plain HTTP over the tailnet

Tailscale's traffic is already encrypted end to end. Let Gantry listen on the host's addresses:

```toml
bind = "0.0.0.0:8080"
```

and browse to `http://<host-name>:8080` (with MagicDNS) or `http://100.x.y.z:8080`. Gantry counts Tailscale addresses (`100.64.0.0/10`) as remote, so cameras are downscaled to suit a phone on mobile data.

### HTTPS with `tailscale serve`

`tailscale serve` gives Gantry an HTTPS address on your tailnet, with a certificate Tailscale manages:

```bash
sudo tailscale serve --bg 8080
# Available within your tailnet: https://<host-name>.<tailnet-name>.ts.net/
```

It proxies to `127.0.0.1:8080`, so keep Gantry on loopback and trust it:

```toml
bind = "127.0.0.1:8080"
trusted_proxies = ["127.0.0.1", "::1"]
```

Don't use `tailscale funnel`: that publishes Gantry to the whole internet.

## WireGuard (private)

If you'd rather run your own VPN, WireGuard on the Gantry host (or the router) works the same way as Tailscale: nothing is public, and each device needs a client.

On the Gantry host, `/etc/wireguard/wg0.conf`:

```ini
[Interface]
Address = 10.8.0.1/24
ListenPort = 51820
PrivateKey = <host private key>

[Peer]
# Your phone
PublicKey = <phone public key>
AllowedIPs = 10.8.0.2/32
```

On the phone:

```ini
[Interface]
Address = 10.8.0.2/32
PrivateKey = <phone private key>

[Peer]
PublicKey = <host public key>
Endpoint = <your home address or dynamic DNS name>:51820
# Only Gantry's VPN address goes through the tunnel.
AllowedIPs = 10.8.0.1/32
PersistentKeepalive = 25
```

Generate keys with `wg genkey | tee private.key | wg pubkey > public.key`, start it with `sudo systemctl enable --now wg-quick@wg0`, and forward UDP 51820 on the router (WireGuard's one port, which answers nobody without a key). Set `bind = "0.0.0.0:8080"` (or `10.8.0.1:8080`) and browse to `http://10.8.0.1:8080`. The VPN range is private, so Gantry treats WireGuard clients as local: choose a lower camera quality in the camera view if your upload is slow.

## Gantry's own HTTPS

Gantry can serve HTTPS itself with built-in `rustls`, without a proxy. This suits HTTPS on the LAN. Give it a PEM certificate chain and its private key:

```toml
bind = "0.0.0.0:8443"
tls_cert = "/etc/gantry/fullchain.pem"
tls_key = "/etc/gantry/privkey.pem"
```

Or `GANTRY_TLS_CERT` and `GANTRY_TLS_KEY`. Both or neither: with neither, Gantry serves plain HTTP. Gantry reads them at start, and a missing file or a key that doesn't match the certificate stops it with a message saying which.

Where the certificate comes from:

- **A real one for a name you own**, from Let's Encrypt with a DNS challenge (no ports opened), for example with certbot and your DNS provider's plugin: `certbot certonly --dns-cloudflare --dns-cloudflare-credentials ~/.secrets/cloudflare.ini -d gantry.home.example.com`. Point `gantry.home.example.com` at the host's LAN address. Restart Gantry after each renewal (a certbot `--deploy-hook` can do it).
- **Your own local CA**, with [mkcert](https://github.com/FiloSottile/mkcert): `mkcert -install && mkcert gantry.local 192.168.1.20` writes a certificate and key that your own devices trust.

Session cookies are marked `Secure` on every HTTPS connection. No `trusted_proxies` is needed when nothing sits in front of Gantry.

### Behind your own reverse proxy

If you already run a reverse proxy such as Caddy, let it do HTTPS and keep Gantry on plain HTTP:

```caddyfile
gantry.home.example.com {
	@metrics path /metrics*
	respond @metrics 404
	reverse_proxy 127.0.0.1:8080
}
```

Caddy sets `X-Forwarded-For` and `X-Forwarded-Proto`, so trust its address (`trusted_proxies = ["127.0.0.1", "::1"]` when it runs on the same host).

## Why not port forwarding

Forwarding a router port straight to Gantry works, but it isn't recommended:

- **Your home address becomes public**, and every scanner on the internet finds the port within hours. Gantry's login, lockout and rate limits hold, but the login page takes the whole internet's traffic.
- **You must manage HTTPS yourself.** Without it, passwords and session cookies cross the internet in the clear.
- **Nothing filters traffic first.** A tunnel, Cloudflare Access or a VPN stops most unwanted requests before they reach your network.
- **One mistake exposes the printers.** Forwarding a printer's port (8883, 990, 6000, 322) gives the internet direct access to a machine that heats to 300 °C. Never forward those, whatever else you do.

If you must forward a port, forward only Gantry's, serve [Gantry's own HTTPS](#gantrys-own-https) with a real certificate, and keep `/metrics` off.

## Keeping metrics private

When the Prometheus endpoint arrives it will be off by default and need a bearer token when on. Even so:

- Scrape it from the LAN, not through the tunnel.
- Block `/metrics` on the public hostname: the `http_status:404` rule in the [cloudflared config](#option-a-cloudflared-on-the-same-host-configured-by-file), the extra rule in the Cloudflare dashboard, or the `respond` rule in the [Caddyfile](#behind-your-own-reverse-proxy).
