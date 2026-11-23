# Skywatch — Agent Traffic Control

## What it is

A 3D "air traffic control" world, built in Rust, that visualises the state of your
agents and jobs. Every aircraft is a live thing you can talk to. Hosted on the
devbox in Tower, viewable in a browser.

## Core metaphor

| Aircraft | Represents | Flight behaviour |
|---|---|---|
| **Holding pattern** | Agent waiting for your action (approval, question, blocked input) | Circles a holding stack, callsign flashes amber |
| **Reaper drone** | Monitoring job / scheduled task / Tower service | Slow loiter orbit over its target, steady |
| **Landed plane** | Blocked / failed agent, waiting to take off | Parked on the apron, engine off |
| **Cruising plane** | Healthy, working agent | Straight-and-level transit across the sky |

**Airframe class reflects the model powering the agent:**

| Airframe | Model class |
|---|---|
| **Little private plane** (single prop) | Local model (e.g. Ollama on the devbox) |
| **Big jet** (twin-engine airliner) | Frontier model (e.g. Claude, GPT) |
| **Reaper drone** | Monitoring/scheduled (regardless of model) |

State changes move the aircraft: an agent that asks a question peels off into a
holding pattern; an approval received lines it up for takeoff and it climbs back
to cruise.

## Interaction

- **Click an aircraft** → opens its **radio panel**:
  - Callsign, role, current status, the owning T3 thread / Tower service
  - Recent activity timeline (summarised)
  - The conversation so far
  - A message box wired to the owning thread — your instructions go to the agent verbatim
  - Explicit **approve / reject** buttons when the agent is waiting
  - For Tower drones: the service or scheduled job, recent results, restart controls, and a clear indication of who receives your message (the investigating agent vs. the service)
- **Routine chatter is ambient**: short one-line radio transmissions scroll in a
  ticker ("Skylane 2, heading 270, refactor in progress"). Attention requests
  (holds, failures) stand out visually and sonically-subtly (text only, v1).
- **Text comms only** in v1 — no speech synthesis.

## Summaries

Local models (via the devbox, e.g. Ollama) summarise progress events into radio
transmissions and panel digests. Raw agent output stays available in the panel;
the model only compresses. If the local model is unavailable, fall back to
truncation — the world must never stall on the summariser.

## Architecture

```
┌───────────────────────────── devbox (Tower) ─────────────────────────────┐
│                                                                          │
│  skywatch-server (Rust, axum)                                            │
│   • REST + WebSocket hub                                                 │
│   • World state: aircraft, flights, radio log                            │
│   • Adapters:                                                            │
│      - T3 adapter   (threads, tasks, agent state, sending messages)      │
│      - Tower adapter (services, scheduled jobs, health, controls)        │
│   • Summariser: local LLM (Ollama) → radio lines + panel digests         │
│                                                                          │
│  skywatch-viz (served static, Rust→WASM or Rust server + Three.js front) │
│   • 3D world, aircraft, picking, radio panel, ticker                     │
└──────────────────────────────────────────────────────────────────────────┘
```

**Rendering choice:** Rust server (`axum` + `tokio`) owns simulation and state;
browser frontend uses Three.js over WebSocket for v1. (A full Bevy/WASM client is
a possible v2 — the WebSocket protocol is the boundary that keeps that door open.)

## Data model (v1)

- `Aircraft { id, callsign, kind: Agent|Drone, model_class: Local|Frontier, state: Cruising|Holding|Landed|Takeoff|Landing, position, heading, altitude, owner: Ref }`
- `RadioMessage { id, aircraft_id, from: Pilot|Tower|You|Agent, text, at, kind: Chatter|Attention|Instruction }`
- `WorldSnapshot { aircraft: [Aircraft], messages: [RadioMessage] }` — full snapshot on connect, deltas after.

## Non-goals (v1)

- Audio/speech, multiplayer, real flight physics, persistent history database.
