//! T3 adapter: mirrors recent T3 threads, sub-agent wingmen, and linked
//! device airports.
//!
//! Enable with `SKYWATCH_T3=1` (any value except `0`/`false`). Polls the `t3`
//! CLI for linked devices and recent threads, maps run state to flight state,
//! parks stale threads (>24h) at their device airport's terminal gates, spawns
//! sub-agents as delta-wing fighter jets on their parent's wing, and relays
//! radio-panel messages back into the owning thread via `t3 thread send`.

use crate::world::{
    formation_position, Aircraft, Airport, Kind, ModelClass, State, World, GATE_HEADING,
    HOME_GATES, SATELLITE_SLOTS,
};
use crate::ws::{ClientFrame, SharedWorld};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Poll interval for the thread list & subagents.
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// How many active threads (<24h) to fly concurrently.
const MAX_AIRCRAFT: usize = 6;
/// Threads untouched for longer than this park at a terminal gate.
const STALE_AFTER_SECS: i64 = 24 * 3600;
/// How many gate-parked stale threads to show at the home airport.
const MAX_GATE_PARKED: usize = 6;
/// How many fighter wingmen to escort any single parent aircraft.
const MAX_WINGMEN_PER_PARENT: usize = 4;
/// Recently finished sub-agents linger on the wing briefly before peeling off.
const SUBAGENT_LINGER_SECS: i64 = 600;

#[derive(Deserialize)]
struct ThreadListEntry {
    #[serde(rename = "threadId", alias = "thread_id")]
    thread_id: String,
    title: String,
    #[serde(default)]
    status: String,
    model: Option<String>,
    #[serde(rename = "updatedAt", alias = "updated_at")]
    updated_at: Option<String>,
    #[serde(default)]
    pending: i64,
}

#[derive(Deserialize)]
struct ThreadShow {
    thread: ThreadDetail,
}

#[derive(Deserialize)]
struct ThreadDetail {
    #[serde(rename = "pendingRequestCount")]
    pending_request_count: i64,
}

#[derive(Deserialize)]
struct EnvLinkEntry {
    #[serde(rename = "environmentId", alias = "environment_id")]
    environment_id: String,
    label: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    last_error: Option<String>,
}

#[derive(Deserialize)]
struct SubagentRow {
    subagent_id: String,
    thread_id: String,
    child_thread_id: Option<String>,
    status: String,
    updated_at: Option<String>,
    title: Option<String>,
}

/// Stable callsign allocation: thread id -> SKYn, in first-sight order.
#[derive(Default)]
struct Callsigns {
    by_thread: HashMap<String, String>,
    next: usize,
}

impl Callsigns {
    fn get(&mut self, thread_id: &str) -> String {
        self.by_thread.get(thread_id).cloned().unwrap_or_else(|| {
            self.next += 1;
            let cs = format!("SKY{}", self.next);
            self.by_thread.insert(thread_id.to_string(), cs.clone());
            cs
        })
    }
}

/// Stable callsign allocation for gate-parked (>24h idle) threads: GTn.
#[derive(Default)]
struct GateCallsigns {
    by_thread: HashMap<String, String>,
    next: usize,
}

impl GateCallsigns {
    fn get(&mut self, thread_id: &str) -> String {
        self.by_thread.get(thread_id).cloned().unwrap_or_else(|| {
            self.next += 1;
            let cs = format!("GT{}", self.next);
            self.by_thread.insert(thread_id.to_string(), cs.clone());
            cs
        })
    }
}

/// Track active fighter wingmen keyed by subagent_id -> (callsign, child_or_parent_thread_id).
#[derive(Default)]
struct Wingmen {
    by_subagent: HashMap<String, (String, String)>,
}

