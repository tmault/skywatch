//! T3 adapter: mirrors recent T3 threads, sub-agent wingmen, and linked
//! device airports.
//!
//! Enable with `SKYWATCH_T3=1` (any value except `0`/`false`). Polls the `t3`
//! CLI for linked devices and recent threads, maps run state to flight state,
//! parks stale threads (>24h) at their device airport's terminal gates, spawns
//! sub-agents as delta-wing fighter jets on their parent's wing, and relays
//! radio-panel messages back into the owning thread via `t3 thread send`.

use crate::world::{
    default_sectors, formation_position, Aircraft, Airport, Kind, ModelClass, Sector, State, World,
    GATE_HEADING, HOME_GATES, PROJECT_SECTOR_RADIUS, PROJECT_SECTOR_SLOTS, RUNWAY_HEADING,
    RUNWAY_THRESHOLD, SATELLITE_SLOTS, TAXIWAY_QUEUE,
};
use crate::ws::{ClientFrame, SharedWorld};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Poll interval for the thread list & subagents.
const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// How many actively working threads to fly concurrently.
const MAX_AIRCRAFT: usize = 6;
/// How many recent (<24h) idle/waiting threads to queue along Taxiway Alpha.
const MAX_TAXIWAY_QUEUE: usize = TAXIWAY_QUEUE.len();
/// Threads untouched for longer than this park at the extended terminal gates.
const STALE_AFTER_SECS: i64 = 24 * 3600;
/// How many stale (>24h) threads to park along the extended gate concourse.
const MAX_GATE_PARKED: usize = HOME_GATES.len();
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
    project: Option<String>,
    #[serde(default)]
    status: String,
    model: Option<String>,
    #[serde(rename = "updatedAt", alias = "updated_at")]
    updated_at: Option<String>,
    #[serde(default)]
    pending: i64,
    #[serde(default)]
    settled: bool,
    #[serde(rename = "settledAt", alias = "settled_at", default)]
    settled_at: Option<String>,
}

