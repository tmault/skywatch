//! T3 adapter: mirrors recent T3 threads, sub-agent wingmen, and linked
//! device airports.
//!
//! Enable with `SKYWATCH_T3=1` (any value except `0`/`false`). Polls the `t3`
//! CLI for linked devices and recent threads, maps run state to flight state,
//! parks stale threads (>24h) at their device airport's terminal gates, spawns
//! sub-agents as delta-wing fighter jets on their parent's wing, and relays
//! radio-panel messages back into the owning thread via `t3 thread send`.

use super::remote;
use crate::time::parse_rfc3339;
#[cfg(test)]
use crate::world::wrap_centered;
use crate::world::{
    default_sectors, formation_position, provider_info, Aircraft, Airport, ChatLinks, Kind,
    ModelClass, Sector, State, World, GATE_HEADING, HOME_GATES, PROJECT_SECTOR_RADIUS,
    PROJECT_SECTOR_SLOTS, RUNWAY_HEADING, RUNWAY_THRESHOLD, SATELLITE_SLOTS, TAXIWAY_QUEUE,
};
use crate::ws::{ClientFrame, SharedWorld};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
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
/// How many token-limited threads can hold on aerial refuelling tankers concurrently.
const MAX_REFUELING: usize = 8;
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

fn deserialize_opt_bool_ish<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    Ok(match val {
        Some(serde_json::Value::Bool(b)) => Some(b),
        Some(serde_json::Value::Number(n)) => Some(n.as_i64().unwrap_or(0) != 0),
        _ => None,
    })
}

#[derive(serde::Serialize, Deserialize)]
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
    #[serde(default, alias = "pendingRequestCount")]
    pending: i64,
    #[serde(default)]
    settled: bool,
    #[serde(rename = "settledAt", alias = "settled_at", default)]
    settled_at: Option<String>,
    #[serde(default, alias = "latestRunId")]
    latest_run_id: Option<String>,
    #[serde(default)]
    latest_run_provider: Option<String>,
    #[serde(default)]
    latest_run_model: Option<String>,
    #[serde(default, alias = "providerInstanceId")]
    latest_run_provider_instance: Option<String>,
    #[serde(default, alias = "parentThreadId")]
    parent_thread_id: Option<String>,
    #[serde(default, alias = "relationshipToParent")]
    relationship_to_parent: Option<String>,
    #[serde(default)]
    limit_recovery_run_id: Option<String>,
    #[serde(default)]
    limit_recovery_reset_at: Option<String>,
    #[serde(default, deserialize_with = "deserialize_opt_bool_ish")]
    limit_recovery_auto_resume: Option<bool>,
    #[serde(default)]
    usage_limit_reset_at: Option<String>,
    #[serde(default)]
    usage_limit_code: Option<String>,
    /// Set when another environment's thread delegated this one. T3's sidebar
    /// nests such threads under that parent (and its settled state) instead of
    /// listing them on their own; SQLite gives the parent id, the CLI an object.
    #[serde(default, alias = "delegatedFrom")]
    delegated_from: Option<serde_json::Value>,
}

impl ThreadListEntry {
    fn is_delegated(&self) -> bool {
        self.delegated_from.as_ref().is_some_and(|v| !v.is_null())
    }
}

/// A delegated thread only gets its own aircraft while it is doing work.
fn shown_on_its_own(e: &ThreadListEntry, parents_with_active_subagents: &HashSet<String>) -> bool {
    !e.is_delegated()
        || is_working_status(&e.status)
        || detect_token_wait(e).is_some()
        || parents_with_active_subagents.contains(&e.thread_id)
}

/// Active token/usage-limit wait detected for a T3 thread.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TokenWait {
    reason: String,
    reset_at: Option<String>,
    model_override: Option<String>,
    provider_hint: Option<String>,
    scope: String,
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
    chat_thread_id: Option<String>,
    #[serde(default)]
    environment_id: Option<String>,
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
    /// Sticky gate index per thread: a plane stays put when another leaves.
    slots: HashMap<String, usize>,
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

impl GateCallsigns {
    /// Give every parked thread (and every ground thread at a satellite airport,
    /// where taxiway-ready and stale threads share the 4 terminal gates) a sticky
    /// non-overlapping gate index that never changes while it stays on the ground.
    fn assign_ground_slots(
        &mut self,
        taxiway: &[ThreadListEntry],
        stale: &[ThreadListEntry],
        airports: &[Airport],
    ) {
        let sat_taxi: Vec<&ThreadListEntry> = taxiway
            .iter()
            .filter(|e| remote::owner(&e.thread_id).is_some())
            .collect();
        let ids: HashSet<&String> = sat_taxi
            .iter()
            .map(|e| &e.thread_id)
            .chain(stale.iter().map(|e| &e.thread_id))
            .collect();
        self.slots.retain(|id, _| ids.contains(id));

        for e in sat_taxi.iter().copied().chain(stale.iter()) {
            if self.slots.contains_key(&e.thread_id) {
                continue;
            }
            let env = remote::owner(&e.thread_id).map(|o| o.0);
            let capacity = env
                .as_ref()
                .and_then(|id| airports.iter().find(|ap| &ap.id == id))
                .map(|ap| ap.gates.len())
                .unwrap_or(HOME_GATES.len())
                .max(1);
            let used: HashSet<usize> = sat_taxi
                .iter()
                .copied()
                .chain(stale.iter())
                .filter(|o| remote::owner(&o.thread_id).map(|x| x.0) == env)
                .filter_map(|o| self.slots.get(&o.thread_id).copied())
                .collect();
            let slot = (0..capacity)
                .find(|s| !used.contains(s))
                .unwrap_or(self.slots.len() % capacity);
            self.slots.insert(e.thread_id.clone(), slot);
        }
    }

