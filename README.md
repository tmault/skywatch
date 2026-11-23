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