#[derive(Deserialize)]
struct ThreadIdRow {
    thread_id: String,
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
    #[serde(default)]
    model: Option<String>,
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

/// Stable callsign allocation for taxiway departure queue (<24h idle/waiting): TXn.
#[derive(Default)]
struct TaxiwayCallsigns {
    by_thread: HashMap<String, String>,
    next: usize,
}

impl TaxiwayCallsigns {
    fn get(&mut self, thread_id: &str) -> String {
        self.by_thread.get(thread_id).cloned().unwrap_or_else(|| {
            self.next += 1;
            let cs = format!("TX{}", self.next);
            self.by_thread.insert(thread_id.to_string(), cs.clone());
            cs
        })
    }
}

/// Stable callsign allocation for gate-parked (>24h untouched) threads: GTn.
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

/// Threads whose aircraft is flying an approach to land and backtrack to the
/// taxiway queue: thread id -> airborne callsign (SKYn).
#[derive(Default)]
struct RunwayOps {
    arrivals: HashMap<String, String>,
    /// False until the first poll is applied: threads already running at
    /// startup appear in the air rather than all queueing for takeoff at once.
    primed: bool,
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

/// Hybrid zone classifier:
/// - Named T3 projects (anything other than "No project") get their own dynamic project zone.
/// - "No project" threads are routed into Build, Homelab, AI & Models, or Research by title keywords.
/// Returns `(zone_name, is_project_zone)`.
fn classify_zone(project: Option<&str>, title: &str) -> (String, bool) {
    if let Some(proj) = project.map(str::trim).filter(|p| !p.is_empty()) {
        if !proj.eq_ignore_ascii_case("no project") {
            return (proj.to_string(), true);
        }
    }

    let t = title.to_lowercase();
    let has_any = |words: &[&str]| words.iter().any(|w| t.contains(w));

    if has_any(&[
        "tower",
        "docker",
        "homelab",
        "backup",
        "hdd",
        "ssd",
        "nas",
        "amd cpu",
        "cpu upgrade",
        "lancool",
        "calibre",
        "searxng",
        "trmnl",
        "rpi",
        "haos",
        "home assistant",
        "tailscale",
        "devbox",
        "cron",
        "mount",
        "4tb",
        "240gb",
    ]) {
        return ("Homelab".to_string(), false);
    }

    if has_any(&[
        "vllm",
        "llama",
        "ollama",
        "qwen",
        "glm",
        "kimi",
        "gemini",
        "claude",
        "gpt",
        "opencode",
        "radiance",
        "inkling",
        "maya",
        "model",
        "prompt",
        "mcp",
        "skill",
        "antigravity",
        "llm",
        "benchmark",
        "strata",
        "gpu",
        "token",
    ]) {
        return ("AI & Models".to_string(), false);
    }

    if has_any(&[
        "repo",
        "site",
        "web",
        "app",
        "rust",
        "three",
        "mesh",
        "plane",
        "aeroplane",
        "island",
        "flight",
        "visuali",
        "geometry",
        "graveyard",
        "airport",
        "code",
        "refactor",
        "build",
        "deploy",
        "frontend",
        "blog",
        "flashcard",
        "nightshift",
        "skywatch",
    ]) {
        return ("Build".to_string(), false);
    }

    ("Research".to_string(), false)
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

fn flight_state(status: &str, pending_requests: i64) -> (State, Option<String>) {
    match status {
        "waiting" => (State::Holding, Some("awaiting your input".to_string())),
        "running" | "starting" | "preparing" | "queued" if pending_requests > 0 => {
            (State::Holding, Some("awaiting your input".to_string()))
        }
        "running" | "starting" | "preparing" | "queued" => (State::Cruising, None),
        "failed" => (State::Ready, Some("run failed — waiting on taxiway".to_string())),
        "interrupted" => (State::Ready, Some("interrupted — waiting on taxiway".to_string())),
        "cancelled" => (State::Ready, Some("cancelled — waiting on taxiway".to_string())),
        // Non-working threads never fly; <24h queue on Taxiway Alpha, >24h park at gates.
        _ => (State::Ready, None),
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
               COALESCE(t.title, json_extract(s.payload_json, '$.title'), json_extract(s.payload_json, '$.role'), 'sub-agent') AS title, \
               json_extract(t.payload_json, '$.modelSelection.model') AS model \
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

/// Query recent active (non-deleted, non-archived, non-settled) threads across
/// all projects from SQLite (fast), or fall back to `t3 thread list --json`.
/// Also returns the set of recently done/archived/deleted thread IDs so any
/// aircraft already in the world can taxi out, take off, and fly away forever.
fn poll_threads() -> Option<(Vec<ThreadListEntry>, HashSet<String>)> {
    let active_sql = "SELECT t.thread_id, t.title, t.updated_at, \
               p.title AS project, \
               json_extract(t.payload_json, '$.modelSelection.model') AS model, \
               COALESCE((SELECT r.status FROM orchestration_v2_projection_runs r \
                         WHERE r.thread_id = t.thread_id \
                         ORDER BY (r.completed_at IS NULL) DESC, r.ordinal DESC LIMIT 1), 'idle') AS status, \
               (SELECT COUNT(*) FROM orchestration_v2_projection_runtime_requests q \
                WHERE q.thread_id = t.thread_id AND q.resolved_at IS NULL) AS pending \
               FROM orchestration_v2_projection_threads t \
               LEFT JOIN projection_projects p ON p.project_id = t.project_id \
               WHERE t.deleted_at IS NULL \
                 AND t.archived_at IS NULL \
                 AND json_extract(t.payload_json, '$.archivedAt') IS NULL \
                 AND ( \
                   (json_extract(t.payload_json, '$.settledAt') IS NULL \
                    AND COALESCE(json_extract(t.payload_json, '$.settledOverride'), '') != 'settled') \
                   OR COALESCE(json_extract(t.payload_json, '$.settledOverride'), '') = 'unsettled' \
                 ) \
                 AND (json_extract(t.payload_json, '$.lineage.relationshipToParent') IS NULL \
                      OR json_extract(t.payload_json, '$.lineage.relationshipToParent') != 'subagent') \
               ORDER BY t.updated_at DESC LIMIT 60;";
    if let Some(raw) = run_sqlite_json(active_sql) {
        let parsed: Result<Vec<ThreadListEntry>, _> = if raw.trim().is_empty() {
            Ok(Vec::new())
        } else {
            serde_json::from_str(&raw)
        };
        if let Ok(entries) = parsed {
            let done_or_archived = poll_done_or_archived_ids().unwrap_or_default();
            return Some((entries, done_or_archived));
        }
    }

    let raw = run_t3(&["thread", "list", "--json"])?;
    let all_entries: Vec<ThreadListEntry> = serde_json::from_str(&raw).ok()?;
    let mut entries = Vec::new();
    let mut done_or_archived = HashSet::new();
    for e in all_entries {
        if e.settled || e.settled_at.is_some() {
            done_or_archived.insert(e.thread_id);
        } else {
            entries.push(e);
        }
    }
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
    Some((entries, done_or_archived))
}

/// Query recently settled (marked done), archived, or deleted thread IDs from SQLite.
fn poll_done_or_archived_ids() -> Option<HashSet<String>> {
    let sql = "SELECT t.thread_id \
               FROM orchestration_v2_projection_threads t \
               WHERE t.deleted_at IS NOT NULL \
                  OR t.archived_at IS NOT NULL \
                  OR json_extract(t.payload_json, '$.archivedAt') IS NOT NULL \
                  OR (COALESCE(json_extract(t.payload_json, '$.settledOverride'), '') != 'unsettled' \
                      AND (json_extract(t.payload_json, '$.settledAt') IS NOT NULL \
                           OR COALESCE(json_extract(t.payload_json, '$.settledOverride'), '') = 'settled')) \
               ORDER BY COALESCE(t.deleted_at, t.archived_at, json_extract(t.payload_json, '$.archivedAt'), json_extract(t.payload_json, '$.settledAt'), t.updated_at) DESC \
               LIMIT 200;";
    let raw = run_sqlite_json(sql)?;
    if raw.trim().is_empty() {
        return Some(HashSet::new());
    }
    let rows: Vec<ThreadIdRow> = serde_json::from_str(&raw).ok()?;
    Some(rows.into_iter().map(|r| r.thread_id).collect())
}

/// All external data collected outside the world mutex so I/O never stalls the
/// 20 Hz simulation tick or WebSocket connections.
struct PollData {
    airports: Vec<Airport>,
    /// Actively working threads (flying or holding).
    flying: Vec<(ThreadListEntry, i64)>,
    /// Recent (<24h) non-running threads queued on Taxiway Alpha waiting for takeoff.
    taxiway: Vec<ThreadListEntry>,
    /// Stale (>24h untouched) threads parked at the extended terminal gates.
    stale: Vec<ThreadListEntry>,
    /// Thread IDs confirmed settled (marked done), archived, or deleted in T3.
    done_or_archived: HashSet<String>,
    subagents: Vec<SubagentRow>,
    now: i64,
}

/// Fetch airports, threads, pending-request counts, and sub-agents off-lock.
fn fetch_poll_data(home_name: &str, reachability: &HashMap<String, bool>) -> Option<PollData> {
    let airports = discover_airports_fast(home_name, reachability);
    let (entries, done_or_archived) = poll_threads()?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut flying = Vec::new();
    let mut taxiway = Vec::new();
    let mut stale = Vec::new();
    for e in entries {
        if is_working_status(&e.status) {
            if flying.len() < MAX_AIRCRAFT {
                let pending = e.pending;
                flying.push((e, pending));
            }
        } else if is_stale(&e.updated_at, now) {
            if stale.len() < MAX_GATE_PARKED {
                stale.push(e);
            }
        } else if taxiway.len() < MAX_TAXIWAY_QUEUE {
            taxiway.push(e);
        }
    }

    let flying_ids: HashSet<String> = flying.iter().map(|(e, _)| e.thread_id.clone()).collect();
    let subagents = poll_subagents(&flying_ids, now);

    Some(PollData {
        airports,
        flying,
        taxiway,
        stale,
        done_or_archived,
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
    taxiway_cs: &mut TaxiwayCallsigns,
    gates: &mut GateCallsigns,
    wingmen: &mut Wingmen,
    runway: &mut RunwayOps,
) -> Vec<(String, String)> {
    world.airports = data.airports;
    let _ = data.now;

    let mut keep: HashMap<String, (State, Option<String>, Option<String>, ModelClass, String)> =
        HashMap::new();
    let mut new_threads: Vec<(String, String, ModelClass)> = Vec::new();
    let mut active_projects: Vec<String> = Vec::new();
    let mut models: HashMap<String, Option<String>> = HashMap::new();

    for (entry, pending) in &data.flying {
        models.insert(entry.thread_id.clone(), entry.model.clone());
        let (state, reason) = flight_state(&entry.status, *pending);
        let (zone, is_project) = classify_zone(entry.project.as_deref(), &entry.title);
        if is_project && !active_projects.contains(&zone) {
            active_projects.push(zone.clone());
        }
        keep.insert(
            entry.thread_id.clone(),
            (
                state,
                reason,
                Some(entry.title.clone()),
                model_class(&entry.model),
                zone,
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

    // A thread that starts working again mid-arrival goes around and keeps its callsign.
    let resumed: Vec<(String, String)> = runway
        .arrivals
        .iter()
        .filter(|(id, _)| keep.contains_key(*id))
        .map(|(id, cs)| (id.clone(), cs.clone()))
        .collect();
    for (id, cs) in resumed {
        runway.arrivals.remove(&id);
        if let Some(a) = world.get_mut(&cs) {
            a.go_around();
            callsigns.by_thread.insert(id.clone(), cs);
            new_threads.retain(|(tid, _, _)| *tid != id);
        }
    }

    // Rebuild sectors: 4 fixed domain zones + dynamic zones for active named projects.
    let mut sectors = default_sectors();
    for (idx, proj_name) in active_projects
        .iter()
        .take(PROJECT_SECTOR_SLOTS.len())
        .enumerate()
    {
        sectors.push(Sector {
            name: proj_name.clone(),
            center: PROJECT_SECTOR_SLOTS[idx],
            radius: PROJECT_SECTOR_RADIUS,
            kind: "project".to_string(),
        });
    }
    world.sectors = sectors;

    // Track every thread still active in any pool so we only trigger farewell
    // departures for threads genuinely marked done / archived / deleted.
    let mut all_active_ids: HashSet<String> = HashSet::new();
    for (e, _) in &data.flying {
        all_active_ids.insert(e.thread_id.clone());
    }
    for e in &data.taxiway {
        all_active_ids.insert(e.thread_id.clone());
    }
    for e in &data.stale {
        all_active_ids.insert(e.thread_id.clone());
    }

    let mut radio_events = Vec::new();
    let taxi_ids: HashSet<String> = data.taxiway.iter().map(|e| e.thread_id.clone()).collect();

    // Remove airborne SKY aircraft whose threads stopped running (they'll appear in taxiway),
    // or send them off into the horizon forever if marked done / archived.
    let drop: Vec<String> = callsigns
        .by_thread
        .keys()
        .filter(|id| !keep.contains_key(*id))
        .cloned()
        .collect();
    for id in drop {
        if let Some(cs) = callsigns.by_thread.remove(&id) {
            if !all_active_ids.contains(&id) && data.done_or_archived.contains(&id) {
                world.depart_by_callsign(&cs);
                radio_events.push((cs, "thread done — departing into the horizon".to_string()));
            } else if runway.primed
                && taxi_ids.contains(&id)
                && world
                    .get_mut(&cs)
                    .map(|a| a.begin_approach(Some(TAXIWAY_QUEUE[0])))
                    .unwrap_or(false)
            {
                // Turn finished: fly the approach, land and backtrack to the taxiway.
                runway.arrivals.insert(id, cs.clone());
                radio_events.push((cs, "on approach, Runway 36".to_string()));
            } else {
                world.remove_by_callsign(&cs);
            }
        }
    }

    let mut order: Vec<String> = callsigns.by_thread.keys().cloned().collect();
    order.extend(new_threads.iter().map(|(id, _, _)| id.clone()));

    for thread_id in &order {
        let Some((state, reason, title, _model_class, zone)) = keep.get(thread_id) else {
            continue;
        };
        let cs = callsigns.get(thread_id);
        if let Some(a) = world.get_mut(&cs) {
            // Let takeoffs and go-arounds finish; the steady state applies after.
            if !a.in_runway_sequence() {
                if a.state != *state {
                    radio_events.push((
                        cs.clone(),
                        match state {
                            State::Holding => "entering hold".to_string(),
                            State::Cruising => "cruising".to_string(),
                            _ => "updating".to_string(),
                        },
                    ));
                }
                a.state = *state;
            }
            a.hold_reason = reason.clone();
            a.title = title.clone();
            a.topic = Some(zone.clone());
            a.model = models.get(thread_id).cloned().flatten();
            a.device = Some(home_name.to_string());
        } else if let Some((_, t, mc)) = new_threads.iter().find(|(id, _, _)| id == thread_id) {
            // Woken from the taxiway queue or a gate: the same airframe taxis
            // out and takes off instead of a new one popping into the sky.
            let ground_cs = taxiway_cs
                .by_thread
                .get(thread_id)
                .or_else(|| gates.by_thread.get(thread_id))
                .cloned();
            if let Some(gcs) = ground_cs {
                if world.rename(&gcs, &cs) {
                    if let Some(a) = world.get_mut(&cs) {
                        a.title = title.clone();
                        a.model = models.get(thread_id).cloned().flatten();
                        a.model_class = *mc;
                        a.assign(zone);
                        a.release();
                        a.hold_reason = reason.clone();
                    }
                    radio_events.push((cs.clone(), "cleared for takeoff, Runway 36".to_string()));
                    continue;
                }
            }
            let mut a = Aircraft::with_model(&cs, Kind::Agent, *mc);
            a.title = Some(t.clone());
            a.model = models.get(thread_id).cloned().flatten();
            a.device = Some(home_name.to_string());
            if let Some((_, reason, title, _, zone)) = keep.get(thread_id) {
                a.hold_reason = reason.clone();
                a.title = title.clone();
                a.assign(zone);
            }
            a.state = *state;
            let a_reason = a.hold_reason.clone();
            if runway.primed {
                // Brand-new thread: line up on Runway 36 and take off.
                a.position = [RUNWAY_THRESHOLD[0], RUNWAY_THRESHOLD[1], 0.0];
                a.altitude = 0.0;
                a.heading = RUNWAY_HEADING;
                a.state = State::Ready;
                a.release();
                a.hold_reason = a_reason;
                radio_events.push((cs.clone(), "lined up, Runway 36".to_string()));
            }
            world.add(a);
        }
    }

    // --- Taxiway Alpha Departure Queue: recent (<24h) non-working threads waiting for takeoff ---
    let taxi_drop: Vec<String> = taxiway_cs
        .by_thread
        .keys()
        .filter(|id| !taxi_ids.contains(*id))
        .cloned()
        .collect();
    for id in taxi_drop {
        if let Some(cs) = taxiway_cs.by_thread.remove(&id) {
            if !all_active_ids.contains(&id) && data.done_or_archived.contains(&id) {
                world.depart_by_callsign(&cs);
                radio_events.push((
                    cs,
                    "thread done — taxiing to Runway 36 for final departure".to_string(),
                ));
            } else {
                world.remove_by_callsign(&cs);
                radio_events.push((cs, "cleared for departure".to_string()));
            }
        }
    }

    for (i, entry) in data.taxiway.iter().enumerate() {
        let cs = taxiway_cs.get(&entry.thread_id);
        if let Some(acs) = runway.arrivals.get(&entry.thread_id).cloned() {
            let arrived = world
                .get_mut(&acs)
                .map(|a| a.state == State::Ready && a.arrival_slot().is_some());
            match arrived {
                // Still landing or backtracking: it becomes this TX when it arrives.
                Some(false) => continue,
                Some(true) => {
                    world.rename(&acs, &cs);
                    if let Some(a) = world.get_mut(&cs) {
                        a.set_arrival_slot(None);
                    }
                    radio_events.push((cs.clone(), "vacated runway, holding on Alpha".to_string()));
                }
                None => {}
            }
            runway.arrivals.remove(&entry.thread_id);
        }
        let (slot_pos, slot_hdg) = TAXIWAY_QUEUE[i % TAXIWAY_QUEUE.len()];
        let (_, reason) = flight_state(&entry.status, entry.pending);
        let (zone, _) = classify_zone(entry.project.as_deref(), &entry.title);
        if let Some(a) = world.get_mut(&cs) {
            a.title = Some(entry.title.clone());
            a.hold_reason = reason;
            a.topic = Some(zone);
            a.state = State::Ready;
            a.position = [slot_pos[0], slot_pos[1], 0.0];
            a.altitude = 0.0;
            a.heading = slot_hdg;
            a.model = entry.model.clone();
            a.device = Some(home_name.to_string());
        } else {
            let mut a = Aircraft::with_model(&cs, Kind::Agent, model_class(&entry.model));
            a.title = Some(entry.title.clone());
            a.model = entry.model.clone();
            a.hold_reason = reason;
            a.topic = Some(zone);
            a.state = State::Ready;
            a.position = [slot_pos[0], slot_pos[1], 0.0];
            a.altitude = 0.0;
            a.heading = slot_hdg;
            a.device = Some(home_name.to_string());
            world.add(a);
            radio_events.push((cs, format!("queued on Taxiway Alpha #{}", i + 1)));
        }
    }

    // Arrivals whose thread left the taxiway pool (archived, gone stale) mid-landing.
    let orphaned: Vec<(String, String)> = runway
        .arrivals
        .iter()
        .filter(|(id, _)| !taxi_ids.contains(*id))
        .map(|(id, cs)| (id.clone(), cs.clone()))
        .collect();
    for (id, cs) in orphaned {
        if data.done_or_archived.contains(&id) {
            runway.arrivals.remove(&id);
            world.depart_by_callsign(&cs);
        } else if world
            .get_mut(&cs)
            .map(|a| a.state == State::Ready)
            .unwrap_or(true)
        {
            runway.arrivals.remove(&id);
            world.remove_by_callsign(&cs);
        }
    }

    // --- Extended Terminal Gates: STRICTLY threads untouched for >24 hours ---
    let stale_ids: HashSet<String> = data.stale.iter().map(|e| e.thread_id.clone()).collect();
    let gate_drop: Vec<String> = gates
        .by_thread
        .keys()
        .filter(|id| !stale_ids.contains(*id))
        .cloned()
        .collect();
    for id in gate_drop {
        if let Some(cs) = gates.by_thread.remove(&id) {
            if !all_active_ids.contains(&id) && data.done_or_archived.contains(&id) {
                world.depart_by_callsign(&cs);
                radio_events.push((
                    cs,
                    "thread archived — pushing back for final departure".to_string(),
                ));
            } else {
                world.remove_by_callsign(&cs);
                radio_events.push((cs, "pushing back from gate".to_string()));
            }
        }
    }

    for (i, entry) in data.stale.iter().enumerate() {
        let cs = gates.get(&entry.thread_id);
        let slot = HOME_GATES[i % HOME_GATES.len()];
        if let Some(a) = world.get_mut(&cs) {
            a.title = Some(entry.title.clone());
            a.state = State::Parked;
            a.position = [slot[0], slot[1], 0.0];
            a.altitude = 0.0;
            a.heading = GATE_HEADING;
            a.model = entry.model.clone();
            a.device = Some(home_name.to_string());
        } else {
            let mut a = Aircraft::with_model(&cs, Kind::Agent, model_class(&entry.model));
            a.title = Some(entry.title.clone());
            a.model = entry.model.clone();
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
            .map(|a| (a.id, a.position, a.heading, a.state, a.model.clone()));
        let Some((parent_id, p_pos, p_heading, p_state, p_model)) = parent_info else {
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

            // Wingmen wear their own model's livery, falling back to the parent's.
            let model = sub.model.clone().or_else(|| p_model.clone());
            if let Some(a) = world.get_mut(&cs) {
                a.parent_id = Some(parent_id);
                a.wing_slot = Some(slot);
                a.title = sub.title.clone();
                a.model = model;
                a.device = Some(home_name.to_string());
            } else {
                let start = formation_position(p_pos, p_heading, on_ground, slot);
                let mut fighter = Aircraft::wingman(&cs, parent_id, slot, start, p_heading);
                fighter.title = sub.title.clone();
                fighter.model = model;
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
            // If the parent is flying off into the horizon, keep escorting it until despawn.
            if !world.is_parent_departing(&cs) {
                world.remove_by_callsign(&cs);
                radio_events.push((cs, "breaking formation — RTB".to_string()));
            }
        }
    }

    runway.primed = true;
    radio_events
}

/// Adapter task: polls T3 off-lock and reconciles the world; also relays client
/// messages into the owning threads.
pub async fn run(world: SharedWorld, mut rx: mpsc::Receiver<ClientFrame>) {
    let home_name = local_device_name();
    let mut callsigns = Callsigns::default();
    let mut taxiway_cs = TaxiwayCallsigns::default();
    let mut gates = GateCallsigns::default();
    let mut wingmen = Wingmen::default();
    let mut runway = RunwayOps::default();

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
                let events = apply_poll(
                    &mut w,
                    &home_name,
                    data,
                    &mut callsigns,
                    &mut taxiway_cs,
                    &mut gates,
                    &mut wingmen,
                    &mut runway,
                );
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
                // Active fleet -> taxiway queue -> gate-parked threads -> fighter wingmen.
                let thread_id = callsigns
                    .by_thread
                    .iter()
                    .find(|(_, cs)| **cs == callsign)
                    .map(|(id, _)| id.clone())
                    .or_else(|| {
                        runway
                            .arrivals
                            .iter()
                            .find(|(_, cs)| **cs == callsign)
                            .map(|(id, _)| id.clone())
                    })
                    .or_else(|| {
                        taxiway_cs
                            .by_thread
                            .iter()
                            .find(|(_, cs)| **cs == callsign)
                            .map(|(id, _)| id.clone())
                    })
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
        assert!((g1[1] - g0[1] - 19.0).abs() < 1e-3);
        assert_eq!(HOME_GATES.len(), 10);
    }

    #[test]
    fn only_working_threads_fly() {
        assert_eq!(flight_state("running", 0).0, State::Cruising);
        assert_eq!(flight_state("starting", 0).0, State::Cruising);
        assert_eq!(flight_state("running", 1).0, State::Holding);
        assert_eq!(flight_state("waiting", 0).0, State::Holding);
        assert_eq!(flight_state("failed", 0).0, State::Ready);
        assert_eq!(flight_state("interrupted", 0).0, State::Ready);
        assert_eq!(flight_state("completed", 0).0, State::Ready);
        assert_eq!(flight_state("idle", 0).0, State::Ready);
    }

    #[test]
    fn hybrid_zone_classifier_routes_projects_and_domains() {
        assert_eq!(
            classify_zone(Some("Skywatch"), "anything"),
            ("Skywatch".to_string(), true)
        );
        assert_eq!(
            classify_zone(Some("No project"), "Check Tower Docker Auto-Updates"),
            ("Homelab".to_string(), false)
        );
        assert_eq!(
            classify_zone(None, "Qwen vLLM GFX1201 Launcher Setup"),
            ("AI & Models".to_string(), false)
        );
        assert_eq!(
            classify_zone(Some("No project"), "Rust Agent Flight Visualization"),
            ("Build".to_string(), false)
        );
        assert_eq!(
            classify_zone(None, "Download Classic Literature on Shelfmark"),
            ("Research".to_string(), false)
        );
    }

    fn make_entry(id: &str, title: &str, status: &str) -> ThreadListEntry {
        ThreadListEntry {
            thread_id: id.to_string(),
            title: title.to_string(),
            project: None,
            status: status.to_string(),
            model: Some("claude-opus-5-5".to_string()),
            updated_at: None,
            pending: 0,
            settled: false,
            settled_at: None,
        }
    }

    #[test]
    fn archiving_or_settling_thread_triggers_final_departure_instead_of_pop_remove() {
        let mut world = World::new();
        let mut callsigns = Callsigns::default();
        let mut taxiway_cs = TaxiwayCallsigns::default();
        let mut gates = GateCallsigns::default();
        let mut wingmen = Wingmen::default();
        let mut runway = RunwayOps::default();

        // Poll 1: one flying (with a wingman), one on taxiway, one parked at a gate.
        let poll1 = PollData {
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![(make_entry("t-fly", "Active Flight", "running"), 0)],
            taxiway: vec![make_entry("t-taxi", "Waiting Thread", "idle")],
            stale: vec![make_entry("t-gate", "Old Thread", "idle")],
            done_or_archived: HashSet::new(),
            subagents: vec![SubagentRow {
                subagent_id: "sub-1".to_string(),
                thread_id: "t-fly".to_string(),
                child_thread_id: None,
                status: "running".to_string(),
                updated_at: None,
                title: Some("Scout".to_string()),
                model: None,
            }],
            now: 1_700_000_000,
        };
        apply_poll(
            &mut world,
            "omarchy",
            poll1,
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        assert_eq!(world.aircraft.len(), 4); // SKY1, SKY1-A, TX1, GT1

        // Poll 2: all three threads are marked done / archived in T3!
        let mut done = HashSet::new();
        done.insert("t-fly".to_string());
        done.insert("t-taxi".to_string());
        done.insert("t-gate".to_string());
        let poll2 = PollData {
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![],
            taxiway: vec![],
            stale: vec![],
            done_or_archived: done,
            subagents: vec![],
            now: 1_700_000_010,
        };
        apply_poll(
            &mut world,
            "omarchy",
            poll2,
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );

        // None of them should have popped out of existence — all are departing!
        assert_eq!(world.aircraft.len(), 4);
        assert_eq!(world.get_mut("SKY1").unwrap().state, State::Departing);
        assert_eq!(world.get_mut("TX1").unwrap().state, State::Taxi);
        assert!(world.get_mut("TX1").unwrap().is_departing());
        assert_eq!(world.get_mut("GT1").unwrap().state, State::Taxi);
        assert!(world.get_mut("GT1").unwrap().is_departing());
        assert!(world.is_parent_departing("SKY1-A"));

        // Tick the world until all departing aircraft (and the wingman) vanish over the horizon.
        for _ in 0..1200 {
            world.tick(0.1);
            if world.aircraft.is_empty() {
                break;
            }
        }
        assert!(
            world.aircraft.is_empty(),
            "all archived planes and wingman should vanish past horizon, left: {:?}",
            world.aircraft.iter().map(|a| &a.callsign).collect::<Vec<_>>()
        );
    }

    #[test]
    fn moving_from_flying_to_taxiway_does_not_trigger_farewell_departure() {
        let mut world = World::new();
        let mut callsigns = Callsigns::default();
        let mut taxiway_cs = TaxiwayCallsigns::default();
        let mut gates = GateCallsigns::default();
        let mut wingmen = Wingmen::default();
        let mut runway = RunwayOps::default();

        let poll1 = PollData {
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![(make_entry("t-1", "Busy Thread", "running"), 0)],
            taxiway: vec![],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
            now: 1_700_000_000,
        };
        apply_poll(
            &mut world,
            "omarchy",
            poll1,
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        assert!(world.get_mut("SKY1").is_some());

        // Thread finishes its turn and moves to taxiway (not marked done/archived).
        let poll2 = PollData {
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![],
            taxiway: vec![make_entry("t-1", "Busy Thread", "completed")],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
            now: 1_700_000_010,
        };
        apply_poll(
            &mut world,
            "omarchy",
            poll2,
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        // The same airframe flies an approach instead of vanishing.
        let sky_id = world.get_mut("SKY1").unwrap().id;
        assert_eq!(world.get_mut("SKY1").unwrap().state, State::Landing);
        assert!(world.get_mut("TX1").is_none(), "TX appears only once it has landed");

        let taxi_poll = || PollData {
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![],
            taxiway: vec![make_entry("t-1", "Busy Thread", "completed")],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
            now: 1_700_000_020,
        };
        let mut touched_down = false;
        for _ in 0..3000 {
            world.tick(0.05);
            let a = world.iter().find(|a| a.id == sky_id).unwrap();
            touched_down |= a.state == State::Landed;
            if a.state == State::Ready {
                break;
            }
        }
        assert!(touched_down, "should touch down on the runway");
        apply_poll(
            &mut world,
            "omarchy",
            taxi_poll(),
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        assert!(world.get_mut("SKY1").is_none());
        let tx = world.get_mut("TX1").unwrap();
        assert_eq!(tx.id, sky_id, "arrival hands over to the taxiway slot");
        assert_eq!(tx.state, State::Ready);
        assert!(!tx.is_departing());
    }

    #[test]
    fn waking_taxiway_thread_takes_off_with_the_same_airframe() {
        let mut world = World::new();
        let mut callsigns = Callsigns::default();
        let mut taxiway_cs = TaxiwayCallsigns::default();
        let mut gates = GateCallsigns::default();
        let mut wingmen = Wingmen::default();
        let mut runway = RunwayOps::default();

        let poll = |flying: bool| PollData {
            airports: vec![Airport::home("local", "omarchy")],
            flying: if flying {
                vec![(make_entry("t-1", "Thread", "running"), 0)]
            } else {
                vec![]
            },
            taxiway: if flying {
                vec![]
            } else {
                vec![make_entry("t-1", "Thread", "idle")]
            },
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
            now: 1_700_000_000,
        };
        apply_poll(&mut world, "omarchy", poll(false), &mut callsigns, &mut taxiway_cs, &mut gates, &mut wingmen, &mut runway);
        let tx_id = world.get_mut("TX1").unwrap().id;

        apply_poll(&mut world, "omarchy", poll(true), &mut callsigns, &mut taxiway_cs, &mut gates, &mut wingmen, &mut runway);
        assert!(world.get_mut("TX1").is_none());
        let sky = world.get_mut("SKY1").unwrap();
        assert_eq!(sky.id, tx_id);
        assert_eq!(sky.state, State::Taxi);

        let mut rolled = false;
        for _ in 0..4000 {
            world.tick(0.05);
            // Polls keep arriving mid-sequence; they must not cut the takeoff short.
            apply_poll(&mut world, "omarchy", poll(true), &mut callsigns, &mut taxiway_cs, &mut gates, &mut wingmen, &mut runway);
            let a = world.get_mut("SKY1").unwrap();
            rolled |= a.state == State::Takeoff && a.altitude <= 0.0;
            if a.state == State::Cruising && !a.in_runway_sequence() && a.altitude > 80.0 {
                break;
            }
        }
        let a = world.get_mut("SKY1").unwrap();
        assert!(rolled, "should roll down the runway");
        assert_eq!(a.state, State::Cruising);
        assert!(a.altitude > 80.0);
    }

    #[test]
    fn brand_new_thread_lines_up_on_the_runway_after_startup() {
        let mut world = World::new();
        let mut callsigns = Callsigns::default();
        let mut taxiway_cs = TaxiwayCallsigns::default();
        let mut gates = GateCallsigns::default();
        let mut wingmen = Wingmen::default();
        let mut runway = RunwayOps::default();
        let poll = |ids: &[&str]| PollData {
            airports: vec![Airport::home("local", "omarchy")],
            flying: ids.iter().map(|id| (make_entry(id, "Thread", "running"), 0)).collect(),
            taxiway: vec![],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
            now: 1_700_000_000,
        };
        // Already running at startup: appears in the air.
        apply_poll(&mut world, "omarchy", poll(&["t-1"]), &mut callsigns, &mut taxiway_cs, &mut gates, &mut wingmen, &mut runway);
        assert_eq!(world.get_mut("SKY1").unwrap().state, State::Cruising);
        // Starts later: lines up on Runway 36 for takeoff.
        apply_poll(&mut world, "omarchy", poll(&["t-1", "t-2"]), &mut callsigns, &mut taxiway_cs, &mut gates, &mut wingmen, &mut runway);
        let a = world.get_mut("SKY2").unwrap();
        assert_eq!(a.state, State::Ready);
        assert_eq!([a.position[0], a.position[1]], RUNWAY_THRESHOLD);
        assert!(a.in_runway_sequence());
    }
}