    #[cfg(test)]
    fn assign_slots(&mut self, entries: &[ThreadListEntry], airports: &[Airport]) {
        self.assign_ground_slots(&[], entries, airports);
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

/// Normalize the current run's structured T3 usage-limit error or armed recovery.
fn detect_token_wait(entry: &ThreadListEntry) -> Option<TokenWait> {
    if entry.pending > 0 || entry.settled || !matches!(entry.status.as_str(), "waiting" | "failed") {
        return None;
    }

    let recovery_armed = entry.limit_recovery_auto_resume == Some(true)
        && entry.limit_recovery_run_id.is_some()
        && entry.limit_recovery_run_id == entry.latest_run_id;
    if !recovery_armed && entry.usage_limit_code.is_none() && entry.usage_limit_reset_at.is_none() {
        return None;
    }
    let reset_at = if recovery_armed {
        entry.limit_recovery_reset_at.as_ref().or(entry.usage_limit_reset_at.as_ref())
    } else {
        entry.usage_limit_reset_at.as_ref()
    };
    let provider = entry.latest_run_provider.as_deref();
    let (environment, _) = remote::owner(&entry.thread_id)
        .unwrap_or_else(|| ("local".into(), entry.thread_id.clone()));
    // A direct Codex or Claude connection can share a tanker within one device.
    // An OpenCode connection can multiplex quota scopes, so it stays per thread.
    let scope = match (provider, entry.latest_run_provider_instance.as_deref()) {
        (Some(provider @ ("codex" | "claudeAgent")), Some(instance)) => {
            serde_json::to_string(&(environment, provider, instance)).expect("strings serialize")
        }
        _ => entry.thread_id.clone(),
    };
    Some(TokenWait {
        reason: match entry.usage_limit_code.as_deref() {
            Some("api_error_429" | "provider.rate-limit") => "Provider rate limit reached",
            Some("provider.quota") => "Provider quota reached",
            _ => "Provider usage limit reached",
        }.into(),
        reset_at: reset_at.cloned(),
        model_override: entry.latest_run_model.clone(),
        provider_hint: entry.latest_run_provider.clone(),
        scope,
    })
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

fn state_dir() -> Option<PathBuf> {
    let base = std::env::var_os("T3CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".t3")))?;
    Some(base.join("userdata"))
}

fn run_sqlite_json(sql: &str) -> Option<String> {
    let db_path = state_dir()?.join("statev2.sqlite");
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

    let sql = "SELECT s.subagent_id, s.thread_id, s.child_thread_id, t.thread_id AS chat_thread_id, s.status, s.updated_at, \
               COALESCE(t.title, json_extract(s.payload_json, '$.title'), json_extract(s.payload_json, '$.role'), 'sub-agent') AS title, \
               json_extract(t.payload_json, '$.modelSelection.model') AS model \
               FROM orchestration_v2_projection_subagents s \
               LEFT JOIN orchestration_v2_projection_threads t \
                   ON t.thread_id = s.child_thread_id AND t.deleted_at IS NULL \
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
    let active_sql = "SELECT t.thread_id, t.title, t.updated_at,
               p.title AS project,
               json_extract(t.payload_json, '$.modelSelection.model') AS model,
               r.run_id AS latest_run_id,
               COALESCE(r.status, 'idle') AS status,
               r.provider AS latest_run_provider,
               json_extract(r.payload_json, '$.providerInstanceId') AS latest_run_provider_instance,
               json_extract(r.payload_json, '$.modelSelection.model') AS latest_run_model,
               (SELECT COUNT(*) FROM orchestration_v2_projection_runtime_requests q
                WHERE q.thread_id = t.thread_id AND q.resolved_at IS NULL) AS pending,
               json_extract(t.payload_json, '$.limitRecovery.runId') AS limit_recovery_run_id,
               json_extract(t.payload_json, '$.limitRecovery.resetAt') AS limit_recovery_reset_at,
               json_extract(t.payload_json, '$.limitRecovery.autoResume') AS limit_recovery_auto_resume,
               CASE WHEN json_extract(i.payload_json, '$.failure.class') = 'usage_limit'
                    THEN json_extract(i.payload_json, '$.failure.resetAt')
               END AS usage_limit_reset_at,
               CASE WHEN json_extract(i.payload_json, '$.failure.class') = 'usage_limit'
                    THEN COALESCE(json_extract(i.payload_json, '$.failure.code'), '')
               END AS usage_limit_code,
               json_extract(t.payload_json, '$.delegatedFrom.threadId') AS delegated_from
               FROM orchestration_v2_projection_threads t
               LEFT JOIN projection_projects p ON p.project_id = t.project_id
               LEFT JOIN orchestration_v2_projection_runs r ON r.run_id = (
                   SELECT run_id FROM orchestration_v2_projection_runs
                   WHERE thread_id = t.thread_id
                   ORDER BY (completed_at IS NULL) DESC, ordinal DESC LIMIT 1)
               LEFT JOIN orchestration_v2_projection_turn_items i ON i.turn_item_id = (
                   SELECT turn_item_id FROM orchestration_v2_projection_turn_items
                   WHERE run_id = r.run_id AND type = 'error'
                   ORDER BY ordinal DESC LIMIT 1)
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
    omitted: usize,
    local_environment_id: Option<String>,
    airports: Vec<Airport>,
    /// Actively working or token-waiting threads (flying, holding, or refueling).
    flying: Vec<(ThreadListEntry, i64)>,
    /// Recent (<24h) non-running threads queued on Taxiway Alpha waiting for takeoff.
    taxiway: Vec<ThreadListEntry>,
    /// Stale (>24h untouched) threads parked at the extended terminal gates.
    stale: Vec<ThreadListEntry>,
    /// Thread IDs confirmed settled (marked done), archived, or deleted in T3.
    done_or_archived: HashSet<String>,
    subagents: Vec<SubagentRow>,
}

fn subagent_linger_secs() -> i64 {
    std::env::var("SKYWATCH_SUBAGENT_LINGER_SECS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(SUBAGENT_LINGER_SECS)
}

/// Separate a remote peer's threads into top-level active threads, settled/done
/// IDs, and active/lingering sub-agent wingmen (matching local SQLite filtering).
fn extract_peer_threads(
    peer: &remote::Peer,
    now: i64,
    linger: i64,
) -> (Vec<ThreadListEntry>, HashSet<String>, Vec<SubagentRow>) {
    let mut entries = Vec::new();
    let mut done_or_archived = HashSet::new();
    let mut subagents = Vec::new();
    for value in peer.threads.as_deref().unwrap_or(&[]) {
        let Ok(mut entry) = serde_json::from_value::<ThreadListEntry>(value.clone()) else {
            continue;
        };
        let is_subagent = entry
            .relationship_to_parent
            .as_deref()
            .is_some_and(|r| r.eq_ignore_ascii_case("subagent"));
        if is_subagent {
            if entry.settled || entry.settled_at.is_some() {
                continue;
            }
            let Some(parent_id) = entry.parent_thread_id.clone() else {
                continue;
            };
            let active = is_working_status(&entry.status);
            let recent = entry
                .updated_at
                .as_deref()
                .and_then(parse_rfc3339)
                .map(|t| now - t <= linger)
                .unwrap_or(false);
            if active || recent {
                let raw_id = entry.thread_id.clone();
                let keyed = remote::key(&peer.environment_id, &raw_id);
                subagents.push(SubagentRow {
                    subagent_id: keyed.clone(),
                    thread_id: remote::key(&peer.environment_id, &parent_id),
                    child_thread_id: Some(keyed),
                    chat_thread_id: Some(raw_id),
                    environment_id: Some(peer.environment_id.clone()),
                    status: entry.status,
                    updated_at: entry.updated_at,
                    title: Some(entry.title),
                    model: entry.model,
                });
            }
            continue;
        }
        entry.thread_id = remote::key(&peer.environment_id, &entry.thread_id);
        if entry.settled || entry.settled_at.is_some() {
            done_or_archived.insert(entry.thread_id);
        } else {
            entries.push(entry);
        }
    }
    (entries, done_or_archived, subagents)
}

/// Partition top-level threads into airborne, taxiway-ready, and gate-parked
/// pools respecting each airport's physical ground capacity.
fn partition_pools(
    entries: Vec<ThreadListEntry>,
    all_subagents: Vec<SubagentRow>,
    airports: &[Airport],
    now: i64,
) -> (
    Vec<(ThreadListEntry, i64)>,
    Vec<ThreadListEntry>,
    Vec<ThreadListEntry>,
    Vec<SubagentRow>,
    usize,
) {
    let parents_with_active_subagents: HashSet<String> = all_subagents
        .iter()
        .filter(|r| is_working_status(&r.status))
        .map(|r| r.thread_id.clone())
        .collect();
    // Delegated threads belong to the thread that delegated them (T3 nests them
    // under it in the sidebar), so they only appear while actually working —
    // never queued on the taxiway or parked at a gate on their own.
    let entries: Vec<ThreadListEntry> = entries
        .into_iter()
        .filter(|e| shown_on_its_own(e, &parents_with_active_subagents))
        .collect();
    let total_entries = entries.len();
    let mut flying = Vec::new();
    let mut working_count = HashMap::<String, usize>::new();
    let mut refuel_count = HashMap::<String, usize>::new();
    let mut taxiway = Vec::new();
    let mut stale = Vec::new();
    for e in entries {
        let env = remote::owner(&e.thread_id)
            .map(|o| o.0)
            .unwrap_or_else(|| "local".into());
        if detect_token_wait(&e).is_some() {
            if *refuel_count.entry(env.clone()).or_default() < MAX_REFUELING {
                let pending = e.pending;
                flying.push((e, pending));
                *refuel_count.entry(env).or_default() += 1;
            }
        } else if is_working_status(&e.status) || parents_with_active_subagents.contains(&e.thread_id)
        {
            if *working_count.entry(env.clone()).or_default() < MAX_AIRCRAFT {
                let mut e = e;
                if !is_working_status(&e.status) {
                    e.status = "running".to_string();
                }
                let pending = e.pending;
                flying.push((e, pending));
                *working_count.entry(env).or_default() += 1;
            }
        } else {
            let count_in = |v: &Vec<ThreadListEntry>| {
                v.iter()
                    .filter(|x| {
                        remote::owner(&x.thread_id)
                            .map(|o| o.0)
                            .unwrap_or_else(|| "local".into())
                            == env
                    })
                    .count()
            };
            if env == "local" {
                // Home island has 6 Taxiway Alpha slots + 10 terminal gates.
                // Overflow recent threads park at gates instead of vanishing.
                if !is_stale(&e.updated_at, now) && count_in(&taxiway) < MAX_TAXIWAY_QUEUE {
                    taxiway.push(e);
                } else if count_in(&stale) < MAX_GATE_PARKED {
                    stale.push(e);
                }
            } else {
                // Satellite islands share their terminal gates (4 stands) across
                // both recent ready threads and stale parked threads so no two
                // aircraft ever overlap on the same stand.
                let sat_cap = airports
                    .iter()
                    .find(|a| a.id == env)
                    .map(|a| a.gates.len())
                    .unwrap_or(4);
                if count_in(&taxiway) + count_in(&stale) < sat_cap {
                    if !is_stale(&e.updated_at, now) {
                        taxiway.push(e);
                    } else {
                        stale.push(e);
                    }
                }
            }
        }
    }

    let flying_ids: HashSet<String> = flying.iter().map(|(e, _)| e.thread_id.clone()).collect();
    let subagents: Vec<SubagentRow> = all_subagents
        .into_iter()
        .filter(|r| flying_ids.contains(&r.thread_id))
        .collect();
    let omitted = total_entries.saturating_sub(flying.len() + taxiway.len() + stale.len());
    (flying, taxiway, stale, subagents, omitted)
}

/// Fetch airports, threads, pending-request counts, and sub-agents off-lock.
fn fetch_poll_data(home_name: &str, reachability: &HashMap<String, bool>, cache: &mut remote::Cache) -> Option<PollData> {
    let local_environment_id = state_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("environment-id")).ok())
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty());
    let mut airports = discover_airports_fast(home_name, reachability);
    let local_poll = poll_threads();
    if let Some(home) = airports.first_mut() { home.online = local_poll.is_some(); }
    if let Some(ref result) = local_poll { cache.local = serde_json::to_value(result).ok(); }
    let (mut entries, mut done_or_archived) = local_poll.or_else(|| cache.local.as_ref().and_then(|v| serde_json::from_value(v.clone()).ok())).unwrap_or_default();
    let local_ids = entries.iter().map(|entry| entry.thread_id.clone()).collect();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let linger = subagent_linger_secs();
    let mut all_subagents = poll_subagents(&local_ids, now);

    let updates = remote::bridge(&serde_json::json!({"action":"poll"}))
        .and_then(|v| serde_json::from_value::<Vec<remote::Peer>>(v).ok()).unwrap_or_default();
    for peer in cache.merge(updates) {
        if let Some(ap) = airports.iter_mut().find(|a| a.id == peer.environment_id) {
            ap.online = peer.online;
        }
        let (peer_entries, peer_done, peer_subs) = extract_peer_threads(&peer, now, linger);
        entries.extend(peer_entries);
        done_or_archived.extend(peer_done);
        all_subagents.extend(peer_subs);
    }

    let (flying, taxiway, stale, subagents, omitted) =
        partition_pools(entries, all_subagents, &airports, now);

    Some(PollData {
        omitted,
        local_environment_id,
        airports,
        flying,
        taxiway,
        stale,
        done_or_archived,
        subagents,
    })
}

/// Move an already-placed ground aircraft to its newly assigned slot by
/// taxiing it along the painted routes — a poll must never copy its
/// coordinates (that is how queued planes used to teleport).
fn retarget_ground(a: &mut Aircraft, slot: [f32; 2], heading: f32, next: State) {
    // Airborne or mid runway sequence: the lifecycle owners handle it.
    if a.altitude > 0.5 || !matches!(a.state, State::Ready | State::Parked | State::Taxi) {
        return;
    }
    let cur = [a.position[0], a.position[1]];
    let at_slot = (cur[0] - slot[0]).hypot(cur[1] - slot[1]) < 3.0;
    let wants_this_slot = a.state == State::Taxi
        && a
            .taxi_destination()
            .is_some_and(|d| (d[0] - slot[0]).hypot(d[1] - slot[1]) < 1.0);
    if a.state == State::Taxi && wants_this_slot {
        // Already taxiing here: the simulation owns the motion; never restart it.
        return;
    }
    if at_slot {
        a.state = next;
        a.heading = heading;
        return;
    }
    a.taxi_to(slot, next);
}

fn airport_for_key<'a>(airports: &'a [Airport], key: &str) -> Option<&'a Airport> {
    if let Some((env, _)) = remote::owner(key) {
        airports.iter().find(|ap| ap.id == env)
    } else {
        airports.iter().find(|ap| ap.id == "local")
    }
}

