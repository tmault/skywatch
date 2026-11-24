# Skywatch

A 3D air-traffic-control visualization of AI agents, sub-agents, and monitoring jobs — built with a Rust (`axum`) simulation server and a Three.js WebGL client.

## Features

- **Device Airports** — Each connected device gets its own island airport with runway, control tower, terminal, and gates. Threads untouched for >24 hours park at their device's gates; touching a parked thread wakes it back into the air.
- **Airframe Classes** — Local models fly as single-prop private planes, frontier models as twin-engine jets, monitoring/scheduled jobs as MQ-9 Reaper drones, and sub-agents as delta-wing fighter jets flying wingman formation beside their parent aircraft.
- **Live T3 Integration** — Mirrors recent T3 threads, sub-agents, and linked environments; click any aircraft to open its radio panel, inspect status, approve holds, or send messages directly back to the thread.
- **Demo Mode** — Set `SKYWATCH_T3=0` to run the standalone scripted fleet without T3.

## Local Development

```bash
cd server
cargo run
```

Open [http://localhost:8422](http://localhost:8422).

### Environment Variables

- `SKYWATCH_T3=0` — Disable T3 adapter (falls back to demo mode).
- `SKYWATCH_DEMO=0` — Disable demo seeding when T3 adapter is off.
- `SKYWATCH_PORT=8423` — Listen on a different port (default `8422`).

## Container deployment

The repository ships a multi-stage `Dockerfile` (Rust build stage with digest-pinned base images and
`cargo build --locked`, then a slim Debian runtime holding the server binary, `web/` assets, `python3`
for the T3 bridge and `sqlite3`), a `docker-compose.yml` and a live-T3 overlay,
`docker-compose.live.yml`. Nothing credential- or database-related is baked into the image; the
`.dockerignore` keeps `.git`, `.nightshift/`, `target/`, SQLite files, tokens and `.env*` out of the
build context. The image runs as a non-root user (UID/GID 1000 by default), with a read-only root
filesystem, no capabilities and a `/health` healthcheck.

### Run (demo fleet, no host access)

```bash
docker compose up -d --build           # http://127.0.0.1:8422
docker compose ps                      # shows health
```

Variables: `SKYWATCH_PORT` (published port, default 8422), `SKYWATCH_BIND` (default `127.0.0.1`; the
server has no authentication, so widen only on a trusted network or behind a trusted proxy),
`SKYWATCH_TAG` (image tag, default `local`), `SKYWATCH_UID`/`SKYWATCH_GID` (build-time runtime user).

### Live T3

```bash
python3 scripts/pair_t3.py     # once, on the T3 host: writes ~/.config/skywatch/t3-mcp-token (0600)
docker compose -f docker-compose.yml -f docker-compose.live.yml up -d --build
```

Defaults can be overridden with `T3_USERDATA` (default `~/.t3/userdata`), `SKYWATCH_T3_TOKEN_PATH`
(default `~/.config/skywatch/t3-mcp-token`), `SKYWATCH_T3_MCP_URL` (default
`http://127.0.0.1:3773/mcp`, read the origin from `~/.t3/userdata/server-runtime.json`) and
`SKYWATCH_UID`/`SKYWATCH_GID`, which must match the owner of those files.

What works from the container (verified against a running T3 on this host):

- **Local threads**: the T3 projection database is read through a read-only bind mount of
  `~/.t3/userdata` using `sqlite3 -readonly`. The whole directory is mounted because SQLite needs the
  adjacent `-wal`/`-shm` files; it is read-only, but it also exposes the other files in that directory
  to the container.
- **Linked devices**: the bundled Python bridge calls T3's MCP endpoint with the paired Skywatch
  credential, supplied as a Compose secret (a read-only file mount, never an image layer or env var).
  T3 listens on host loopback only, so the live overlay uses `network_mode: host`.

What does not work from the container:

- The **`t3` CLI** is a host-side wrapper around the desktop AppImage and is not in the image. The
  features that shell out to it are unavailable: the linked-device reachability probe and relaying
  radio messages to **local** threads. Messages to threads on linked devices go through the bridge and
  do work.
- **Host networking** makes `SKYWATCH_PORT` the real host port and the unauthenticated server listens
  on all host interfaces. Firewall it or expose it only through a trusted proxy. A bridge-network
  deployment cannot reach T3's loopback-only listener, so it cannot show live threads.

If the paired token expires, re-run `scripts/pair_t3.py` and `docker compose ... restart`.

### Update

```bash
git pull --ff-only
SKYWATCH_TAG=$(git rev-parse --short HEAD) docker compose [-f ... -f docker-compose.live.yml] up -d --build
```

Tagging each build with the commit keeps earlier images around for rollback. Base images are pinned by
digest in the `Dockerfile`; to refresh them, pull the new tag, copy the digests from
`docker image inspect --format '{{index .RepoDigests 0}}' <image>` and rebuild.

### Rollback

```bash
docker image ls skywatch                          # previous tags
SKYWATCH_TAG=<previous-sha> docker compose [-f ... -f docker-compose.live.yml] up -d --no-build
```

If the previous image is gone, `git checkout <previous-sha>` and rebuild with the same
`SKYWATCH_TAG`. The application keeps no persistent state, so a rollback never needs data migration.

## Tests

```bash
cd server
cargo test
```

### Linked-device polling

Skywatch uses a dedicated T3 MCP client to list linked devices across projects. With T3 running,
run `python3 scripts/pair_t3.py` on the owning host. It uses T3's local scoped session and OAuth
PKCE approval endpoints to create an approval-required client; it never starts a provider run.
The credential is saved with mode 0600 at `~/.config/skywatch/t3-mcp-token`, outside the checkout.
Re-run pairing when it expires or revoke the Skywatch client in T3 access settings.

Override `SKYWATCH_T3_MCP_URL` and `SKYWATCH_T3_TOKEN_FILE` for containers. Mount only the
Skywatch credential, rather than a whole provider home. Python 3 is required for the MCP transport.
A failed device poll retains its last-known aircraft with an offline marker; absence from a page
is not deletion. Confirmed settled threads depart normally. The status bar reports threads omitted
by display capacity. Remote aircraft and actions carry both environment and thread identity.