/// Days since the Unix epoch for a civil (year, month, day) date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse an RFC3339 UTC timestamp ("2026-11-01T17:39:45.542Z") to Unix secs.
fn parse_rfc3339(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    let num = |a: usize, n: usize| s.get(a..a + n)?.parse::<i64>().ok();
    let y = num(0, 4)?;
    let mo = num(5, 2)?;
    let d = num(8, 2)?;
    let h = num(11, 2)?;
    let mi = num(14, 2)?;
    let sec = num(17, 2)?;
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec)
}

/// A thread is stale when its last update is older than 24 hours.
fn is_stale(updated_at: &Option<String>, now: i64) -> bool {
    updated_at
        .as_deref()
        .and_then(parse_rfc3339)
        .map(|t| now - t >= STALE_AFTER_SECS)
        .unwrap_or(false)
}

/// Gate slot i at the home device airport (6 gates along the terminal).
#[cfg(test)]
fn home_gate_slot(i: usize) -> [f32; 2] {
    HOME_GATES[i % HOME_GATES.len()]
}

fn local_device_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".to_string())
}

fn model_class(model: &Option<String>) -> ModelClass {
    let m = model.as_deref().unwrap_or("").to_lowercase();
    if m.contains("local") || m.contains("ollama") {
        ModelClass::Local
    } else {
        ModelClass::Frontier
    }
}

fn is_working_status(status: &str) -> bool {
    matches!(
        status,
        "running" | "starting" | "preparing" | "queued" | "waiting"
    )
}

fn is_blocked_status(status: &str) -> bool {
    matches!(status, "failed" | "interrupted" | "cancelled")
}

fn flight_state(status: &str, pending_requests: i64) -> (State, Option<String>) {
    match status {
        "waiting" => (State::Holding, Some("awaiting your input".to_string())),
        "running" | "starting" | "preparing" | "queued" if pending_requests > 0 => {
            (State::Holding, Some("awaiting your input".to_string()))
        }
        "running" | "starting" | "preparing" | "queued" => (State::Cruising, None),
        "failed" => (State::Landed, Some("run failed".to_string())),
        "interrupted" => (State::Landed, Some("interrupted".to_string())),
        "cancelled" => (State::Landed, Some("cancelled".to_string())),
        // Completed / idle threads stay parked at a terminal gate — only working planes fly.
        _ => (State::Parked, None),
    }
}

fn run_t3(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("t3")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn state_db_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".t3/userdata/statev2.sqlite"))
        .filter(|p| p.exists())
}