fn taxiway_slot(
    entries: &[ThreadListEntry],
    i: usize,
    airports: &[Airport],
    gates: &GateCallsigns,
) -> ([f32; 2], f32) {
    let tid = &entries[i].thread_id;
    if let Some((env, _)) = remote::owner(tid) {
        if let Some(ap) = airports.iter().find(|ap| ap.id == env) {
            let slot = gates.slots.get(tid).copied().unwrap_or(0);
            return (ap.gates[slot % ap.gates.len()], GATE_HEADING);
        }
    }
    let local_idx = entries[..i]
        .iter()
        .filter(|e| remote::owner(&e.thread_id).is_none())
        .count();
    TAXIWAY_QUEUE[local_idx % TAXIWAY_QUEUE.len()]
}

fn slot_position(thread_id: &str, slot: usize, airports: &[Airport], taxi: bool) -> ([f32;2], f32) {
    let env = remote::owner(thread_id).map(|o| o.0);
    if let Some(ap) = env.and_then(|id| airports.iter().find(|ap| ap.id == id)) {
        return (ap.gates[slot % ap.gates.len()], GATE_HEADING);
    }
    if taxi { TAXIWAY_QUEUE[slot % TAXIWAY_QUEUE.len()] } else { (HOME_GATES[slot % HOME_GATES.len()], GATE_HEADING) }
}

struct KeepInfo {
    state: State,
    reason: Option<String>,
    title: Option<String>,
    model_class: ModelClass,
    model: Option<String>,
    provider: Option<String>,
    reset_at: Option<String>,
    refuel_scope: Option<String>,
    zone: String,
}

fn conversation_links(environment: &str, thread: &str) -> Option<ChatLinks> {
    // Match T3's hosted link encoding. In particular, percent escapes already
    // in a T3 thread ID are literal and must survive one URL decode.
    const THREAD_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_');
    if [environment, thread].iter().any(|id| matches!(*id, "" | "." | "..")) {
        return None;
    }
    let environment = utf8_percent_encode(environment, THREAD_SEGMENT);
    let thread = utf8_percent_encode(thread, THREAD_SEGMENT);
    // T3's Copy thread Web route and desktop promptLinkAppPath handler.
    Some(ChatLinks {
        web: format!("https://app.t3.codes/threads/{environment}/{thread}"),
        desktop: format!("t3code://threads/{environment}/{thread}"),
    })
}

