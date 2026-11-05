# Running Gantry in Docker

Docker on a home server is the main way to run Gantry (PRD §9.3). The image is published to the GitHub Container Registry for `linux/amd64` and `linux/arm64`, so it runs on a mini PC, a NAS or a Raspberry Pi 4 or 5 (64-bit OS).

```
ghcr.io/adrianeyre/gantry:latest      the newest release
ghcr.io/adrianeyre/gantry:0.2.0       one release, pinned
```

The image holds one binary with the frontend embedded. It has no shell and no package manager (it is based on Google's distroless `cc-debian12`).

## What the image sets

| Setting | Value in the image | Why |
|---|---|---|
| `GANTRY_BIND` | `0.0.0.0:8080` | Listens on every interface inside the container. Choose what the host exposes with `-p`. |
| `GANTRY_DATA_DIR` | `/data` | The SQLite database. `/data` is a volume. |
| `GANTRY_LOG_FORMAT` | `json` | One JSON object per log line, for `docker logs` and log collectors. |
| User | `65532:65532` | The distroless `nonroot` user. The process never runs as root. |

You must set `GANTRY_SECRET_KEY` yourself. It encrypts Printer credentials (ADR-0005). Gantry won't start without it, and if it is lost the stored credentials can't be read. Keep it somewhere safe, apart from the data.

Every other setting in [`gantry.example.toml`](../gantry.example.toml) can be set with its `GANTRY_*` environment variable. Or put a `gantry.toml` in the data volume: the working directory is `/data`, so Gantry finds it there.

## With `docker run`

```bash
# Once: make a secret key and keep it.
openssl rand -base64 32 > gantry-secret-key

docker run -d --name gantry --restart unless-stopped \
  -p 8080:8080 \
  -v gantry-data:/data \
  -e GANTRY_SECRET_KEY="$(cat gantry-secret-key)" \
  ghcr.io/adrianeyre/gantry:latest
```

`-p 8080:8080` exposes Gantry on every interface of the host. To keep it on the host only, for a reverse proxy or a tunnel on the same machine, use `-p 127.0.0.1:8080:8080`.

## With Docker Compose

`compose.yaml`:

```yaml
services:
  gantry:
    image: ghcr.io/adrianeyre/gantry:latest
    container_name: gantry
    restart: unless-stopped
    ports:
      - "8080:8080"
    env_file: gantry.env
    volumes:
      - gantry-data:/data

volumes:
  gantry-data:
```

`gantry.env`, beside it (and out of Git):

```bash
GANTRY_SECRET_KEY=paste-the-output-of-openssl-rand-base64-32-here
```

Then `docker compose up -d`.

## The first account

There is no setup page. Create the first Admin inside the running container:

```bash
docker exec -it gantry gantry user create --admin --username admin
```

It asks for the password twice. To script it, pass the password on standard input instead (note `-i` without `-t`):

```bash
printf '%s\n' "$PASSWORD" | docker exec -i gantry gantry user create --admin --username admin --password-stdin
```

Then open `http://<host>:8080`, log in, and add Printers under **Printers**. Further accounts are added under **Users**, or the same way with `--role viewer|operator|admin`.

## Where the data lives

Everything Gantry keeps (Printers, accounts, Jobs, the Queue, settings) is in SQLite under `/data`, in the `gantry-data` volume. Removing or recreating the container keeps it. Removing the volume (`docker volume rm gantry-data`, or `docker compose down -v`) deletes it.

To back up, stop the container and copy the volume:

```bash
docker stop gantry
docker run --rm -v gantry-data:/data -v "$PWD:/backup" debian:12-slim tar -czf /backup/gantry-data.tar.gz -C /data .
docker start gantry
```

Keep the secret key with the backup, or the Printer credentials in it can't be decrypted.

### Bind mounts and file ownership

A named volume is created owned by uid 65532, so it just works. A bind mount (`-v /srv/gantry:/data`) keeps the host folder's owner, so make it writable by that uid first:

```bash
sudo mkdir -p /srv/gantry && sudo chown 65532:65532 /srv/gantry
```

## Health checks

`GET /healthz` answers `200` without logging in. The image has no `HEALTHCHECK`, because it has no shell or `curl` to run one. Point your reverse proxy, uptime monitor or orchestrator at `/healthz` instead.

## Upgrading

```bash
docker pull ghcr.io/adrianeyre/gantry:latest
docker rm -f gantry
# then the same `docker run` as before
```

With Compose: `docker compose pull && docker compose up -d`. The database is migrated forward on start. Pin a version tag rather than `latest` if you want upgrades to happen only when you choose. Back up first: a migrated database can't be opened by an older version.

## Building the image yourself

From the repository root:

```bash
docker buildx build --platform linux/amd64,linux/arm64 -t gantry .   # both architectures
docker build -t gantry .                                             # this machine's only
```

Rust is cross-compiled on the build machine's architecture, so building the other one doesn't need slow emulation. See the comments in the [`Dockerfile`](../Dockerfile).