fn run_sqlite_json(sql: &str) -> Option<String> {
    let db_path = state_db_path()?;
    let out = std::process::Command::new("sqlite3")
        .args(["-readonly", "-json", &db_path.to_string_lossy(), sql])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Fast airport list from local SQLite (instant startup, before reachability probe).
fn discover_airports_fast(home_name: &str, reachability: &HashMap<String, bool>) -> Vec<Airport> {
    let mut airports = vec![Airport::home("local", home_name)];
    if let Some(raw) = run_sqlite_json(
        "SELECT environment_id, label, last_error FROM peer_environment_links ORDER BY linked_at ASC;",
    ) {
        if let Ok(peers) = serde_json::from_str::<Vec<EnvLinkEntry>>(&raw) {
            for (i, peer) in peers.into_iter().take(SATELLITE_SLOTS.len()).enumerate() {
                let online = reachability
                    .get(&peer.environment_id)
                    .copied()
                    .unwrap_or_else(|| peer.last_error.is_none());
                airports.push(Airport::satellite(
                    peer.environment_id,
                    peer.label,
                    SATELLITE_SLOTS[i],
                    online,
                ));
            }
        }
    }
    airports
}

/// Live reachability probe via `t3 environment list --json` (runs on a slower cadence).
fn probe_environment_reachability() -> HashMap<String, bool> {
    let mut map = HashMap::new();
    if let Some(raw) = run_t3(&["environment", "list", "--json"]) {
        if let Ok(peers) = serde_json::from_str::<Vec<EnvLinkEntry>>(&raw) {
            for peer in peers {
                map.insert(
                    peer.environment_id,
                    peer.status.eq_ignore_ascii_case("reachable"),
                );
            }
        }
    }
    map
}

/// Query T3's local SQLite projection (read-only) for active or very recent
/// sub-agents belonging to any of `parent_thread_ids`.
fn poll_subagents(parent_thread_ids: &HashSet<String>, now: i64) -> Vec<SubagentRow> {
    if parent_thread_ids.is_empty() {
        return Vec::new();
    }
    let linger = std::env::var("SKYWATCH_SUBAGENT_LINGER_SECS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(SUBAGENT_LINGER_SECS);

    let sql = "SELECT s.subagent_id, s.thread_id, s.child_thread_id, s.status, s.updated_at, \
               COALESCE(t.title, json_extract(s.payload_json, '$.title'), json_extract(s.payload_json, '$.role'), 'sub-agent') AS title \
               FROM orchestration_v2_projection_subagents s \
               LEFT JOIN orchestration_v2_projection_threads t ON t.thread_id = s.child_thread_id \
               ORDER BY s.updated_at DESC LIMIT 40;";

    let Some(raw) = run_sqlite_json(sql) else {
        return Vec::new();
    };
    let rows: Vec<SubagentRow> = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    rows.into_iter()
        .filter(|r| parent_thread_ids.contains(&r.thread_id))
        .filter(|r| {
            let active = matches!(
                r.status.as_str(),
                "running" | "starting" | "queued" | "waiting" | "preparing"
            );
            if active {
                return true;
            }
            r.updated_at
                .as_deref()
                .and_then(parse_rfc3339)
                .map(|t| now - t <= linger)
                .unwrap_or(false)
        })
        .collect()
}

/// Query recent threads across all projects from SQLite (fast), or fall back to
/// `t3 thread list --json` for the current directory's project.
fn poll_threads() -> Option<Vec<ThreadListEntry>> {
    let sql = "SELECT t.thread_id, t.title, t.updated_at, \
               json_extract(t.payload_json, '$.modelSelection.model') AS model, \
               COALESCE((SELECT r.status FROM orchestration_v2_projection_runs r \
                         WHERE r.thread_id = t.thread_id \
                         ORDER BY (r.completed_at IS NULL) DESC, r.ordinal DESC LIMIT 1), 'idle') AS status, \
               (SELECT COUNT(*) FROM orchestration_v2_projection_runtime_requests q \
                WHERE q.thread_id = t.thread_id AND q.resolved_at IS NULL) AS pending \
               FROM orchestration_v2_projection_threads t \
               WHERE t.deleted_at IS NULL AND t.archived_at IS NULL \
                 AND (json_extract(t.payload_json, '$.lineage.relationshipToParent') IS NULL \
                      OR json_extract(t.payload_json, '$.lineage.relationshipToParent') != 'subagent') \
               ORDER BY t.updated_at DESC LIMIT 40;";
    if let Some(raw) = run_sqlite_json(sql) {
        if let Ok(entries) = serde_json::from_str::<Vec<ThreadListEntry>>(&raw) {
            if !entries.is_empty() {
                return Some(entries);
            }
        }
    }

    let raw = run_t3(&["thread", "list", "--json"])?;
    let mut entries: Vec<ThreadListEntry> = serde_json::from_str(&raw).ok()?;
    for e in entries.iter_mut().take(MAX_AIRCRAFT) {
        e.pending = run_t3(&[
            "thread",
            "show",
            &e.thread_id,
            "--messages",
            "0",
            "--json",
        ])
        .and_then(|r| serde_json::from_str::<ThreadShow>(&r).ok())
        .map(|show| show.thread.pending_request_count)
        .unwrap_or(0);
    }
    Some(entries)
}

/// All external data collected outside the world mutex so I/O never stalls the
/// 20 Hz simulation tick or WebSocket connections.
struct PollData {
    airports: Vec<Airport>,
    recent: Vec<(ThreadListEntry, i64)>,
    stale: Vec<ThreadListEntry>,
    subagents: Vec<SubagentRow>,
    now: i64,
}

/// Fetch airports, threads, pending-request counts, and sub-agents off-lock.
fn fetch_poll_data(home_name: &str, reachability: &HashMap<String, bool>) -> Option<PollData> {
    let airports = discover_airports_fast(home_name, reachability);
    let entries = poll_threads()?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut recent = Vec::new();
    let mut stale = Vec::new();
    for e in entries {
        // Working or recently blocked (<24h) threads belong to the active fleet.
        // Completed / idle threads (and anything >24h old) park at the terminal gates.
        let active_or_blocked = is_working_status(&e.status)
            || (!is_stale(&e.updated_at, now) && is_blocked_status(&e.status));
        if active_or_blocked {
            if recent.len() < MAX_AIRCRAFT {
                let pending = e.pending;
                recent.push((e, pending));
            }
        } else if stale.len() < MAX_GATE_PARKED {
            stale.push(e);
        }
    }

    let recent_ids: HashSet<String> = recent.iter().map(|(e, _)| e.thread_id.clone()).collect();
    let subagents = poll_subagents(&recent_ids, now);

    Some(PollData {
        airports,
        recent,
        stale,
        subagents,
        now,
    })
}

/// Reconcile the world in-memory from already-fetched `PollData`.
fn apply_poll(
    world: &mut World,
    home_name: &str,
    data: PollData,
    callsigns: &mut Callsigns,
    gates: &mut GateCallsigns,
    wingmen: &mut Wingmen,
) -> Vec<(String, String)> {
    world.airports = data.airports;
    let _ = data.now;

    let mut keep: HashMap<String, (State, Option<String>, Option<String>, ModelClass)> =
        HashMap::new();
    let mut new_threads: Vec<(String, String, ModelClass)> = Vec::new();

    for (entry, pending) in &data.recent {
        let (state, reason) = flight_state(&entry.status, *pending);
        keep.insert(
            entry.thread_id.clone(),
            (
                state,
                reason,
                Some(entry.title.clone()),
                model_class(&entry.model),
            ),
        );
        if callsigns.by_thread.contains_key(&entry.thread_id) {
            continue;
        }
        new_threads.push((
            entry.thread_id.clone(),
            entry.title.clone(),
            model_class(&entry.model),
        ));
    }

    // Remove aircraft whose threads dropped out of the recent set.
    let drop: Vec<String> = callsigns
        .by_thread
        .keys()
        .filter(|id| !keep.contains_key(*id))
        .cloned()
        .collect();
    for id in drop {
        if let Some(cs) = callsigns.by_thread.remove(&id) {
            world.remove_by_callsign(&cs);
        }
    }

    let mut order: Vec<String> = callsigns.by_thread.keys().cloned().collect();
    order.extend(new_threads.iter().map(|(id, _, _)| id.clone()));
    let mut radio_events = Vec::new();

    for thread_id in &order {
        let Some((state, reason, title, _model_class)) = keep.get(thread_id) else {
            continue;
        };
        let cs = callsigns.get(thread_id);
        if let Some(a) = world.get_mut(&cs) {
            let was_on_ground = matches!(a.state, State::Landed | State::Taxi | State::Parked | State::Ready);
            // If an aircraft is already on the ground for a blocked/failed run,
            // let it finish taxiing off the runway instead of stomping back to Landed.
            let target_state = if *state == State::Landed
                && matches!(a.state, State::Landed | State::Taxi | State::Parked)
            {
                a.state
            } else if *state == State::Cruising && was_on_ground {
                // Woke up from the ground: taxi & take off instead of teleporting into the sky!
                if matches!(a.state, State::Landed | State::Parked) {
                    a.release();
                }
                a.state
            } else {
                *state
            };
            if a.state != target_state {
                radio_events.push((
                    cs.clone(),
                    match target_state {
                        State::Holding => "entering hold".to_string(),
                        State::Landed => "landing — blocked".to_string(),
                        State::Cruising => "cruising".to_string(),
                        State::Parked => "parked at gate".to_string(),
                        _ => "updating".to_string(),
                    },
                ));
            }
            if target_state == State::Landed && a.state != State::Landed {
                a.land();
            } else {
                a.state = target_state;
            }
            a.hold_reason = reason.clone();
            a.title = title.clone();
            a.device = Some(home_name.to_string());
        } else if let Some((_, t, mc)) = new_threads.iter().find(|(id, _, _)| id == thread_id) {
            let mut a = Aircraft::with_model(&cs, Kind::Agent, *mc);
            a.title = Some(t.clone());
            a.device = Some(home_name.to_string());
            if let Some((_, reason, title, _)) = keep.get(thread_id) {
                a.hold_reason = reason.clone();
                a.title = title.clone();
            }
            a.state = *state;
            if a.state == State::Landed {
                a.land();
                // Stagger initial runway position slightly if multiple threads start landed.
                let offset = (callsigns.next as f32 % 4.0) * 14.0 - 14.0;
                a.position[1] = (a.position[1] + offset).clamp(-45.0, 28.0);
            }
            let names = ["Deploy", "Refactor", "Monitor"];
            a.assign(names[callsigns.next % names.len()]);
            world.add(a);
        }
    }

    // --- Terminal Gates: idle/completed threads fill only remaining open gates ---
    // Reserve gate capacity for any grounded/blocked active aircraft so arriving
    // SKY planes always have an open gate to taxi into and never park over GT planes.
    let grounded_sky = world
        .iter()
        .filter(|a| {
            !a.callsign.starts_with("GT")
                && a.parent_id.is_none()
                && matches!(
                    a.state,
                    State::Landed | State::Taxi | State::Parked | State::Ready
                )
        })
        .count();
    let max_gt = HOME_GATES.len().saturating_sub(grounded_sky);
    let stale_slice = &data.stale[..data.stale.len().min(max_gt)];
    let stale_ids: HashSet<String> = stale_slice.iter().map(|e| e.thread_id.clone()).collect();

    let gate_drop: Vec<String> = gates
        .by_thread
        .keys()
        .filter(|id| !stale_ids.contains(*id))
        .cloned()
        .collect();
    for id in gate_drop {
        if let Some(cs) = gates.by_thread.remove(&id) {
            world.remove_by_callsign(&cs);
            radio_events.push((cs, "vacating gate".to_string()));
        }
    }

    for (i, entry) in stale_slice.iter().enumerate() {
        let cs = gates.get(&entry.thread_id);
        if let Some(a) = world.get_mut(&cs) {
            a.title = Some(entry.title.clone());
            a.position[2] = 0.0;
            a.altitude = 0.0;
            a.heading = GATE_HEADING;
            a.device = Some(home_name.to_string());
        } else if let Some(slot) = world.find_free_gate(None, i) {
            let mut a = Aircraft::with_model(&cs, Kind::Agent, model_class(&entry.model));
            a.title = Some(entry.title.clone());
            a.state = State::Parked;
            a.position = [slot[0], slot[1], 0.0];
            a.altitude = 0.0;
            a.heading = GATE_HEADING;
            a.topic = None;
            a.device = Some(home_name.to_string());
            world.add(a);
            radio_events.push((cs.clone(), format!("parked at gate {}", i + 1)));
        }
    }

    // --- Sub-agent Fighter Wingmen flanking their parent aircraft ---
    let mut by_parent: HashMap<String, Vec<SubagentRow>> = HashMap::new();
    for s in data.subagents {
        let list = by_parent.entry(s.thread_id.clone()).or_default();
        if list.len() < MAX_WINGMEN_PER_PARENT {
            list.push(s);
        }
    }

    let mut active_sub_ids: HashSet<String> = HashSet::new();
    let suffixes = ["A", "B", "C", "D"];

    for (parent_tid, subs) in by_parent {
        let Some(parent_cs) = callsigns.by_thread.get(&parent_tid).cloned() else {
            continue;
        };
        let parent_info = world
            .iter()
            .find(|a| a.callsign == parent_cs)
            .map(|a| (a.id, a.position, a.heading, a.state));
        let Some((parent_id, p_pos, p_heading, p_state)) = parent_info else {
            continue;
        };
        let on_ground = matches!(
            p_state,
            State::Parked | State::Landed | State::Taxi | State::Ready
        );

        for (slot, sub) in subs.into_iter().enumerate() {
            active_sub_ids.insert(sub.subagent_id.clone());
            let relay_tid = sub
                .child_thread_id
                .clone()
                .unwrap_or_else(|| parent_tid.clone());
            let cs = format!("{parent_cs}-{}", suffixes[slot % suffixes.len()]);
            wingmen
                .by_subagent
                .insert(sub.subagent_id.clone(), (cs.clone(), relay_tid));

            if let Some(a) = world.get_mut(&cs) {
                a.parent_id = Some(parent_id);
                a.wing_slot = Some(slot);
                a.title = sub.title.clone();
                a.device = Some(home_name.to_string());
            } else {
                let start = formation_position(p_pos, p_heading, on_ground, slot);
                let mut fighter = Aircraft::wingman(&cs, parent_id, slot, start, p_heading);
                fighter.title = sub.title.clone();
                fighter.device = Some(home_name.to_string());
                world.add(fighter);
                radio_events.push((cs, format!("scrambled on {parent_cs}'s wing")));
            }
        }
    }

    // Remove fighters whose sub-agents have finished and lingered out.
    let drop_subs: Vec<String> = wingmen
        .by_subagent
        .keys()
        .filter(|id| !active_sub_ids.contains(*id))
        .cloned()
        .collect();
    for id in drop_subs {
        if let Some((cs, _)) = wingmen.by_subagent.remove(&id) {
            world.remove_by_callsign(&cs);
            radio_events.push((cs, "breaking formation — RTB".to_string()));
        }
    }

    radio_events
}

/// Adapter task: polls T3 off-lock and reconciles the world; also relays client
/// messages into the owning threads.
pub async fn run(world: SharedWorld, mut rx: mpsc::Receiver<ClientFrame>) {
    let home_name = local_device_name();
    let mut callsigns = Callsigns::default();
    let mut gates = GateCallsigns::default();
    let mut wingmen = Wingmen::default();

    // Set home airport immediately before first poll returns.
    {
        let mut w = world.lock().await;
        w.airports = vec![Airport::home("local", &home_name)];
    }

    // Shared reachability cache updated slowly in background so fast polls never wait on offline peers.
    let reachability = std::sync::Arc::new(std::sync::RwLock::new(HashMap::<String, bool>::new()));
    {
        let reach = std::sync::Arc::clone(&reachability);
        tokio::spawn(async move {
            loop {
                if let Ok(map) = tokio::task::spawn_blocking(probe_environment_reachability).await {
                    if !map.is_empty() {
                        if let Ok(mut guard) = reach.write() {
                            *guard = map;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }

    // Channel for background poll results so polling never blocks client frames or sim ticks.
    let (poll_tx, mut poll_rx) = mpsc::channel::<PollData>(2);
    {
        let home_clone = home_name.clone();
        let reach = std::sync::Arc::clone(&reachability);
        tokio::spawn(async move {
            loop {
                let hn = home_clone.clone();
                let snap_reach = reach.read().map(|g| g.clone()).unwrap_or_default();
                let res = tokio::task::spawn_blocking(move || fetch_poll_data(&hn, &snap_reach)).await;
                if let Ok(Some(data)) = res {
                    if poll_tx.send(data).await.is_err() {
                        break;
                    }
                } else {
                    tracing::warn!("t3 poll failed; keeping world as-is");
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        });
    }

    loop {
        tokio::select! {
            Some(data) = poll_rx.recv() => {
                let mut w = world.lock().await;
                let events = apply_poll(&mut w, &home_name, data, &mut callsigns, &mut gates, &mut wingmen);
                for (cs, what) in &events {
                    tracing::info!(callsign = cs, what, "t3 aircraft update");
                }
            }
            Some(frame) = rx.recv() => {
                let (aircraft_id, text) = match frame {
                    ClientFrame::Message { aircraft_id, text } => (aircraft_id, text),
                    ClientFrame::Approve { aircraft_id } => (aircraft_id, "Approved — please continue.".to_string()),
                };
                let callsign = {
                    let w = world.lock().await;
                    let cs = w
                        .iter()
                        .find(|a| a.id == aircraft_id)
                        .map(|a| a.callsign.clone());
                    cs
                };
                let Some(callsign) = callsign else {
                    tracing::warn!(aircraft_id, "unknown aircraft; dropping message");
                    continue;
                };
                // Active fleet -> gate-parked threads -> fighter wingmen.
                let thread_id = callsigns
                    .by_thread
                    .iter()
                    .find(|(_, cs)| **cs == callsign)
                    .map(|(id, _)| id.clone())
                    .or_else(|| {
                        gates
                            .by_thread
                            .iter()
                            .find(|(_, cs)| **cs == callsign)
                            .map(|(id, _)| id.clone())
                    })
                    .or_else(|| {
                        wingmen
                            .by_subagent
                            .values()
                            .find(|(cs, _)| *cs == callsign)
                            .map(|(_, tid)| tid.clone())
                    });
                let Some(thread_id) = thread_id else {
                    tracing::warn!(callsign, "no thread for callsign; dropping message");
                    continue;
                };
                tracing::info!(callsign, thread_id, "relaying message to t3 thread");
                tokio::task::spawn_blocking(move || {
                    if let Some(out) = run_t3(&["thread", "send", &thread_id, &text]) {
                        tracing::debug!(thread_id, "t3 send ok: {out}");
                    } else {
                        tracing::error!(thread_id, "t3 thread send failed");
                    }
                });
            }
            else => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_to_unix_secs() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-02T00:00:00Z"), Some(86400));
        assert!(parse_rfc3339("2026-11-01T17:39:45.542Z").is_some());
        assert_eq!(parse_rfc3339("garbage"), None);
    }

    #[test]
    fn staleness_boundary_is_24_hours() {
        let now = 1_000_000_000i64;
        let ts = |secs_ago: i64| {
            let t = now - secs_ago;
            let days = t.div_euclid(86400);
            let rem = t.rem_euclid(86400);
            let (y, m, d) = civil_from_days(days);
            Some(format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                y,
                m,
                d,
                rem / 3600,
                (rem % 3600) / 60,
                rem % 60
            ))
        };
        assert!(!is_stale(&ts(24 * 3600 - 60), now));
        assert!(is_stale(&ts(24 * 3600), now));
        assert!(is_stale(&ts(48 * 3600), now));
        assert!(!is_stale(&None, now));
    }

    fn civil_from_days(z: i64) -> (i64, i64, i64) {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (if m <= 2 { y + 1 } else { y }, m, d)
    }

    #[test]
    fn home_gates_are_spaced_along_terminal() {
        let g0 = home_gate_slot(0);
        let g1 = home_gate_slot(1);
        assert_eq!(g0[0], g1[0]);
        assert_eq!(g1[1] - g0[1], 20.0);
    }

    #[test]
    fn only_working_threads_fly() {
        assert_eq!(flight_state("running", 0).0, State::Cruising);
        assert_eq!(flight_state("starting", 0).0, State::Cruising);
        assert_eq!(flight_state("running", 1).0, State::Holding);
        assert_eq!(flight_state("waiting", 0).0, State::Holding);
        assert_eq!(flight_state("failed", 0).0, State::Landed);
        assert_eq!(flight_state("interrupted", 0).0, State::Landed);
        assert_eq!(flight_state("completed", 0).0, State::Parked);
        assert_eq!(flight_state("idle", 0).0, State::Parked);
    }
}