fn bind_conversation(
    aircraft: &mut Aircraft,
    airport: Option<&Airport>,
    environment: Option<&str>,
    thread: Option<&str>,
) {
    aircraft.environment_id = environment.map(str::to_owned);
    aircraft.thread_id = thread.map(str::to_owned);
    aircraft.data_stale = !airport.is_some_and(|airport| airport.online);
    if let Some(airport) = airport {
        aircraft.device = Some(airport.name.clone());
    }
    aircraft.chat_links = if aircraft.data_stale {
        None
    } else {
        environment
            .zip(thread)
            .and_then(|(env, id)| conversation_links(env, id))
    };
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
    world.omitted_threads = data.omitted;
    world.airports = data.airports;
    // Assign sticky non-overlapping gate slots across all satellite ground
    // aircraft (both recent ready and stale parked) plus local stale gates.
    gates.assign_ground_slots(&data.taxiway, &data.stale, &world.airports);

    let mut keep: HashMap<String, KeepInfo> = HashMap::new();
    let mut new_threads: Vec<(String, String, ModelClass)> = Vec::new();
    let mut active_projects: Vec<String> = Vec::new();

    for (entry, pending) in &data.flying {
        let token_wait = detect_token_wait(entry);
        let (state, reason, reset_at, eff_model, prov_hint, refuel_scope) = if let Some(tw) = token_wait {
            (
                State::Refueling,
                Some(tw.reason),
                tw.reset_at,
                tw.model_override.or_else(|| entry.model.clone()),
                tw.provider_hint,
                Some(tw.scope),
            )
        } else {
            let (st, r) = flight_state(&entry.status, *pending);
            (
                st,
                r,
                None,
                entry.model.clone(),
                entry.latest_run_provider.clone(),
                None,
            )
        };
        let (driver_key, _, _, _) = provider_info(None, prov_hint.as_deref());
        let (model_key, _, _, _) = provider_info(eff_model.as_deref(), prov_hint.as_deref());
        let prov_key = if refuel_scope.is_some() && driver_key != "generic" {
            driver_key
        } else {
            model_key
        };
        let mc = model_class(&eff_model);
        let (zone, is_project) = classify_zone(entry.project.as_deref(), &entry.title);
        if is_project && !active_projects.contains(&zone) {
            active_projects.push(zone.clone());
        }
        keep.insert(
            entry.thread_id.clone(),
            KeepInfo {
                state,
                reason,
                title: Some(entry.title.clone()),
                model_class: mc,
                model: eff_model,
                provider: Some(prov_key.to_string()),
                reset_at,
                refuel_scope,
                zone,
            },
        );
        if callsigns.by_thread.contains_key(&entry.thread_id) {
            continue;
        }
        new_threads.push((entry.thread_id.clone(), entry.title.clone(), mc));
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
            let owning_ap = airport_for_key(&world.airports, &id).cloned();
            if let Some(ap) = &owning_ap {
                if let Some(a) = world.get_mut(&cs) {
                    a.set_airport(ap);
                }
            }
            if !all_active_ids.contains(&id) && data.done_or_archived.contains(&id) {
                world.depart_by_callsign(&cs);
                radio_events.push((cs, "thread done — departing into the horizon".to_string()));
            } else if runway.primed && taxi_ids.contains(&id) {
                let arrival_slot = owning_ap
                    .as_ref()
                    .filter(|ap| !ap.is_home && !ap.gates.is_empty())
                    .map(|ap| {
                        let idx = gates.slots.get(&id).copied().unwrap_or(0);
                        (ap.gates[idx % ap.gates.len()], GATE_HEADING)
                    })
                    .unwrap_or(TAXIWAY_QUEUE[0]);
                if world
                    .get_mut(&cs)
                    .map(|a| a.begin_approach(Some(arrival_slot)))
                    .unwrap_or(false)
                {
                    // Turn finished: fly the approach, land and backtrack to the owning airport's slot.
                    runway.arrivals.insert(id, cs.clone());
                    radio_events.push((cs, "on approach, Runway 36".to_string()));
                } else {
                    world.remove_by_callsign(&cs);
                }
            } else {
                world.remove_by_callsign(&cs);
            }
        }
    }

    let mut order: Vec<String> = callsigns.by_thread.keys().cloned().collect();
    order.extend(new_threads.iter().map(|(id, _, _)| id.clone()));

    for thread_id in &order {
        let Some(info) = keep.get(thread_id) else {
            continue;
        };
        let cs = callsigns.get(thread_id);
        let owning_ap = airport_for_key(&world.airports, thread_id).cloned();
        if let Some(a) = world.get_mut(&cs) {
            if let Some(ap) = &owning_ap {
                a.set_airport(ap);
            }
            // Let takeoffs and go-arounds finish; the steady state applies after.
            if !a.in_runway_sequence() {
                if a.state != info.state {
                    radio_events.push((
                        cs.clone(),
                        match info.state {
                            State::Holding => "entering hold".to_string(),
                            State::Refueling => "connecting to aerial tanker — waiting for tokens".to_string(),
                            State::Cruising => "cruising".to_string(),
                            _ => "updating".to_string(),
                        },
                    ));
                }
                if info.state == State::Refueling {
                    a.refuel(
                        info.reason
                            .as_deref()
                            .unwrap_or("usage limit reached — waiting for tokens"),
                        info.reset_at.clone(),
                    );
                } else {
                    if a.state == State::Refueling && info.state == State::Cruising {
                        a.release();
                    }
                    a.state = info.state;
                    a.refuel_reset_at = None;
                }
            }
            a.hold_reason = info.reason.clone();
            a.title = info.title.clone();
            a.topic = Some(info.zone.clone());
            a.model = info.model.clone();
            a.model_class = info.model_class;
            a.provider = info.provider.clone();
            a.refuel_scope = info.refuel_scope.clone();
            a.device = Some(
                owning_ap
                    .as_ref()
                    .map(|ap| ap.name.clone())
                    .unwrap_or_else(|| home_name.to_string()),
            );
        } else if let Some((_, t, mc)) = new_threads.iter().find(|(id, _, _)| id == thread_id) {
            // Woken from the taxiway queue or a gate: the same airframe taxis
            // out and takes off from its owning airport instead of popping into the sky.
            let ground_cs = taxiway_cs
                .by_thread
                .get(thread_id)
                .or_else(|| gates.by_thread.get(thread_id))
                .cloned();
            if let Some(gcs) = ground_cs {
                if world.rename(&gcs, &cs) {
                    if let Some(a) = world.get_mut(&cs) {
                        if let Some(ap) = &owning_ap {
                            a.set_airport(ap);
                        }
                        a.title = info.title.clone();
                        a.model = info.model.clone();
                        a.model_class = *mc;
                        a.provider = info.provider.clone();
                        a.refuel_scope = info.refuel_scope.clone();
                        a.assign(&info.zone);
                        a.release();
                        a.hold_reason = info.reason.clone();
                        a.refuel_reset_at = info.reset_at.clone();
                    }
                    radio_events.push((cs.clone(), "cleared for takeoff, Runway 36".to_string()));
                    continue;
                }
            }
            let mut a = Aircraft::with_model(&cs, Kind::Agent, *mc);
            if let Some(ap) = &owning_ap {
                a.set_airport(ap);
            }
            a.title = info.title.clone().or_else(|| Some(t.clone()));
            a.model = info.model.clone();
            a.provider = info.provider.clone();
            a.refuel_scope = info.refuel_scope.clone();
            a.device = Some(
                owning_ap
                    .as_ref()
                    .map(|ap| ap.name.clone())
                    .unwrap_or_else(|| home_name.to_string()),
            );
            a.hold_reason = info.reason.clone();
            a.refuel_reset_at = info.reset_at.clone();
            a.assign(&info.zone);
            a.state = info.state;
            let a_reason = a.hold_reason.clone();
            let a_reset = a.refuel_reset_at.clone();
            if runway.primed && info.state != State::Refueling {
                // Brand-new thread: line up on the owning airport's runway and take off.
                let thresh = owning_ap
                    .as_ref()
                    .map(|ap| ap.runway_layout().threshold)
                    .unwrap_or(RUNWAY_THRESHOLD);
                a.position = [thresh[0], thresh[1], 0.0];
                a.altitude = 0.0;
                a.heading = RUNWAY_HEADING;
                a.state = State::Ready;
                a.release();
                a.hold_reason = a_reason;
                a.refuel_reset_at = a_reset;
                radio_events.push((cs.clone(), "lined up, Runway 36".to_string()));
            }
            world.add(a);
        }
    }

    // --- Taxiway Alpha / Satellite Stand Departure Queue: recent (<24h) non-working threads ---
    let taxi_drop: Vec<String> = taxiway_cs
        .by_thread
        .keys()
        .filter(|id| !taxi_ids.contains(*id))
        .cloned()
        .collect();
    for id in taxi_drop {
        if let Some(cs) = taxiway_cs.by_thread.remove(&id) {
            if let Some(ap) = airport_for_key(&world.airports, &id).cloned() {
                if let Some(a) = world.get_mut(&cs) {
                    a.set_airport(&ap);
                }
            }
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
        let owning_ap = airport_for_key(&world.airports, &entry.thread_id).cloned();
        let (slot_pos, slot_hdg) = taxiway_slot(&data.taxiway, i, &world.airports, gates);
        let (_, reason) = flight_state(&entry.status, entry.pending);
        let (zone, _) = classify_zone(entry.project.as_deref(), &entry.title);
        if let Some(a) = world.get_mut(&cs) {
            if let Some(ap) = &owning_ap {
                a.set_airport(ap);
            }
            a.title = Some(entry.title.clone());
            a.hold_reason = reason;
            a.topic = Some(zone);
            a.model = entry.model.clone();
            a.device = Some(
                owning_ap
                    .as_ref()
                    .map(|ap| ap.name.clone())
                    .unwrap_or_else(|| home_name.to_string()),
            );
            // Queued threads shuffle to their new slot by taxi, not by teleport.
            retarget_ground(a, slot_pos, slot_hdg, State::Ready);
        } else {
            let mut a = Aircraft::with_model(&cs, Kind::Agent, model_class(&entry.model));
            if let Some(ap) = &owning_ap {
                a.set_airport(ap);
            }
            a.title = Some(entry.title.clone());
            a.model = entry.model.clone();
            a.hold_reason = reason;
            a.topic = Some(zone);
            a.state = State::Ready;
            a.position = [slot_pos[0], slot_pos[1], 0.0];
            a.altitude = 0.0;
            a.heading = slot_hdg;
            a.device = Some(
                owning_ap
                    .as_ref()
                    .map(|ap| ap.name.clone())
                    .unwrap_or_else(|| home_name.to_string()),
            );
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

    // --- Extended Terminal Gates: threads parked at gates (>24h or home overflow) ---
    let stale_ids: HashSet<String> = data.stale.iter().map(|e| e.thread_id.clone()).collect();
    let gate_drop: Vec<String> = gates
        .by_thread
        .keys()
        .filter(|id| !stale_ids.contains(*id))
        .cloned()
        .collect();
    for id in gate_drop {
        if let Some(cs) = gates.by_thread.remove(&id) {
            if let Some(ap) = airport_for_key(&world.airports, &id).cloned() {
                if let Some(a) = world.get_mut(&cs) {
                    a.set_airport(&ap);
                }
            }
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

    for entry in data.stale.iter() {
        let cs = gates.get(&entry.thread_id);
        let gate_index = gates.slots.get(&entry.thread_id).copied().unwrap_or(0);
        let owning_ap = airport_for_key(&world.airports, &entry.thread_id).cloned();
        let (slot, _) = slot_position(&entry.thread_id, gate_index, &world.airports, false);
        if let Some(a) = world.get_mut(&cs) {
            if let Some(ap) = &owning_ap {
                a.set_airport(ap);
            }
            a.title = Some(entry.title.clone());
            a.model = entry.model.clone();
            a.device = Some(
                owning_ap
                    .as_ref()
                    .map(|ap| ap.name.clone())
                    .unwrap_or_else(|| home_name.to_string()),
            );
            // Stale threads shuffle between gates by taxi, not by teleport.
            retarget_ground(a, slot, GATE_HEADING, State::Parked);
        } else {
            let mut a = Aircraft::with_model(&cs, Kind::Agent, model_class(&entry.model));
            if let Some(ap) = &owning_ap {
                a.set_airport(ap);
            }
            a.title = Some(entry.title.clone());
            a.model = entry.model.clone();
            a.state = State::Parked;
            a.position = [slot[0], slot[1], 0.0];
            a.altitude = 0.0;
            a.heading = GATE_HEADING;
            a.topic = None;
            a.device = Some(
                owning_ap
                    .as_ref()
                    .map(|ap| ap.name.clone())
                    .unwrap_or_else(|| home_name.to_string()),
            );
            world.add(a);
            radio_events.push((cs.clone(), format!("parked at gate {}", gate_index + 1)));
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

    let previous_wingmen = std::mem::take(&mut wingmen.by_subagent);
    let mut child_chats: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
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
        let parent_ap = airport_for_key(&world.airports, &parent_tid).cloned();
        let on_ground = matches!(
            p_state,
            State::Parked | State::Landed | State::Taxi | State::Ready
        );

        for (slot, sub) in subs.into_iter().enumerate() {
            let relay_tid = sub
                .child_thread_id
                .clone()
                .unwrap_or_else(|| parent_tid.clone());
            let cs = format!("{parent_cs}-{}", suffixes[slot % suffixes.len()]);
            let sub_env = sub
                .environment_id
                .clone()
                .or_else(|| remote::owner(&parent_tid).map(|o| o.0));
            child_chats.insert(cs.clone(), (sub_env, sub.chat_thread_id));
            wingmen
                .by_subagent
                .insert(sub.subagent_id.clone(), (cs.clone(), relay_tid));

            // Wingmen wear their own model's livery, falling back to the parent's.
            let model = sub.model.clone().or_else(|| p_model.clone());
            if let Some(a) = world.get_mut(&cs) {
                if let Some(ap) = &parent_ap {
                    a.set_airport(ap);
                }
                a.parent_id = Some(parent_id);
                a.wing_slot = Some(slot);
                a.title = sub.title.clone();
                a.model = model;
                a.device = Some(
                    parent_ap
                        .as_ref()
                        .map(|ap| ap.name.clone())
                        .unwrap_or_else(|| home_name.to_string()),
                );
            } else {
                let start = formation_position(p_pos, p_heading, on_ground, slot);
                let mut fighter = Aircraft::wingman(&cs, parent_id, slot, start, p_heading);
                if let Some(ap) = &parent_ap {
                    fighter.set_airport(ap);
                }
                fighter.title = sub.title.clone();
                fighter.model = model;
                fighter.device = Some(
                    parent_ap
                        .as_ref()
                        .map(|ap| ap.name.clone())
                        .unwrap_or_else(|| home_name.to_string()),
                );
                world.add(fighter);
                radio_events.push((cs, format!("scrambled on {parent_cs}'s wing")));
            }
        }
    }

    // Remove fighters whose sub-agents have finished and lingered out.
    for (cs, _) in previous_wingmen.into_values() {
        // A different child can occupy this callsign when the formation changes.
        // Departing wingmen keep escorting their parent until despawn.
        if !child_chats.contains_key(&cs) && !world.is_parent_departing(&cs) {
            world.remove_by_callsign(&cs);
            radio_events.push((cs, "breaking formation — RTB".to_string()));
        }
    }

    // Ownership is independent of callsign changes during landing/taxi handoff.
    let owners: HashMap<_, _> = callsigns
        .by_thread
        .iter()
        .chain(&taxiway_cs.by_thread)
        .chain(&gates.by_thread)
        .chain(&runway.arrivals)
        .map(|(key, callsign)| (callsign.as_str(), key.as_str()))
        .collect();
    let airport_for = |id: &str| world.airports.iter().find(|airport| airport.id == id);
    let local_airport = airport_for("local");
    let local_environment = data.local_environment_id.as_deref();
    for aircraft in &mut world.aircraft {
        if let Some(key) = owners.get(aircraft.callsign.as_str()) {
            if let Some((env, thread)) = remote::owner(key) {
                bind_conversation(aircraft, airport_for(&env), Some(&env), Some(&thread));
            } else {
                bind_conversation(aircraft, local_airport, local_environment, Some(key));
            }
        } else if let Some((sub_env, child)) = child_chats.get(&aircraft.callsign) {
            if let Some(env) = sub_env.as_deref() {
                bind_conversation(aircraft, airport_for(env), Some(env), child.as_deref());
            } else {
                bind_conversation(aircraft, local_airport, local_environment, child.as_deref());
            }
        } else {
            // A departing thread can have been deleted. Keep its reference only.
            aircraft.chat_links = None;
        }
    }

    world.sync_tankers();

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
            let mut cache = remote::Cache::default();
            loop {
                let hn = home_clone.clone();
                let snap_reach = reach.read().map(|g| g.clone()).unwrap_or_default();
                let res = tokio::task::spawn_blocking(move || {
                    let data = fetch_poll_data(&hn, &snap_reach, &mut cache);
                    (data, cache)
                }).await;
                let res = match res { Ok((data, next_cache)) => { cache = next_cache; Ok(data) }, Err(e) => { cache = remote::Cache::default(); Err(e) } };
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
                    let response = if let Some((environment, thread)) = remote::owner(&thread_id) {
                        remote::bridge(&serde_json::json!({"action":"send", "arguments": {"environmentId":environment,"threadId":thread,"message":text}})).map(|v| v.to_string())
                    } else { run_t3(&["thread", "send", &thread_id, &text]) };
                    if let Some(out) = response {
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
    fn conversation_links_preserve_literal_path_segments() {
        let links = conversation_links("device-1", "child%2Fname/one?#").unwrap();
        assert_eq!(
            links.web,
            "https://app.t3.codes/threads/device-1/child%252Fname%2Fone%3F%23"
        );
        assert_eq!(
            links.desktop,
            "t3code://threads/device-1/child%252Fname%2Fone%3F%23"
        );
        assert!(conversation_links("", "child").is_none());
        assert!(conversation_links("device-1", "..").is_none());
    }

    #[test]
    fn remote_thread_ids_do_not_alias_local_ids_and_keep_device_ownership() {
        let mut world = World::new();
        let mut sky = Callsigns::default();
        let mut taxi = TaxiwayCallsigns::default();
        let mut gates = GateCallsigns::default();
        let mut wing = Wingmen::default();
        let mut runway = RunwayOps::default();
        let poll = PollData {
            omitted: 3,
            local_environment_id: Some("pc".into()),
            airports: vec![
                Airport::home("local", "PC"),
                Airport::satellite("mac", "Mac", [400.0, 0.0], false),
                Airport::satellite("tower", "Tower", [800.0, 0.0], true),
            ],
            flying: vec![
                (make_entry("same", "Local", "running"), 0),
                (
                    make_entry(&remote::key("mac", "same"), "Remote", "running"),
                    0,
                ),
                // The remote list already includes real child threads as distinct entries.
                (
                    make_entry(&remote::key("tower", "child"), "Remote child", "running"),
                    0,
                ),
            ],
            taxiway: vec![],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![
                SubagentRow {
                    subagent_id: "sub-1".into(),
                    thread_id: "same".into(),
                    child_thread_id: Some("child".into()),
                    chat_thread_id: Some("child".into()),
                    environment_id: None,
                    status: "running".into(),
                    updated_at: None,
                    title: Some("Local wingman".into()),
                    model: None,
                },
                SubagentRow {
                    subagent_id: "sub-2".into(),
                    thread_id: "same".into(),
                    child_thread_id: None,
                    chat_thread_id: None,
                    environment_id: None,
                    status: "running".into(),
                    updated_at: None,
                    title: Some("Wingman without chat".into()),
                    model: None,
                },
            ],
        };
        apply_poll(
            &mut world,
            "PC",
            poll,
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );
        let snapshot = world.snapshot();
        let by_title = |title| {
            snapshot
                .aircraft
                .iter()
                .find(|a| a.title.as_deref() == Some(title))
                .unwrap()
        };
        let local = by_title("Local");
        let remote = by_title("Remote");
        assert_ne!(local.id, remote.id);
        assert_eq!(local.environment_id.as_deref(), Some("pc"));
        assert_eq!(
            local.chat_links.as_ref().unwrap().web,
            "https://app.t3.codes/threads/pc/same"
        );
        assert_eq!(remote.environment_id.as_deref(), Some("mac"));
        assert_eq!(remote.thread_id.as_deref(), Some("same"));
        assert_eq!(remote.device.as_deref(), Some("Mac"));
        assert!(remote.data_stale);
        assert!(remote.chat_links.is_none());
        let remote_child = by_title("Remote child");
        assert_eq!(
            remote_child.chat_links.as_ref().unwrap().desktop,
            "t3code://threads/tower/child"
        );
        let local_child = by_title("Local wingman");
        assert_eq!(
            local_child.chat_links.as_ref().unwrap().desktop,
            "t3code://threads/pc/child"
        );
        let no_chat = by_title("Wingman without chat");
        assert!(no_chat.thread_id.is_none() && no_chat.chat_links.is_none());
        assert_eq!(snapshot.omitted_threads, 3);
    }

    #[test]
    fn parses_rfc3339_to_unix_secs() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-02T00:00:00Z"), Some(86400));
        assert!(parse_rfc3339("2026-11-01T17:39:45.542Z").is_some());
        assert_eq!(parse_rfc3339("garbage"), None);
    }

    #[test]
    fn idle_delegated_threads_do_not_queue_on_their_own() {
        let none = HashSet::new();
        let mut child = make_entry("mcp:launch:x", "Delegated", "completed");
        child.delegated_from = Some(serde_json::json!("parent-thread"));
        assert!(!shown_on_its_own(&child, &none), "idle delegated thread must not queue");
        child.status = "running".into();
        assert!(shown_on_its_own(&child, &none), "working delegated thread still flies");
        let own = make_entry("t-own", "Own", "completed");
        assert!(shown_on_its_own(&own, &none), "ordinary idle threads still queue");
        let mut cli = make_entry("t-cli", "CLI", "idle");
        cli.delegated_from = Some(serde_json::json!({"environmentId": "e", "threadId": "p"}));
        assert!(!shown_on_its_own(&cli, &none));
    }

    #[test]
    fn gate_slots_stay_put_when_another_plane_leaves() {
        let mut gates = GateCallsigns::default();
        let e = |id: &str| make_entry(id, id, "idle");
        gates.assign_slots(&[e("a"), e("b"), e("c")], &[]);
        let before: Vec<usize> = ["a", "b", "c"].iter().map(|k| gates.slots[*k]).collect();
        assert_eq!(before, vec![0, 1, 2]);
        // "a" leaves; b and c keep their gates, and a newcomer takes the freed one.
        gates.assign_slots(&[e("b"), e("c")], &[]);
        assert_eq!((gates.slots["b"], gates.slots["c"]), (1, 2));
        gates.assign_slots(&[e("d"), e("b"), e("c")], &[]);
        assert_eq!(gates.slots["d"], 0);
        assert_eq!((gates.slots["b"], gates.slots["c"]), (1, 2));
    }

    #[test]
    fn staleness_boundary_is_24_hours() {
        let now = parse_rfc3339("2026-11-24T00:00:00Z").unwrap();
        assert!(!is_stale(&Some("2026-11-23T00:00:01Z".into()), now));
        assert!(is_stale(&Some("2026-11-23T00:00:00Z".into()), now));
        assert!(is_stale(&Some("2026-11-22T00:00:00Z".into()), now));
        assert!(!is_stale(&None, now));
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
            latest_run_id: Some(format!("run:{id}:1")),
            latest_run_provider: Some("claudeAgent".to_string()),
            latest_run_model: Some("claude-opus-5-5".to_string()),
            latest_run_provider_instance: None,
            parent_thread_id: None,
            relationship_to_parent: None,
            limit_recovery_run_id: None,
            limit_recovery_reset_at: None,
            limit_recovery_auto_resume: None,
            usage_limit_reset_at: None,
            delegated_from: None,
            usage_limit_code: None,
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
            omitted: 0,
            local_environment_id: None,
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![(make_entry("t-fly", "Active Flight", "running"), 0)],
            taxiway: vec![make_entry("t-taxi", "Waiting Thread", "idle")],
            stale: vec![make_entry("t-gate", "Old Thread", "idle")],
            done_or_archived: HashSet::new(),
            subagents: vec![SubagentRow {
                subagent_id: "sub-1".to_string(),
                thread_id: "t-fly".to_string(),
                child_thread_id: None,
                chat_thread_id: None,
                environment_id: None,
                status: "running".to_string(),
                updated_at: None,
                title: Some("Scout".to_string()),
                model: None,
            }],
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
            omitted: 0,
            local_environment_id: None,
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![],
            taxiway: vec![],
            stale: vec![],
            done_or_archived: done,
            subagents: vec![],
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
            omitted: 0,
            local_environment_id: None,
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![(make_entry("t-1", "Busy Thread", "running"), 0)],
            taxiway: vec![],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
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
            omitted: 0,
            local_environment_id: None,
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![],
            taxiway: vec![make_entry("t-1", "Busy Thread", "completed")],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
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
            omitted: 0,
            local_environment_id: None,
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![],
            taxiway: vec![make_entry("t-1", "Busy Thread", "completed")],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
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
            omitted: 0,
            local_environment_id: None,
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
    fn repeated_polls_during_arrival_do_not_reset_or_duplicate_landing() {
        let mut world = World::new();
        let mut callsigns = Callsigns::default();
        let mut taxiway_cs = TaxiwayCallsigns::default();
        let mut gates = GateCallsigns::default();
        let mut wingmen = Wingmen::default();
        let mut runway = RunwayOps::default();

        let poll = |running: bool| PollData {
            omitted: 0,
            local_environment_id: None,
            airports: vec![Airport::home("local", "omarchy")],
            flying: if running {
                vec![(make_entry("t-1", "Thread", "running"), 0)]
            } else {
                vec![]
            },
            taxiway: if running {
                vec![]
            } else {
                vec![make_entry("t-1", "Thread", "idle")]
            },
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
        };
        // Running at startup: appears in the air.
        apply_poll(&mut world, "omarchy", poll(true), &mut callsigns, &mut taxiway_cs, &mut gates, &mut wingmen, &mut runway);
        assert_eq!(world.get_mut("SKY1").unwrap().state, State::Cruising);
        // Thread stops: the aircraft turns into an arrival.
        apply_poll(&mut world, "omarchy", poll(false), &mut callsigns, &mut taxiway_cs, &mut gates, &mut wingmen, &mut runway);
        assert_eq!(world.get_mut("SKY1").unwrap().state, State::Landing);
        // Track by id: when the arrival vacates the runway it is renamed to
        // its holding-short callsign (TX1) — identity must survive that.
        let a_id = world.get_mut("SKY1").unwrap().id;

        // Identical polls keep arriving through the whole approach, rollout
        // and backtrack: identity persists, motion stays bounded, and the
        // touchdown happens exactly once.
        let mut touchdowns = 0;
        let mut prev_state = State::Landing;
        let mut prev_pos = world.get_mut("SKY1").unwrap().position;
        for step in 0..4000 {
            world.tick(0.05);
            apply_poll(&mut world, "omarchy", poll(false), &mut callsigns, &mut taxiway_cs, &mut gates, &mut wingmen, &mut runway);
            let a = world.aircraft.iter().find(|a| a.id == a_id).unwrap();
            if a.state == State::Landed && prev_state != State::Landed {
                touchdowns += 1;
            }
            // Torus-aware: a legitimate boundary crossing must not read as a
            // teleport, while any real snap stays larger than the bound.
            let d = wrap_centered(a.position[0] - prev_pos[0])
                .hypot(wrap_centered(a.position[1] - prev_pos[1]));
            assert!(d < 3.0, "unbounded per-tick motion at {step}: {d}");
            prev_state = a.state;
            prev_pos = a.position;
            if matches!(a.state, State::Ready | State::Parked) && a.speed < 0.5 {
                break;
            }
        }
        let a = world.aircraft.iter().find(|a| a.id == a_id).unwrap();
        assert!(
            matches!(a.state, State::Ready | State::Parked),
            "arrival never finished: {:?}",
            a.state
        );
        assert_eq!(touchdowns, 1, "expected exactly one touchdown");
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
            omitted: 0,
            local_environment_id: None,
            airports: vec![Airport::home("local", "omarchy")],
            flying: ids.iter().map(|id| (make_entry(id, "Thread", "running"), 0)).collect(),
            taxiway: vec![],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
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

    #[test]
    fn only_current_structured_limit_waits_use_a_tanker() {
        let fixtures: Vec<ThreadListEntry> = serde_json::from_str(include_str!(
            "../../tests/fixtures/provider-waits.json"
        )).unwrap();
        let waits: Vec<_> = fixtures.iter().map(|entry| detect_token_wait(entry)).collect();
        assert_eq!(waits[0].as_ref().unwrap().reset_at.as_deref(), Some("2026-11-24T03:22:09.000Z"));
        let open_code = waits[1].as_ref().unwrap();
        assert_eq!(open_code.reset_at, None, "relative prose stays unknown");
        assert_eq!(open_code.reason, "Provider quota reached");
        assert_eq!(waits[2].as_ref().unwrap().reset_at.as_deref(), Some("2026-11-24T06:00:00.000Z"));
        assert!(waits[3..].iter().all(Option::is_none));
    }

    #[test]
    fn tanker_identity_survives_poll_updates_and_receiver_ownership_survives_disconnect() {
        let mut world = World::new();
        let (mut sky, mut taxi, mut gates, mut wing, mut runway) = (
            Callsigns::default(),
            TaxiwayCallsigns::default(),
            GateCallsigns::default(),
            Wingmen::default(),
            RunwayOps::default(),
        );
        let receiver = remote::key("mac", "shared-id");
        let mut poll = |online, reset: &str, settled: bool| {
            let wait = |id: &str, connection: &str| {
                let mut entry = make_entry(id, id, "failed");
                entry.latest_run_provider = Some("claudeAgent".into());
                entry.latest_run_provider_instance = Some(connection.into());
                entry.usage_limit_reset_at = Some(reset.into());
                (entry, 0)
            };
            let mut flying = vec![wait("local-a", "connection-1"), wait("local-b", "connection-1"),
                wait("local-c", "connection-2")];
            if !settled { flying.push(wait(&receiver, "connection-1")); }
            let data = PollData {
                local_environment_id: None,
                airports: vec![Airport::home("local", "PC"), Airport::satellite("mac", "Mac", [400.0, 0.0], online)],
                omitted: 0,
                flying,
                taxiway: vec![],
                stale: vec![],
                done_or_archived: if settled { HashSet::from([receiver.clone()]) } else { HashSet::new() },
                subagents: vec![],
            };
            apply_poll(&mut world, "PC", data, &mut sky, &mut taxi, &mut gates, &mut wing, &mut runway);
            world.snapshot().aircraft
        };
        let initial = poll(true, "2026-11-24T02:00:00Z", false);
        let get = |fleet: &[Aircraft], callsign: &str| {
            fleet.iter().find(|a| a.callsign == callsign).unwrap().clone()
        };
        let remote = get(&initial, "SKY4");
        let first = get(&initial, "SKY1");
        assert_eq!(first.tanker_id, get(&initial, "SKY2").tanker_id);
        assert_ne!(first.tanker_id, get(&initial, "SKY3").tanker_id);
        assert_ne!(first.tanker_id, remote.tanker_id);
        assert!(remote.tanker_id.is_some());

        let extended = poll(true, "2026-11-24T06:00:00Z", false);
        let updated = get(&extended, "SKY4");
        assert_eq!(updated.tanker_id, remote.tanker_id);
        assert_eq!(updated.refuel_slot, remote.refuel_slot);
        assert_eq!(updated.position, remote.position, "a metadata update must not move the aircraft");
        assert_eq!(updated.refuel_reset_at.as_deref(), Some("2026-11-24T06:00:00Z"));

        let disconnected = poll(false, "2026-11-24T06:00:00Z", false);
        let offline = get(&disconnected, "SKY4");
        assert_eq!(offline.id, remote.id);
        assert_eq!(offline.state, State::Refueling);
        assert_eq!(offline.tanker_id, None);
        assert!(offline.data_stale);
        assert!(offline.chat_links.is_none());
        assert_eq!(offline.thread_id.as_deref(), Some("shared-id"));
        assert!(!disconnected.iter().any(|a| Some(a.id) == remote.tanker_id));

        let reconnected = poll(true, "2026-11-24T06:00:00Z", false);
        let back = get(&reconnected, "SKY4");
        assert_eq!(back.id, remote.id);
        assert_eq!(back.environment_id.as_deref(), Some("mac"));
        assert_eq!(back.thread_id.as_deref(), Some("shared-id"));
        assert!(!back.data_stale);
        assert_eq!(
            back.chat_links.as_ref().unwrap().web,
            "https://app.t3.codes/threads/mac/shared-id"
        );
        assert!(back.tanker_id.is_some());

        let settled = poll(true, "2026-11-24T06:00:00Z", true);
        let departing = get(&settled, "SKY4");
        assert_eq!(departing.id, remote.id);
        assert_eq!(departing.state, State::Departing);
        assert!(departing.chat_links.is_none());
        assert!(departing.tanker_id.is_none());
        assert!(!settled.iter().any(|a| Some(a.id) == back.tanker_id));
    }

    #[test]
    fn token_limited_threads_spawn_provider_tanker_and_despawn_when_resumed() {
        let mut world = World::new();
        let mut callsigns = Callsigns::default();
        let mut taxiway_cs = TaxiwayCallsigns::default();
        let mut gates = GateCallsigns::default();
        let mut wingmen = Wingmen::default();
        let mut runway = RunwayOps::default();

        let mut limited = make_entry("t-claude", "Refuel Me", "failed");
        limited.usage_limit_reset_at = Some("2026-11-24T02:00:00.000Z".to_string());
        limited.usage_limit_code = Some("api_error_429".to_string());

        // Completed thread with stale limitRecovery must NOT trigger refueling.
        let mut completed = make_entry("t-done", "Recovered", "completed");
        completed.limit_recovery_auto_resume = Some(true);
        completed.limit_recovery_reset_at = Some("2026-11-24T02:00:00.000Z".to_string());
        assert!(detect_token_wait(&completed).is_none());

        let poll1 = PollData {
            omitted: 0,
            local_environment_id: None,
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![(limited, 0)],
            taxiway: vec![completed],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
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

        let sky1 = world.iter().find(|a| a.callsign == "SKY1").unwrap();
        assert_eq!(sky1.state, State::Refueling);
        assert_eq!(sky1.refuel_reset_at.as_deref(), Some("2026-11-24T02:00:00.000Z"));
        let tankers: Vec<_> = world.iter().filter(|a| a.kind == Kind::Tanker).collect();
        assert_eq!(tankers.len(), 1);
        assert_eq!(tankers[0].callsign, "KC-CLAUDE");
        assert_eq!(sky1.tanker_id, Some(tankers[0].id));

        // Thread resumes running -> disconnects from tanker and tanker disappears!
        let resumed = make_entry("t-claude", "Refuel Me", "running");
        let poll2 = PollData {
            omitted: 0,
            local_environment_id: None,
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![(resumed, 0)],
            taxiway: vec![],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
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
        let sky1 = world.iter().find(|a| a.callsign == "SKY1").unwrap();
        assert_eq!(sky1.state, State::Cruising);
        assert!(sky1.tanker_id.is_none());
        assert_eq!(world.iter().filter(|a| a.kind == Kind::Tanker).count(), 0);
    }

    #[test]
    fn satellite_taxiway_and_stale_threads_never_overlap_on_same_gate() {
        let now = parse_rfc3339("2026-11-24T21:00:00Z").unwrap();
        let airports = vec![
            Airport::home("local", "omarchy"),
            Airport::satellite("mac", "Tom's M5 Air", [0.0, 285.0], true),
        ];
        let mut recent1 = make_entry(&remote::key("mac", "r1"), "Recent 1", "completed");
        recent1.updated_at = Some("2026-11-24T20:00:00Z".into());
        let mut recent2 = make_entry(&remote::key("mac", "r2"), "Recent 2", "completed");
        recent2.updated_at = Some("2026-11-24T19:00:00Z".into());
        let mut stale1 = make_entry(&remote::key("mac", "s1"), "Stale 1", "completed");
        stale1.updated_at = Some("2026-11-22T10:00:00Z".into());
        let mut stale2 = make_entry(&remote::key("mac", "s2"), "Stale 2", "completed");
        stale2.updated_at = Some("2026-11-22T09:00:00Z".into());
        let mut stale3 = make_entry(&remote::key("mac", "s3"), "Stale 3", "completed");
        stale3.updated_at = Some("2026-11-20T08:00:00Z".into());

        let (flying, taxiway, stale, subagents, omitted) = partition_pools(
            vec![recent1, recent2, stale1, stale2, stale3],
            vec![],
            &airports,
            now,
        );
        assert_eq!(taxiway.len(), 2);
        assert_eq!(stale.len(), 2, "remaining 2 of 4 satellite gates hold stale threads");
        assert_eq!(omitted, 1, "5th ground thread is omitted instead of overlapping");

        let mut world = World::new();
        let (mut sky, mut taxi, mut gates, mut wing, mut runway) = (
            Callsigns::default(),
            TaxiwayCallsigns::default(),
            GateCallsigns::default(),
            Wingmen::default(),
            RunwayOps::default(),
        );
        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted,
                local_environment_id: Some("pc".into()),
                airports,
                flying,
                taxiway,
                stale,
                done_or_archived: HashSet::new(),
                subagents,
            },
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );
        assert_eq!(world.aircraft.len(), 4);
        for (i, a) in world.aircraft.iter().enumerate() {
            assert_eq!(a.device.as_deref(), Some("Tom's M5 Air"));
            for b in &world.aircraft[i + 1..] {
                let d = (a.position[0] - b.position[0]).hypot(a.position[1] - b.position[1]);
                assert!(
                    d >= 14.0,
                    "satellite aircraft {} and {} overlapped at {:?} vs {:?}",
                    a.callsign,
                    b.callsign,
                    a.position,
                    b.position
                );
            }
        }
    }

    #[test]
    fn remote_subagents_are_filtered_from_top_level_and_active_ones_fly_as_wingmen() {
        let now = parse_rfc3339("2026-11-24T21:00:00Z").unwrap();
        let peer = remote::Peer {
            environment_id: "mac".into(),
            label: "Tom's M5 Air".into(),
            online: true,
            threads: Some(vec![
                serde_json::json!({
                    "threadId": "parent-1",
                    "title": "Main M5 Task",
                    "status": "running",
                    "model": "gpt-6-luna",
                    "updatedAt": "2026-11-24T20:59:00Z",
                    "settled": false,
                    "parentThreadId": null,
                    "relationshipToParent": null
                }),
                serde_json::json!({
                    "threadId": "sub-live",
                    "title": "Active M5 Scout",
                    "status": "running",
                    "model": "gpt-6.1-sol",
                    "updatedAt": "2026-11-24T20:59:30Z",
                    "settled": false,
                    "parentThreadId": "parent-1",
                    "relationshipToParent": "subagent"
                }),
                serde_json::json!({
                    "threadId": "sub-old-1",
                    "title": "Old Completed Subagent",
                    "status": "completed",
                    "model": "gpt-6.1-sol",
                    "updatedAt": "2026-11-22T10:00:00Z",
                    "settled": false,
                    "parentThreadId": "old-parent",
                    "relationshipToParent": "subagent"
                }),
                serde_json::json!({
                    "threadId": "real-stale",
                    "title": "Real M5 Stale Thread",
                    "status": "completed",
                    "model": "gpt-6.1-sol",
                    "updatedAt": "2026-11-20T08:00:00Z",
                    "settled": false,
                    "parentThreadId": null,
                    "relationshipToParent": null
                }),
            ]),
        };

        let (entries, done, subs) = extract_peer_threads(&peer, now, SUBAGENT_LINGER_SECS);
        assert!(done.is_empty());
        assert_eq!(entries.len(), 2, "only top-level threads become aircraft");
        assert!(entries.iter().all(|e| e.title != "Old Completed Subagent"));
        assert!(entries.iter().all(|e| e.title != "Active M5 Scout"));
        assert_eq!(subs.len(), 1, "only active/lingering subagent becomes wingman");
        assert_eq!(subs[0].title.as_deref(), Some("Active M5 Scout"));

        let airports = vec![
            Airport::home("local", "omarchy"),
            Airport::satellite("mac", "Tom's M5 Air", [0.0, 285.0], true),
        ];
        let (flying, taxiway, stale, subagents, omitted) =
            partition_pools(entries, subs, &airports, now);
        let mut world = World::new();
        let (mut sky, mut taxi, mut gates, mut wing, mut runway) = (
            Callsigns::default(),
            TaxiwayCallsigns::default(),
            GateCallsigns::default(),
            Wingmen::default(),
            RunwayOps::default(),
        );
        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted,
                local_environment_id: Some("pc".into()),
                airports,
                flying,
                taxiway,
                stale,
                done_or_archived: HashSet::new(),
                subagents,
            },
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );

        let fighter = world
            .iter()
            .find(|a| a.kind == Kind::Fighter)
            .expect("active remote subagent should spawn as fighter wingman");
        assert_eq!(fighter.title.as_deref(), Some("Active M5 Scout"));
        assert_eq!(fighter.device.as_deref(), Some("Tom's M5 Air"));
        assert_eq!(fighter.environment_id.as_deref(), Some("mac"));
        assert_eq!(fighter.thread_id.as_deref(), Some("sub-live"));
        assert_eq!(
            fighter.chat_links.as_ref().unwrap().web,
            "https://app.t3.codes/threads/mac/sub-live"
        );
        let parked = world
            .iter()
            .find(|a| a.title.as_deref() == Some("Real M5 Stale Thread"))
            .expect("real top-level thread should occupy a gate");
        assert_eq!(parked.state, State::Parked);
        assert_eq!(parked.device.as_deref(), Some("Tom's M5 Air"));
    }

    #[test]
    fn remote_running_thread_takes_off_and_lands_at_its_satellite_airport() {
        let mac = Airport::satellite("mac", "Tom's M5 Air", [0.0, 285.0], true);
        let mac_thresh = mac.runway_layout().threshold;
        let airports = vec![Airport::home("local", "omarchy"), mac.clone()];
        let mut world = World::new();
        let (mut sky, mut taxi, mut gates, mut wing, mut runway) = (
            Callsigns::default(),
            TaxiwayCallsigns::default(),
            GateCallsigns::default(),
            Wingmen::default(),
            RunwayOps::default(),
        );

        // Poll 1: remote thread starts parked on M5 Air gate 0.
        let t1 = remote::key("mac", "t1");
        let t2 = remote::key("mac", "t2");
        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted: 0,
                local_environment_id: Some("pc".into()),
                airports: airports.clone(),
                flying: vec![],
                taxiway: vec![make_entry(&t1, "M5 Thread 1", "completed")],
                stale: vec![],
                done_or_archived: HashSet::new(),
                subagents: vec![],
            },
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );
        let tx = world.get_mut("TX1").unwrap();
        assert_eq!([tx.position[0], tx.position[1]], mac.gates[0]);

        // Poll 2: t1 wakes up, AND brand-new t2 starts running on M5 Air.
        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted: 0,
                local_environment_id: Some("pc".into()),
                airports: airports.clone(),
                flying: vec![
                    (make_entry(&t1, "M5 Thread 1", "running"), 0),
                    (make_entry(&t2, "M5 Thread 2", "running"), 0),
                ],
                taxiway: vec![],
                stale: vec![],
                done_or_archived: HashSet::new(),
                subagents: vec![],
            },
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );
        let sky1 = world.get_mut("SKY1").unwrap();
        assert_eq!(sky1.airport_id(), "mac");
        assert_eq!(sky1.taxi_destination(), Some(mac_thresh));
        let sky2 = world.get_mut("SKY2").unwrap();
        assert_eq!(sky2.airport_id(), "mac");
        assert_eq!([sky2.position[0], sky2.position[1]], mac_thresh);

        // Advance until SKY2 is cruising, then finish SKY2's turn -> must approach M5 Air!
        for _ in 0..1000 {
            world.tick(0.05);
            if world.get_mut("SKY2").unwrap().state == State::Cruising {
                break;
            }
        }
        assert_eq!(world.get_mut("SKY2").unwrap().state, State::Cruising);

        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted: 0,
                local_environment_id: Some("pc".into()),
                airports,
                flying: vec![(make_entry(&t1, "M5 Thread 1", "running"), 0)],
                taxiway: vec![make_entry(&t2, "M5 Thread 2", "completed")],
                stale: vec![],
                done_or_archived: HashSet::new(),
                subagents: vec![],
            },
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );
        let arriving = world.get_mut("SKY2").unwrap();
        assert_eq!(arriving.state, State::Landing);
        assert_eq!(arriving.airport_id(), "mac");
        let (slot_pos, _) = arriving.arrival_slot().unwrap();
        assert!(
            mac.gates.contains(&slot_pos),
            "remote arrival slot should be a gate on M5 Air, got {:?}",
            slot_pos
        );
    }

}
