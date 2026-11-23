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
