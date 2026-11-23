# Skywatch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust-hosted 3D air-traffic-control view of T3 agents and Tower jobs, with clickable aircraft, text radio comms back to the owning threads, and local-LLM summaries.

**Architecture:** `axum` server owns world state, adapters (T3, Tower) and the summariser; a Three.js browser client renders the world and speaks WebSocket. The wire protocol is the stability boundary.

**Tech Stack:** Rust (axum, tokio, serde, tokio-tungstenite), Three.js (ES module, no bundler), Ollama on the devbox.

---

### Task 1: Server skeleton with world heartbeat

**Files:**
- Create: `server/Cargo.toml`, `server/src/main.rs`

- [ ] **Step 1: Scaffold crate**

```toml
# server/Cargo.toml
[package]
name = "skywatch-server"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = { version = "0.7", features = ["ws"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 2: Minimal axum app serving `/health` and a static dir**

```rust
// server/src/main.rs
use axum::{routing::get, Router};

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .fallback_service(get(|| async { "skywatch" }));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8422").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

- [ ] **Step 3: Run and verify**

Run: `cargo run` in `server/`, then `curl localhost:8422/health` → `ok`.

- [ ] **Step 4: Commit** — `git add . && git commit -m "feat: server skeleton with health endpoint"`

### Task 2: World state and snapshot protocol

**Files:**
- Create: `server/src/world.rs`, `server/src/state.rs`
- Test: `server/src/world.rs` (unit tests inline)

- [ ] **Step 1: Failing test — aircraft state transitions**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hold_then_approval_climbs_to_cruise() {
        let mut a = Aircraft::new("SKY1", Kind::Agent);
        a.hold("awaiting approval");
        assert!(matches!(a.state, State::Holding));
        a.release();
        assert!(matches!(a.state, State::Takeoff));
        a.tick(60.0); // 60s of sim
        assert!(matches!(a.state, State::Cruising));
    }
}
```

- [ ] **Step 2: Implement `Aircraft`, `Kind`, `State`, `World`** — `World::tick(dt)` advances positions (holding aircraft circle their stack, drones loiter, landed sit, cruising transit and wrap). Serde `Serialize`/`Deserialize` on all types; `WorldSnapshot` = full aircraft list + last N radio messages.

- [ ] **Step 3: Run tests** — `cargo test` passes.

- [ ] **Step 4: Commit** — `feat: world state, aircraft lifecycle, sim tick`

### Task 3: WebSocket hub

**Files:**
- Create: `server/src/ws.rs`
- Modify: `server/src/main.rs` (route `/ws`, spawn sim tick at 20 Hz broadcasting deltas)

- [ ] **Step 1: Implement** — on connect send `{"type":"snapshot",...}`; then broadcast `{"type":"delta",...}` frames from the tick loop. Client→server frames: `{"type":"message","aircraft_id":..,"text":..}` and `{"type":"approve","aircraft_id":..}` are accepted and echoed onto a broadcast channel for adapters (Task 6) to act on.
- [ ] **Step 2: Verify** — `websocat ws://localhost:8422/ws` shows a snapshot then deltas.
- [ ] **Step 3: Commit** — `feat: websocket hub with snapshot/delta protocol`

### Task 4: Browser world (Three.js)

**Files:**
- Create: `web/index.html`, `web/main.js`, `web/style.css`, `web/radio.js`
- Modify: `server/src/main.rs` (serve `web/` as static files)

- [ ] **Step 1: Scene** — ground plane with grid, sky gradient, directional light. Import Three.js as ES module from a vendored copy (no CDN dependency on the devbox).
- [ ] **Step 2: Aircraft meshes** — simple low-poly plane (fuselage + wings cone geometry) and drone (quad body + rotors). One mesh per server aircraft, driven by delta frames; lerp positions client-side between frames for smoothness.
- [ ] **Step 3: Click → radio panel** — raycast picking opens the panel (name, status, timeline, conversation, message box, Approve/Reject buttons shown only when the aircraft is Holding). Sending posts a `message` frame; Approve posts `approve`.
- [ ] **Step 4: Radio ticker** — bottom-left scrolling list of last 5 chatter lines; `Attention` lines render amber and persist until acknowledged by clicking the aircraft.
- [ ] **Step 5: Verify in browser** — see planes move, click one, send a message, see it appear in the conversation.
- [ ] **Step 6: Commit** — `feat: 3D world, picking, radio panel, ticker`

### Task 5: Demo seeding (fake agents)

**Files:**
- Create: `server/src/seed.rs`
- Modify: `server/src/main.rs` (spawn seeded aircraft with a scripted state machine)

- [ ] **Step 1:** Seed 1 cruising agent, 1 holding agent (with pending approval), 1 landed agent, 2 Reaper drones. Scripted transitions every ~60s so the world is alive without T3/Tower wired up.
- [ ] **Step 2:** Verify end-to-end in browser, including approving the holding agent and watching it take off.
- [ ] **Step 3: Commit** — `feat: scripted demo seed`

### Task 6: T3 + Tower adapters

**Files:**
- Create: `server/src/adapters/mod.rs`, `server/src/adapters/t3.rs`, `server/src/adapters/tower.rs`

- [ ] **Step 1: T3 adapter** — poll/config-driven list of thread IDs; map agent state → aircraft state (working=Cruising, awaiting input=Holding, blocked/failed=Landed). `message`/`approve` frames from the WS are posted into the owning thread via the T3 API/CLI available on the devbox. Discovery of the exact API happens against the devbox; the adapter trait in `adapters/mod.rs` isolates this.
- [ ] **Step 2: Tower adapter** — list Tower Docker services/scheduled jobs as drones; health/status → loiter colour and radio chatter; restart control from the drone panel.
- [ ] **Step 3:** Both adapters are optional at startup (feature/config flags) so the demo mode still works standalone.
- [ ] **Step 4: Commit** — `feat: T3 and Tower adapters behind config flags`

### Task 7: Local-LLM summariser

**Files:**
- Create: `server/src/summariser.rs`

- [ ] **Step 1:** Client for Ollama (`POST /api/generate`, model from config, default `llama3.2`). Input: recent adapter events for an aircraft; output: one ≤12-word radio line, stored as a `Chatter` message; panel digest refreshed every ~5 min per aircraft.
- [ ] **Step 2:** Timeout 5s, on failure fall back to a templated line ("SKY1: task 42 in progress"). Never blocks the tick loop — summarisation runs on a separate task with an mpsc inbox.
- [ ] **Step 3: Commit** — `feat: local LLM summariser with fallback`

### Task 8: Devbox deployment on Tower

**Files:**
- Create: `Dockerfile`, `docker-compose.yml`, `README.md`

- [ ] **Step 1: Multi-stage Dockerfile** — cargo build stage, runtime stage copying binary + `web/`.
- [ ] **Step 2: compose service** on the Tower devbox, port 8422, config file mounted (`config.toml` with T3 thread list, Tower target, Ollama URL).
- [ ] **Step 3:** Deploy with the tower-docker-apps workflow, verify reachable from the browser, README documents config and local dev.
- [ ] **Step 4: Commit** — `feat: containerised deployment for Tower devbox`

---

## Execution handoff

**Plan saved to `docs/plan.md`. Two options:**

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks
2. **Inline Execution** — execute here with checkpoints

**Which approach?**
