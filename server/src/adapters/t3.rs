//! T3 adapter: mirrors recent T3 threads, sub-agent wingmen, and linked
//! device airports.
//!
//! Enable with `SKYWATCH_T3=1` (any value except `0`/`false`). Polls the `t3`
//! CLI for linked devices and recent threads, maps run state to flight state,
//! parks stale threads (>24h) at their device airport's terminal gates, spawns
//! sub-agents as delta-wing fighter jets on their parent's wing, and relays
//! radio-panel messages back into the owning thread via `t3 thread send`.

use super::remote;

use super::{fleet::*, scheduled};
use crate::time::parse_rfc3339;
#[cfg(test)]
use crate::world::wrap_centered;
#[cfg(test)]
use crate::world::{Aircraft, Kind, ModelClass, State, World, HOME_GATES, RUNWAY_THRESHOLD};
use crate::world::{Airport, SATELLITE_SLOTS};
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
/// Recently finished sub-agents linger on the wing briefly before peeling off.
const SUBAGENT_LINGER_SECS: i64 = 600;

#[derive(Deserialize)]
pub(super) struct ThreadIdRow {
    pub(super) thread_id: String,
}

#[derive(Deserialize)]
pub(super) struct ThreadShow {
    pub(super) thread: ThreadDetail,
}

#[derive(Deserialize)]
pub(super) struct ThreadDetail {
    #[serde(rename = "pendingRequestCount")]
    pub(super) pending_request_count: i64,
}

#[derive(Deserialize)]
pub(super) struct EnvLinkEntry {
    #[serde(rename = "environmentId", alias = "environment_id")]
    pub(super) environment_id: String,
    pub(super) label: String,
    #[serde(default)]
    pub(super) status: String,
    #[serde(default)]
    pub(super) last_error: Option<String>,
}

fn run_t3(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(crate::platform::t3_cli()?)
        .args(args)
        .stdin(Stdio::null())
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
    Some(crate::platform::t3_home()?.join("userdata"))
}

/// Run a read-only query against T3's projection database and return the rows
/// as a JSON array of objects (the shape `sqlite3 -json` produced). Uses the
/// bundled SQLite so no `sqlite3` binary is needed on the host.
pub(super) fn run_sqlite_json(sql: &str) -> Option<String> {
    run_sqlite_json_at(&state_dir()?.join("statev2.sqlite"), sql)
}

fn run_sqlite_json_at(db_path: &std::path::Path, sql: &str) -> Option<String> {
    use rusqlite::{types::ValueRef, Connection, OpenFlags};
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    conn.busy_timeout(Duration::from_secs(2)).ok()?;
    let mut stmt = conn.prepare(sql).ok()?;
    let names: Vec<String> = stmt.column_names().map(str::to_owned).collect();
    let mut rows = stmt.query([]).ok()?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().ok()? {
        let mut object = serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {
            let value = match row.get_ref(i).ok()? {
                ValueRef::Null => serde_json::Value::Null,
                ValueRef::Integer(n) => n.into(),
                ValueRef::Real(f) => f.into(),
                ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
                ValueRef::Blob(_) => serde_json::Value::Null,
            };
            object.insert(name.clone(), value);
        }
        out.push(serde_json::Value::Object(object));
    }
    serde_json::to_string(&out).ok()
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
fn poll_subagents(parent_thread_ids: &HashSet<String>, now: i64) -> Vec<ChildEntry> {
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
    let rows: Vec<ChildEntry> = match serde_json::from_str(&raw) {
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
fn poll_threads() -> Option<(Vec<SessionEntry>, HashSet<String>)> {
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
               (SELECT q.kind FROM orchestration_v2_projection_runtime_requests q
                WHERE q.thread_id = t.thread_id AND q.resolved_at IS NULL
                ORDER BY (q.kind = 'user_input'), q.created_at DESC LIMIT 1) AS pending_kind,
               (SELECT json_group_array(json(task)) FROM (
                   SELECT json_object('taskId', json_extract(j.value, '$.taskId'),
                                      'kind', json_extract(j.value, '$.kind'),
                                      'awaited', json_extract(j.value, '$.awaited')) AS task
                   FROM orchestration_v2_projection_provider_threads pt,
                        json_each(pt.payload_json, '$.pendingBackgroundTasks') j
                   WHERE pt.thread_id = t.thread_id
                     AND (json_extract(t.payload_json, '$.activeProviderThreadId') IS NULL
                          OR pt.provider_thread_id = json_extract(t.payload_json, '$.activeProviderThreadId'))
                   UNION ALL
                   SELECT json_object('taskId', bi.turn_item_id,
                                      'kind', CASE bi.type WHEN 'subagent' THEN 'subagent' ELSE 'background_task' END)
                   FROM orchestration_v2_projection_turn_items bi
                   WHERE bi.thread_id = t.thread_id
                     AND bi.type IN ('subagent', 'dynamic_tool')
                     AND bi.status IN ('pending', 'running', 'waiting')
                     AND NOT (bi.type = 'dynamic_tool'
                              AND COALESCE(json_extract(bi.payload_json, '$.input.persistent'), 0) = 1)
                     AND NOT EXISTS (SELECT 1 FROM orchestration_v2_projection_runs rb
                                     WHERE rb.run_id = bi.run_id AND rb.status = 'rolled_back')
                   UNION ALL
                   SELECT json_object('taskId', 'pull-request-watch:' || COALESCE(json_extract(pr.value, '$.number'), ''),
                                      'kind', 'monitor',
                                      'awaited', CASE WHEN json_extract(pr.value, '$.watch.checksPending') = 0
                                                       AND COALESCE(json_extract(pr.value, '$.watch.until'), '') != 'merged'
                                                       AND COALESCE(json_extract(pr.value, '$.snapshot.state'), '') != 'merged'
                                                      THEN 0 ELSE 1 END)
                   FROM json_each(t.payload_json, '$.pullRequests') pr
                   WHERE json_type(pr.value, '$.watch') = 'object'
                     AND COALESCE(json_extract(pr.value, '$.source'), '') != 'stack-dismissed'
               )) AS background_tasks,
               (r.status = 'running' AND EXISTS (
                   SELECT 1 FROM orchestration_v2_projection_turn_items wi
                   WHERE wi.run_id = r.run_id AND wi.type = 'dynamic_tool'
                     AND wi.status IN ('pending', 'running', 'waiting')
                     AND (json_extract(wi.payload_json, '$.toolName') GLOB '*t3_thread_wait'
                          OR (json_extract(wi.payload_json, '$.toolName') GLOB '*delegate_task'
                              AND json_extract(wi.payload_json, '$.input.mode') = 'wait')))) AS blocked_on_thread_wait,
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
        let parsed: Result<Vec<SessionEntry>, _> = if raw.trim().is_empty() {
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
    let all_entries: Vec<SessionEntry> = serde_json::from_str(&raw).ok()?;
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
        e.pending = run_t3(&["thread", "show", &e.thread_id, "--messages", "0", "--json"])
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
pub(super) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(super) fn subagent_linger_secs() -> i64 {
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
) -> (Vec<SessionEntry>, HashSet<String>, Vec<ChildEntry>) {
    let mut entries = Vec::new();
    let mut done_or_archived = HashSet::new();
    let mut subagents = Vec::new();
    for value in peer.threads.as_deref().unwrap_or(&[]) {
        let Ok(mut entry) = serde_json::from_value::<SessionEntry>(value.clone()) else {
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
                subagents.push(ChildEntry {
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
/// Fetch airports, threads, pending-request counts, and sub-agents off-lock.
fn fetch_poll_data(
    home_name: &str,
    reachability: &HashMap<String, bool>,
    cache: &mut remote::Cache,
    last_local_jobs: &mut Vec<scheduled::JobEntry>,
) -> Option<(PollData, Vec<scheduled::JobEntry>)> {
    let local_environment_id = state_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("environment-id")).ok())
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty());
    let mut airports = discover_airports_fast(home_name, reachability);
    let local_poll = poll_threads();
    if let Some(home) = airports.first_mut() {
        home.online = local_poll.is_some();
    }
    if let Some(ref result) = local_poll {
        cache.local = serde_json::to_value(result).ok();
    }
    let (mut entries, mut done_or_archived) = local_poll
        .or_else(|| {
            cache
                .local
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok())
        })
        .unwrap_or_default();
    let local_ids = entries
        .iter()
        .map(|entry| entry.thread_id.clone())
        .collect();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let linger = subagent_linger_secs();
    let mut all_subagents = poll_subagents(&local_ids, now);

    // Linked devices' scheduled tasks change slowly and cost one MCP call per
    // project, so they ride along on every fifth poll; the cache keeps them between.
    static POLLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let with_tasks = POLLS
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .is_multiple_of(5);
    let updates =
        remote::bridge(&serde_json::json!({"action":"poll", "scheduledTasks": with_tasks}))
            .and_then(|v| serde_json::from_value::<Vec<remote::Peer>>(v).ok())
            .unwrap_or_default();
    // Scheduled jobs: a failed local read keeps the last known jobs.
    if let Some(local_jobs) = scheduled::poll_local_tasks() {
        *last_local_jobs = local_jobs;
    }
    let mut jobs = last_local_jobs.clone();
    jobs.extend(scheduled::poll_os_jobs());
    for peer in cache.merge(updates) {
        if let Some(ap) = airports.iter_mut().find(|a| a.id == peer.environment_id) {
            ap.online = peer.online;
        }
        jobs.extend(scheduled::peer_tasks(&peer));
        let (peer_entries, peer_done, peer_subs) = extract_peer_threads(&peer, now, linger);
        entries.extend(peer_entries);
        done_or_archived.extend(peer_done);
        all_subagents.extend(peer_subs);
    }

    let (flying, taxiway, stale, subagents, omitted) =
        partition_pools(entries, all_subagents, &airports, now);

    Some((
        PollData {
            omitted,
            local_environment_id,
            airports,
            flying,
            taxiway,
            stale,
            done_or_archived,
            subagents,
        },
        jobs,
    ))
}

/// Move an already-placed ground aircraft to its newly assigned slot by
/// taxiing it along the painted routes — a poll must never copy its
/// coordinates (that is how queued planes used to teleport).
pub async fn run(world: SharedWorld, mut rx: mpsc::Receiver<ClientFrame>) {
    let home_name = local_device_name();
    let mut callsigns = Callsigns::default();
    let mut taxiway_cs = TaxiwayCallsigns::default();
    let mut gates = GateCallsigns::default();
    let mut wingmen = Wingmen::default();
    let mut runway = RunwayOps::default();
    let mut reapers = scheduled::ReaperCallsigns::default();

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
    let (poll_tx, mut poll_rx) = mpsc::channel::<(PollData, Vec<scheduled::JobEntry>)>(2);
    {
        let home_clone = home_name.clone();
        let reach = std::sync::Arc::clone(&reachability);
        tokio::spawn(async move {
            let mut cache = remote::Cache::default();
            let mut jobs = Vec::new();
            loop {
                let hn = home_clone.clone();
                let snap_reach = reach.read().map(|g| g.clone()).unwrap_or_default();
                let res = tokio::task::spawn_blocking(move || {
                    let data = fetch_poll_data(&hn, &snap_reach, &mut cache, &mut jobs);
                    (data, cache, jobs)
                })
                .await;
                let res = match res {
                    Ok((data, next_cache, next_jobs)) => {
                        cache = next_cache;
                        jobs = next_jobs;
                        Ok(data)
                    }
                    Err(e) => {
                        cache = remote::Cache::default();
                        jobs = Vec::new();
                        Err(e)
                    }
                };
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
            Some((data, jobs)) = poll_rx.recv() => {
                let local_environment = data.local_environment_id.clone();
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
                scheduled::apply_jobs(&mut w, &home_name, local_environment.as_deref(), jobs, &mut reapers, unix_now());
                for (cs, what) in &events {
                    tracing::info!(callsign = cs, what, "t3 aircraft update");
                }
            }
            Some(frame) = rx.recv() => {
                let (aircraft_id, text) = match frame {
                    ClientFrame::Reject { .. } => continue,
                    ClientFrame::Message { aircraft_id, text } => (aircraft_id, text),
                    ClientFrame::Approve { aircraft_id, .. } => (aircraft_id, "Approved — please continue.".to_string()),
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
                // Scheduled jobs are a read-only view: radio traffic never
                // runs, edits, enables or disables a task.
                if reapers.owns(&callsign) {
                    tracing::info!(callsign, "scheduled job radio is read-only; message not relayed");
                    continue;
                }
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
                        let _ = out;
                        tracing::debug!(thread_id, "t3 send accepted");
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
                ChildEntry {
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
                ChildEntry {
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
        assert!(
            !shown_on_its_own(&child, &none),
            "idle delegated thread must not queue"
        );
        child.status = "running".into();
        assert!(
            shown_on_its_own(&child, &none),
            "working delegated thread still flies"
        );
        let own = make_entry("t-own", "Own", "completed");
        assert!(
            shown_on_its_own(&own, &none),
            "ordinary idle threads still queue"
        );
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

    fn make_entry(id: &str, title: &str, status: &str) -> SessionEntry {
        SessionEntry {
            thread_id: id.to_string(),
            title: title.to_string(),
            project: None,
            status: status.to_string(),
            model: Some("claude-opus-5-5".to_string()),
            updated_at: None,
            pending: 0,
            pending_kind: None,
            background_tasks: Vec::new(),
            blocked_on_thread_wait: None,
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
            subagents: vec![ChildEntry {
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
            world
                .aircraft
                .iter()
                .map(|a| &a.callsign)
                .collect::<Vec<_>>()
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
        assert!(
            world.get_mut("TX1").is_none(),
            "TX appears only once it has landed"
        );

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
        // Shallow arrivals may need an extra circuit before glide capture.
        for _ in 0..8000 {
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
        apply_poll(
            &mut world,
            "omarchy",
            poll(false),
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        let tx_id = world.get_mut("TX1").unwrap().id;

        apply_poll(
            &mut world,
            "omarchy",
            poll(true),
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        assert!(world.get_mut("TX1").is_none());
        let sky = world.get_mut("SKY1").unwrap();
        assert_eq!(sky.id, tx_id);
        assert_eq!(sky.state, State::Taxi);

        let mut rolled = false;
        for _ in 0..4000 {
            world.tick(0.05);
            // Polls keep arriving mid-sequence; they must not cut the takeoff short.
            apply_poll(
                &mut world,
                "omarchy",
                poll(true),
                &mut callsigns,
                &mut taxiway_cs,
                &mut gates,
                &mut wingmen,
                &mut runway,
            );
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

    /// A row shaped like the SQLite poll's output: the latest run row says
    /// `completed` and the background work arrives as `json_group_array` text.
    fn sqlite_row(id: &str, status: &str, background_tasks: &str) -> SessionEntry {
        serde_json::from_value(serde_json::json!({
            "thread_id": id,
            "title": "Anon thread",
            "status": status,
            "model": "claude-opus-5-5",
            "updated_at": "2026-11-24T21:12:19.789Z",
            "pending": 0,
            "pending_kind": null,
            "latest_run_id": format!("run:{id}:2"),
            "latest_run_provider": "claudeAgent",
            "background_tasks": background_tasks,
            "blocked_on_thread_wait": 0,
            "delegated_from": null
        }))
        .expect("sqlite-shaped row parses")
    }

    fn pools(
        entries: Vec<SessionEntry>,
        subagents: Vec<ChildEntry>,
    ) -> (Vec<SessionEntry>, Vec<SessionEntry>) {
        let now = parse_rfc3339("2026-11-24T21:17:00Z").unwrap();
        let airports = vec![Airport::home("local", "home")];
        let (flying, taxiway, stale, _, _) = partition_pools(entries, subagents, &airports, now);
        assert!(stale.is_empty());
        (flying.into_iter().map(|(e, _)| e).collect(), taxiway)
    }

    #[test]
    fn completed_run_with_awaited_background_command_stays_airborne_and_says_why() {
        // Reproduces the report: run status `completed`, no pending requests,
        // one awaited background command on the provider thread; T3 says `waiting`.
        let awaited = sqlite_row(
            "anon-awaiting",
            "completed",
            r#"[{"taskId":"bg-1","kind":"command","awaited":1}]"#,
        );
        let fire_and_forget = sqlite_row(
            "anon-detached",
            "completed",
            r#"[{"taskId":"bg-2","kind":"command","awaited":null}]"#,
        );
        let failed = sqlite_row(
            "anon-failed",
            "failed",
            r#"[{"taskId":"bg-3","kind":"command","awaited":1}]"#,
        );
        let (flying, taxiway) = pools(vec![awaited, fire_and_forget, failed], vec![]);
        assert_eq!(flying.len(), 1);
        assert_eq!(flying[0].thread_id, "anon-awaiting");
        assert_eq!(flying[0].status, "waiting");
        assert_eq!(
            entry_flight_state(&flying[0]),
            (
                State::Holding,
                Some("waiting on background command".to_string())
            )
        );
        let ground: Vec<&str> = taxiway.iter().map(|e| e.thread_id.as_str()).collect();
        assert_eq!(
            ground,
            vec!["anon-detached", "anon-failed"],
            "unawaited commands and failed runs stay down, as in T3"
        );
    }

    #[test]
    fn delegated_tasks_tool_calls_and_pull_request_watches_hold_with_their_reason() {
        let cases = [
            (
                r#"[{"taskId":"ti-1","kind":"background_task"},{"taskId":"ti-2","kind":"subagent"}]"#,
                "waiting on sub-agent",
            ),
            (
                r#"[{"taskId":"ti-3","kind":"background_task"}]"#,
                "waiting on background tool",
            ),
            (
                r#"[{"taskId":"pull-request-watch:github.com/anon/repo#7","kind":"monitor","awaited":1}]"#,
                "watching pull request checks",
            ),
            (
                r#"[{"taskId":"mon-1","kind":"monitor","awaited":1}]"#,
                "waiting on monitor",
            ),
        ];
        for (tasks, reason) in cases {
            let (flying, taxiway) = pools(vec![sqlite_row("anon", "completed", tasks)], vec![]);
            assert!(taxiway.is_empty(), "{tasks} must not land");
            assert_eq!(
                entry_flight_state(&flying[0]),
                (State::Holding, Some(reason.to_string()))
            );
        }
        // A watched pull request whose checks finished no longer holds the thread.
        let done = r#"[{"taskId":"pull-request-watch:github.com/anon/repo#7","kind":"monitor","awaited":0}]"#;
        let (flying, taxiway) = pools(vec![sqlite_row("anon", "completed", done)], vec![]);
        assert!(flying.is_empty());
        assert_eq!(taxiway.len(), 1);
    }

    #[test]
    fn pending_requests_and_thread_waits_name_what_the_thread_waits_on() {
        let mut approval = make_entry("anon-approval", "Anon", "running");
        approval.pending = 1;
        approval.pending_kind = Some("command".into());
        assert_eq!(
            entry_flight_state(&approval),
            (State::Holding, Some("waiting on approval".into()))
        );
        assert!(
            detect_token_wait(&approval).is_none(),
            "approval waits never refuel"
        );

        let mut question = make_entry("anon-question", "Anon", "waiting");
        question.pending = 1;
        question.pending_kind = Some("user_input".into());
        assert_eq!(
            entry_flight_state(&question),
            (State::Holding, Some("awaiting your input".into()))
        );

        let mut blocked = make_entry("anon-blocked", "Anon", "running");
        blocked.blocked_on_thread_wait = Some(true);
        assert_eq!(
            entry_flight_state(&blocked),
            (State::Holding, Some("waiting on delegated thread".into()))
        );

        assert_eq!(
            entry_flight_state(&make_entry("anon-busy", "Anon", "running")),
            (State::Cruising, None)
        );
    }

    #[test]
    fn parent_whose_run_ended_holds_for_its_active_subagent() {
        let sub = ChildEntry {
            subagent_id: "anon-sub".into(),
            thread_id: "anon-parent".into(),
            child_thread_id: Some("anon-child".into()),
            chat_thread_id: Some("anon-child".into()),
            environment_id: None,
            status: "running".into(),
            updated_at: None,
            title: Some("Anon scout".into()),
            model: None,
        };
        let (flying, taxiway) = pools(
            vec![make_entry("anon-parent", "Anon", "completed")],
            vec![sub],
        );
        assert!(taxiway.is_empty());
        assert_eq!(
            entry_flight_state(&flying[0]),
            (State::Holding, Some("waiting on sub-agent".into()))
        );
    }

    #[test]
    fn linked_device_threads_keep_t3_background_wait_and_reason() {
        // The MCP/CLI thread list already applies T3's status and lists the tasks.
        let peer = remote::Peer {
            environment_id: "anon-env".into(),
            label: "Anon Laptop".into(),
            online: true,
            scheduled_tasks: None,
            threads: Some(vec![serde_json::json!({
                "threadId": "anon-remote",
                "title": "Anon remote",
                "status": "waiting",
                "updatedAt": "2026-11-24T21:12:19Z",
                "settled": false,
                "pendingRequestCount": 0,
                "blockedOnThreadWait": false,
                "pendingBackgroundTasks": [
                    {"taskId": "bg-9", "kind": "command", "description": "anon", "awaited": true}
                ]
            })]),
        };
        let (entries, _, _) = extract_peer_threads(&peer, 0, 600);
        let (flying, taxiway) = pools(entries, vec![]);
        assert!(taxiway.is_empty());
        assert_eq!(flying[0].thread_id, remote::key("anon-env", "anon-remote"));
        assert_eq!(
            entry_flight_state(&flying[0]),
            (State::Holding, Some("waiting on background command".into()))
        );
    }

    #[test]
    fn taxiing_thread_that_starts_waiting_on_background_work_takes_off_and_holds() {
        let mut world = World::new();
        let mut callsigns = Callsigns::default();
        let mut taxiway_cs = TaxiwayCallsigns::default();
        let mut gates = GateCallsigns::default();
        let mut wingmen = Wingmen::default();
        let mut runway = RunwayOps::default();
        let waiting = r#"[{"taskId":"bg-1","kind":"command","awaited":1}]"#;
        let poll = |tasks: &str| {
            let (flying, taxiway) = pools(vec![sqlite_row("anon-1", "completed", tasks)], vec![]);
            PollData {
                omitted: 0,
                local_environment_id: None,
                airports: vec![Airport::home("local", "home")],
                flying: flying.into_iter().map(|e| (e, 0)).collect(),
                taxiway,
                stale: vec![],
                done_or_archived: HashSet::new(),
                subagents: vec![],
            }
        };
        apply_poll(
            &mut world,
            "home",
            poll("[]"),
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        let tx_id = world.get_mut("TX1").unwrap().id;

        apply_poll(
            &mut world,
            "home",
            poll(waiting),
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        assert!(world.get_mut("TX1").is_none());
        let sky = world.get_mut("SKY1").unwrap();
        assert_eq!(sky.id, tx_id, "same airframe, no teleport");
        assert_eq!(sky.state, State::Taxi);
        assert_eq!(
            sky.hold_reason.as_deref(),
            Some("waiting on background command")
        );

        let mut rolled = false;
        for _ in 0..4000 {
            world.tick(0.05);
            apply_poll(
                &mut world,
                "home",
                poll(waiting),
                &mut callsigns,
                &mut taxiway_cs,
                &mut gates,
                &mut wingmen,
                &mut runway,
            );
            let a = world.get_mut("SKY1").unwrap();
            rolled |= a.state == State::Takeoff && a.altitude <= 0.0;
            if a.state == State::Holding && !a.in_runway_sequence() && a.altitude > 80.0 {
                break;
            }
        }
        let a = world.get_mut("SKY1").unwrap();
        assert!(rolled, "should roll down the runway");
        assert_eq!(a.state, State::Holding);
        assert!(a.altitude > 80.0);
        assert_eq!(
            a.hold_reason.as_deref(),
            Some("waiting on background command")
        );
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
        apply_poll(
            &mut world,
            "omarchy",
            poll(true),
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        assert_eq!(world.get_mut("SKY1").unwrap().state, State::Cruising);
        // Thread stops: the aircraft turns into an arrival.
        apply_poll(
            &mut world,
            "omarchy",
            poll(false),
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
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
        // Shallow circuit, far-end rollout and the taxi back take a while.
        for step in 0..12000 {
            world.tick(0.05);
            apply_poll(
                &mut world,
                "omarchy",
                poll(false),
                &mut callsigns,
                &mut taxiway_cs,
                &mut gates,
                &mut wingmen,
                &mut runway,
            );
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
            flying: ids
                .iter()
                .map(|id| (make_entry(id, "Thread", "running"), 0))
                .collect(),
            taxiway: vec![],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
        };
        // Already running at startup: appears in the air.
        apply_poll(
            &mut world,
            "omarchy",
            poll(&["t-1"]),
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        assert_eq!(world.get_mut("SKY1").unwrap().state, State::Cruising);
        // Starts later: lines up on Runway 36 for takeoff.
        apply_poll(
            &mut world,
            "omarchy",
            poll(&["t-1", "t-2"]),
            &mut callsigns,
            &mut taxiway_cs,
            &mut gates,
            &mut wingmen,
            &mut runway,
        );
        let a = world.get_mut("SKY2").unwrap();
        assert_eq!(a.state, State::Ready);
        assert_eq!([a.position[0], a.position[1]], RUNWAY_THRESHOLD);
        assert!(a.in_runway_sequence());
    }

    #[test]
    fn only_current_structured_limit_waits_use_a_tanker() {
        let fixtures: Vec<SessionEntry> =
            serde_json::from_str(include_str!("../../tests/fixtures/provider-waits.json")).unwrap();
        let waits: Vec<_> = fixtures
            .iter()
            .map(|entry| detect_token_wait(entry))
            .collect();
        assert_eq!(
            waits[0].as_ref().unwrap().reset_at.as_deref(),
            Some("2026-11-24T03:22:09.000Z")
        );
        let open_code = waits[1].as_ref().unwrap();
        assert_eq!(open_code.reset_at, None, "relative prose stays unknown");
        assert_eq!(open_code.reason, "Provider quota reached");
        assert_eq!(
            waits[2].as_ref().unwrap().reset_at.as_deref(),
            Some("2026-11-24T06:00:00.000Z")
        );
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
            let mut flying = vec![
                wait("local-a", "connection-1"),
                wait("local-b", "connection-1"),
                wait("local-c", "connection-2"),
            ];
            if !settled {
                flying.push(wait(&receiver, "connection-1"));
            }
            let data = PollData {
                local_environment_id: None,
                airports: vec![
                    Airport::home("local", "PC"),
                    Airport::satellite("mac", "Mac", [400.0, 0.0], online),
                ],
                omitted: 0,
                flying,
                taxiway: vec![],
                stale: vec![],
                done_or_archived: if settled {
                    HashSet::from([receiver.clone()])
                } else {
                    HashSet::new()
                },
                subagents: vec![],
            };
            apply_poll(
                &mut world,
                "PC",
                data,
                &mut sky,
                &mut taxi,
                &mut gates,
                &mut wing,
                &mut runway,
            );
            world.snapshot().aircraft
        };
        let initial = poll(true, "2026-11-24T02:00:00Z", false);
        let get = |fleet: &[Aircraft], callsign: &str| {
            fleet
                .iter()
                .find(|a| a.callsign == callsign)
                .unwrap()
                .clone()
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
        assert_eq!(
            updated.position, remote.position,
            "a metadata update must not move the aircraft"
        );
        assert_eq!(
            updated.refuel_reset_at.as_deref(),
            Some("2026-11-24T06:00:00Z")
        );

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
        assert_eq!(
            sky1.refuel_reset_at.as_deref(),
            Some("2026-11-24T02:00:00.000Z")
        );
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
        assert_eq!(
            stale.len(),
            2,
            "remaining 2 of 4 satellite gates hold stale threads"
        );
        assert_eq!(
            omitted, 1,
            "5th ground thread is omitted instead of overlapping"
        );

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
            scheduled_tasks: None,
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
        assert_eq!(
            subs.len(),
            1,
            "only active/lingering subagent becomes wingman"
        );
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

    // ---------------------------------------------- scheduled jobs (Reapers)

    use scheduled::{apply_jobs, parse_local_tasks, peer_tasks, JobEntry, ReaperCallsigns};

    const JOB_NOW: i64 = 1_800_000_000;

    fn iso(offset: i64) -> String {
        crate::time::format_rfc3339(JOB_NOW + offset)
    }

    fn job_entry(
        key: &str,
        title: &str,
        enabled: bool,
        status: &str,
        next_in: Option<i64>,
    ) -> JobEntry {
        JobEntry {
            key: key.into(),
            title: title.into(),
            project: None,
            thread_id: Some(format!("thread-{title}")),
            model: None,
            job: crate::world::ScheduledJob {
                schedule: "every 5 min".into(),
                enabled,
                running: status == "running",
                next_run_at: next_in.map(iso),
                last_run_at: None,
                last_run_status: status.into(),
                run_count: Some(3),
                ..Default::default()
            },
        }
    }

    fn job_world() -> World {
        let mut world = World::new();
        world.airports = vec![
            Airport::home("local", "PC"),
            Airport::satellite("mac", "Mac", SATELLITE_SLOTS[0], true),
            Airport::satellite("pc2", "PC2", SATELLITE_SLOTS[1], true),
        ];
        world
    }

    fn reaper_by_title<'a>(w: &'a World, title: &str) -> &'a Aircraft {
        w.iter()
            .find(|a| a.scheduled_job.is_some() && a.title.as_deref() == Some(title))
            .unwrap()
    }

    #[test]
    fn scheduled_tasks_never_send_prompts_or_secrets_to_the_snapshot() {
        let dir = std::env::temp_dir().join(format!("skywatch-jobs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("statev2.sqlite");
        let _ = std::fs::remove_file(&db);
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE projection_projects (project_id TEXT PRIMARY KEY, title TEXT);
                 CREATE TABLE scheduled_tasks (task_id TEXT PRIMARY KEY, title TEXT NOT NULL, prompt TEXT NOT NULL,
                   enabled INTEGER NOT NULL, schedule_json TEXT NOT NULL, project_id TEXT NOT NULL, thread_id TEXT,
                   workspace_strategy_json TEXT NOT NULL, model_selection_json TEXT NOT NULL, runtime_mode TEXT NOT NULL,
                   interaction_mode TEXT NOT NULL, created_by TEXT NOT NULL, creation_source TEXT NOT NULL,
                   created_at TEXT NOT NULL, updated_at TEXT NOT NULL, next_run_at TEXT, last_run_at TEXT,
                   last_run_status TEXT NOT NULL, last_run_error TEXT, run_count INTEGER NOT NULL,
                   webhook_token TEXT, webhook_secret TEXT, last_run_webhook_delivery_id TEXT,
                   thread_mode TEXT NOT NULL DEFAULT 'new_thread');
                 INSERT INTO projection_projects VALUES ('proj-1', 'Anon project');
                 INSERT INTO scheduled_tasks VALUES ('task-a', 'Anon interval job', 'PROMPT-SENTINEL', 1,
                   '{\"type\":\"interval\",\"everyMs\":300000}', 'proj-1', 'thread-a', '{\"path\":\"/WORKSPACE-SENTINEL\"}',
                   '{\"instanceId\":\"x\",\"model\":\"claude-sonnet-5-5\"}', 'full-access', 'default', 'agent', 'mcp',
                   '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', NULL, '2026-01-01T00:00:00Z', 'failed',
                   'ERROR-SENTINEL /private/path', 4, 'TOKEN-SENTINEL', 'SECRET-SENTINEL', 'DELIVERY-SENTINEL', 'continue');
                 INSERT INTO scheduled_tasks VALUES ('task-b', 'Anon webhook job', 'PROMPT-SENTINEL', 0,
                   '{\"type\":\"webhook\",\"signature\":{\"secret\":\"SIG-SENTINEL\",\"header\":\"x-sig\",\"encoding\":\"hex\",\"prefix\":\"\"},\"filters\":[{\"path\":\"body.FILTER-SENTINEL\",\"op\":\"exists\"}]}',
                   'proj-1', NULL, '{}', '{}', 'full-access', 'default', 'agent', 'mcp',
                   '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z', NULL, NULL, 'never', NULL, 0,
                   'TOKEN-SENTINEL', 'SECRET-SENTINEL', NULL, 'new_thread');",
            )
            .unwrap();
        }
        let raw = run_sqlite_json_at(&db, scheduled::SCHEDULED_TASKS_SQL).unwrap();
        let jobs = parse_local_tasks(&raw);
        assert_eq!(jobs.len(), 2);
        let mut world = job_world();
        let mut reapers = ReaperCallsigns::default();
        apply_jobs(&mut world, "PC", Some("pc"), jobs, &mut reapers, JOB_NOW);
        let wire = serde_json::to_string(&world.snapshot()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        // The query reads schedule_json (to humanise it) but no other private column.
        for leak in [
            "PROMPT-",
            "TOKEN-",
            "SECRET-SENTINEL",
            "WORKSPACE",
            "ERROR-",
            "DELIVERY-",
            "/private",
        ] {
            assert!(!raw.contains(leak), "query returned {leak}");
        }
        for leak in [
            "sentinel",
            "prompt",
            "secret",
            "signature",
            "filter",
            "workspace",
            "token",
            "/private",
        ] {
            assert!(
                !wire.to_lowercase().contains(leak),
                "snapshot leaked {leak}"
            );
        }
        // The only webhook mention is the humanised trigger.
        assert!(wire.contains("\"schedule\":\"on webhook\""));
        assert!(wire.contains("\"schedule\":\"every 5 min\""));
        let a = reaper_by_title(&world, "Anon interval job");
        assert_eq!(a.kind, Kind::Drone);
        assert_eq!(a.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(a.scheduled_job.as_ref().unwrap().run_count, Some(4));
        assert_eq!(
            a.chat_links.as_ref().unwrap().web,
            "https://app.t3.codes/threads/pc/thread-a"
        );
    }

    #[test]
    fn schedule_json_variants_are_humanised_without_inventing_fields() {
        use crate::world::humanize_schedule as h;
        assert_eq!(h(r#"{"type":"interval","everyMs":300000}"#), "every 5 min");
        assert_eq!(
            h(r#"{"type":"interval","everyMs":3600000}"#),
            "every 1 hour"
        );
        assert_eq!(
            h(r#"{"type":"interval","everyMs":5400000}"#),
            "every 1 h 30 min"
        );
        assert_eq!(
            h(r#"{"type":"interval","everyMs":172800000}"#),
            "every 2 days"
        );
        assert_eq!(
            h(r#"{"type":"fixed_time","timeOfDay":"00:05"}"#),
            "daily at 00:05"
        );
        assert_eq!(
            h(r#"{"type":"fixed_time","timeOfDay":"09:30","weekdays":[5,1,2,3,4]}"#),
            "weekdays at 09:30"
        );
        assert_eq!(
            h(r#"{"type":"fixed_time","timeOfDay":"10:00","weekdays":[0,6]}"#),
            "weekends at 10:00"
        );
        assert_eq!(
            h(r#"{"type":"fixed_time","timeOfDay":"07:00","weekdays":[1,3,5]}"#),
            "Mon, Wed, Fri at 07:00"
        );
        assert_eq!(
            h(r#"{"type":"once","runAt":"2026-11-28T09:00:00.000Z"}"#),
            "once on 2026-11-28 09:00 UTC"
        );
        assert_eq!(
            h(r#"{"type":"webhook","signature":{"secret":"s"}}"#),
            "on webhook"
        );
        assert_eq!(
            h(r#"{"type":"cron","expression":"*/5 * * * *"}"#),
            "custom schedule"
        );
        assert_eq!(h("not json"), "schedule unavailable");
    }

    #[test]
    fn task_list_flies_reapers_with_stable_callsigns_across_polls() {
        let mut world = job_world();
        let mut reapers = ReaperCallsigns::default();
        let a = job_entry("task-a", "Anon A", true, "never", Some(7200));
        let b = job_entry("task-b", "Anon B", false, "succeeded", None);
        let c = job_entry("task-c", "Anon C", true, "never", Some(9000));
        apply_jobs(
            &mut world,
            "PC",
            Some("pc"),
            vec![a.clone(), b.clone()],
            &mut reapers,
            JOB_NOW,
        );
        let cs = |w: &World, t: &str| reaper_by_title(w, t).callsign.clone();
        let (ca, cb) = (cs(&world, "Anon A"), cs(&world, "Anon B"));
        assert_eq!((ca.as_str(), cb.as_str()), ("REAP1", "REAP2"));
        let id_a = reaper_by_title(&world, "Anon A").id;
        // Reordered list plus a new job: identities hold, the airframe is the same.
        apply_jobs(
            &mut world,
            "PC",
            Some("pc"),
            vec![c.clone(), b.clone(), a.clone()],
            &mut reapers,
            JOB_NOW + 3,
        );
        assert_eq!(cs(&world, "Anon A"), ca);
        assert_eq!(cs(&world, "Anon B"), cb);
        assert_eq!(cs(&world, "Anon C"), "REAP3");
        assert_eq!(reaper_by_title(&world, "Anon A").id, id_a);
        // Enabled jobs park idle; disabled ones park dimmed.
        use crate::world::JobPhase;
        assert_eq!(
            reaper_by_title(&world, "Anon A")
                .scheduled_job
                .as_ref()
                .unwrap()
                .phase,
            JobPhase::Parked
        );
        assert_eq!(
            reaper_by_title(&world, "Anon B")
                .scheduled_job
                .as_ref()
                .unwrap()
                .phase,
            JobPhase::Disabled
        );
        assert!(world
            .iter()
            .filter(|x| x.scheduled_job.is_some())
            .all(|x| x.state == State::Parked));
        // A job that disappears (e.g. a failed poll on reconnect) keeps its callsign on return.
        apply_jobs(
            &mut world,
            "PC",
            Some("pc"),
            vec![a.clone(), c.clone()],
            &mut reapers,
            JOB_NOW + 6,
        );
        assert!(world.iter().all(|x| x.title.as_deref() != Some("Anon B")));
        apply_jobs(
            &mut world,
            "PC",
            Some("pc"),
            vec![a, b, c],
            &mut reapers,
            JOB_NOW + 9,
        );
        assert_eq!(cs(&world, "Anon B"), cb);
    }

    #[test]
    fn due_job_taxis_out_and_takes_off_in_runway_sequence_then_lands_back_on_the_ramp() {
        let mut world = job_world();
        let mut reapers = ReaperCallsigns::default();
        let mut job = job_entry("task-a", "Anon A", true, "succeeded", Some(100));
        let poll = |w: &mut World, r: &mut ReaperCallsigns, j: &JobEntry, t: i64| {
            apply_jobs(w, "PC", Some("pc"), vec![j.clone()], r, JOB_NOW + t);
        };
        // A thread departure already queued for the runway goes first.
        let mut thread = Aircraft::with_model("SKY1", Kind::Agent, ModelClass::Frontier);
        thread.state = State::Parked;
        thread.position = [HOME_GATES[4][0], HOME_GATES[4][1], 0.0];
        thread.altitude = 0.0;
        thread.release();
        world.add(thread);
        poll(&mut world, &mut reapers, &job, 0);
        let mut t = 0.0f32;
        let mut states = Vec::new();
        let step = |w: &mut World,
                    r: &mut ReaperCallsigns,
                    j: &JobEntry,
                    t: &mut f32,
                    states: &mut Vec<State>| {
            for i in 0..60 {
                w.tick(0.05);
                if i == 0 {
                    let a = reaper_by_title(w, "Anon A");
                    if states.last() != Some(&a.state) {
                        states.push(a.state);
                    }
                }
            }
            *t += 3.0;
            poll(w, r, j, *t as i64);
        };
        for _ in 0..30 {
            step(&mut world, &mut reapers, &job, &mut t, &mut states);
        }
        assert_eq!(
            reaper_by_title(&world, "Anon A").state,
            State::Ready,
            "holding short while due"
        );
        job.job.running = true;
        job.job.last_run_status = "running".into();
        let sky1_airborne_first = loop {
            step(&mut world, &mut reapers, &job, &mut t, &mut states);
            let r = reaper_by_title(&world, "Anon A");
            if r.altitude > 0.0 {
                break world
                    .iter()
                    .find(|a| a.callsign == "SKY1")
                    .unwrap()
                    .altitude
                    > 0.0;
            }
            assert!(t < 600.0, "reaper never took off");
        };
        assert!(
            sky1_airborne_first,
            "the reaper must take its turn behind the queued departure"
        );
        for _ in 0..80 {
            step(&mut world, &mut reapers, &job, &mut t, &mut states);
        }
        assert_eq!(reaper_by_title(&world, "Anon A").state, State::Cruising);
        assert_eq!(
            reaper_by_title(&world, "Anon A")
                .scheduled_job
                .as_ref()
                .unwrap()
                .phase,
            crate::world::JobPhase::OnStation
        );
        // Finished, next run far away: land and taxi back.
        job.job.running = false;
        job.job.last_run_status = "succeeded".into();
        job.job.next_run_at = Some(iso(t as i64 + 7200));
        for _ in 0..300 {
            step(&mut world, &mut reapers, &job, &mut t, &mut states);
            if reaper_by_title(&world, "Anon A").state == State::Parked
                && states.contains(&State::Landed)
            {
                break;
            }
        }
        let r = reaper_by_title(&world, "Anon A");
        states.push(r.state);
        assert_eq!(r.state, State::Parked);
        assert!(world.airports[0]
            .runway_layout()
            .on_ramp([r.position[0], r.position[1]]));
        for s in [
            State::Taxi,
            State::Ready,
            State::Takeoff,
            State::Cruising,
            State::Landing,
            State::Landed,
            State::Parked,
        ] {
            assert!(states.contains(&s), "lifecycle skipped {s:?}: {states:?}");
        }
    }

    #[test]
    fn failed_last_run_raises_a_failure_alert_and_radio_is_read_only() {
        let mut world = job_world();
        let mut reapers = ReaperCallsigns::default();
        apply_jobs(
            &mut world,
            "PC",
            Some("pc"),
            vec![job_entry("task-f", "Anon F", true, "failed", Some(3600))],
            &mut reapers,
            JOB_NOW,
        );
        let a = reaper_by_title(&world, "Anon F");
        assert_eq!(a.hold_reason.as_deref(), Some("last scheduled run failed"));
        assert_eq!(a.state, State::Parked);
        assert!(reapers.owns(&a.callsign));
        assert!(!reapers.owns("SKY1"));
    }

    #[test]
    fn identical_task_ids_on_two_devices_stay_distinct_and_keep_ownership() {
        let mut world = job_world();
        let mut reapers = ReaperCallsigns::default();
        let peer = |env: &str| remote::Peer {
            environment_id: env.into(),
            label: env.into(),
            online: true,
            threads: Some(vec![]),
            scheduled_tasks: Some(vec![serde_json::json!({
                "scheduledTaskId": "task-1",
                "title": format!("Anon job on {env}"),
                "prompt": "PROMPT-SENTINEL",
                "enabled": true,
                "boundThreadId": "thread-1",
                "schedule": {"type": "interval", "everyMs": 600000},
                "nextRunAt": iso(3600),
                "lastRunStatus": "succeeded"
            })]),
        };
        let mut jobs = vec![job_entry(
            "task-1",
            "Anon job on local",
            true,
            "never",
            Some(3600),
        )];
        jobs.extend(peer_tasks(&peer("mac")));
        jobs.extend(peer_tasks(&peer("pc2")));
        apply_jobs(
            &mut world,
            "PC",
            Some("pc"),
            jobs.clone(),
            &mut reapers,
            JOB_NOW,
        );
        let local = reaper_by_title(&world, "Anon job on local");
        let mac = reaper_by_title(&world, "Anon job on mac");
        let pc2 = reaper_by_title(&world, "Anon job on pc2");
        let ids: HashSet<u64> = [local.id, mac.id, pc2.id].into_iter().collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(mac.airport_id(), "mac");
        assert_eq!(pc2.airport_id(), "pc2");
        assert_eq!(local.airport_id(), "local");
        assert_eq!(mac.device.as_deref(), Some("Mac"));
        assert_eq!(
            mac.chat_links.as_ref().unwrap().web,
            "https://app.t3.codes/threads/mac/thread-1"
        );
        let mac_layout = world.airports[1].runway_layout();
        assert!(
            mac_layout.on_ramp([mac.position[0], mac.position[1]]),
            "remote job parks on its own island's ramp"
        );
        assert_eq!(mac.scheduled_job.as_ref().unwrap().schedule, "every 10 min");
        let wire = serde_json::to_string(&world.snapshot()).unwrap();
        assert!(!wire.contains("PROMPT-SENTINEL"));
        let before: Vec<String> = ["local", "mac", "pc2"]
            .iter()
            .map(|e| {
                reaper_by_title(&world, &format!("Anon job on {e}"))
                    .callsign
                    .clone()
            })
            .collect();
        apply_jobs(
            &mut world,
            "PC",
            Some("pc"),
            jobs.into_iter().rev().collect(),
            &mut reapers,
            JOB_NOW + 3,
        );
        let after: Vec<String> = ["local", "mac", "pc2"]
            .iter()
            .map(|e| {
                reaper_by_title(&world, &format!("Anon job on {e}"))
                    .callsign
                    .clone()
            })
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn airborne_job_reappearing_before_departure_keeps_identity_and_motion() {
        let mut world = job_world();
        let mut callsigns = ReaperCallsigns::default();
        let entry = job_entry("anon-task", "Anon job", true, "running", None);
        apply_jobs(
            &mut world,
            "PC",
            None,
            vec![entry.clone()],
            &mut callsigns,
            JOB_NOW,
        );
        let cs = reaper_by_title(&world, "Anon job").callsign.clone();
        let a = world.get_mut(&cs).unwrap();
        a.state = State::Cruising;
        a.altitude = 60.0;
        a.position = [40.0, -250.0, 60.0];
        let id = a.id;
        apply_jobs(&mut world, "PC", None, vec![], &mut callsigns, JOB_NOW + 3);
        assert_eq!(world.get_mut(&cs).unwrap().state, State::Departing);
        let pos = world.get_mut(&cs).unwrap().position;
        apply_jobs(
            &mut world,
            "PC",
            None,
            vec![entry],
            &mut callsigns,
            JOB_NOW + 6,
        );
        let a = world.get_mut(&cs).unwrap();
        assert_eq!(a.id, id);
        assert_eq!(a.position, pos);
        assert_eq!(a.state, State::Cruising);
        assert!(a.ramp_slot().is_some());
        assert!(!a.is_departing());
    }

    #[test]
    fn scheduled_only_project_gets_its_own_sector() {
        let mut world = job_world();
        let mut entry = job_entry("anon-task", "Anon job", true, "running", None);
        entry.project = Some("Anon project".into());
        apply_jobs(
            &mut world,
            "PC",
            None,
            vec![entry],
            &mut ReaperCallsigns::default(),
            JOB_NOW,
        );
        let a = reaper_by_title(&world, "Anon job");
        let zone = a.topic.as_deref().unwrap();
        assert!(world
            .sectors
            .iter()
            .any(|s| s.name == zone && s.kind == "project"));
    }

    #[test]
    fn ramp_capacity_shows_an_omitted_count_and_prefers_running_jobs() {
        let mut world = job_world();
        let mut reapers = ReaperCallsigns::default();
        let mut jobs: Vec<JobEntry> = (0..7)
            .map(|i| {
                job_entry(
                    &format!("task-{i}"),
                    &format!("Anon {i}"),
                    true,
                    "never",
                    Some(3600 + i * 60),
                )
            })
            .collect();
        jobs[6].job.running = true;
        jobs[6].job.last_run_status = "running".into();
        apply_jobs(&mut world, "PC", Some("pc"), jobs, &mut reapers, JOB_NOW);
        let shown: Vec<_> = world.iter().filter(|a| a.scheduled_job.is_some()).collect();
        assert_eq!(
            shown.len(),
            crate::world::World::new().ramp_capacity("local")
        );
        assert_eq!(world.omitted_jobs, 7 - shown.len());
        assert!(
            shown.iter().any(|a| a.title.as_deref() == Some("Anon 6")),
            "running job is never the one omitted"
        );
        assert_eq!(world.snapshot().omitted_jobs, 3);
    }

    #[test]
    fn os_scheduler_jobs_expose_only_names_and_timing() {
        struct Fake;
        impl scheduled::JobSource for Fake {
            fn name(&self) -> &'static str {
                "systemd"
            }
            fn list(&self) -> Vec<scheduled::OsJob> {
                vec![scheduled::OsJob {
                    name: "anon-backup.timer".into(),
                    next_run_at: Some(iso(600)),
                    last_run_at: None,
                    last_result: "succeeded".into(),
                }]
            }
        }
        let sources: Vec<Box<dyn scheduled::JobSource>> = vec![Box::new(Fake)];
        let jobs = scheduled::os_entries(&sources);
        assert_eq!(jobs[0].key, "os:systemd:anon-backup.timer");
        assert!(jobs[0].thread_id.is_none());
        let mut world = job_world();
        apply_jobs(
            &mut world,
            "PC",
            None,
            jobs,
            &mut ReaperCallsigns::default(),
            JOB_NOW,
        );
        let a = reaper_by_title(&world, "anon-backup.timer");
        assert_eq!(a.scheduled_job.as_ref().unwrap().schedule, "systemd timer");
        assert!(a.chat_links.is_none());
    }

    #[test]
    fn reentering_hold_after_cruising_away_does_not_teleport_to_old_orbit() {
        let mut world = World::new();
        let (mut sky, mut taxi, mut gates, mut wing, mut runway) = (
            Callsigns::default(),
            TaxiwayCallsigns::default(),
            GateCallsigns::default(),
            Wingmen::default(),
            RunwayOps::default(),
        );
        let poll = |status: &str| PollData {
            omitted: 0,
            local_environment_id: None,
            airports: vec![Airport::home("local", "omarchy")],
            flying: vec![(make_entry("t-hold", "Hold Test", status), 0)],
            taxiway: vec![],
            stale: vec![],
            done_or_archived: HashSet::new(),
            subagents: vec![],
        };

        // Hold #1 at [-200, -200].
        apply_poll(
            &mut world,
            "omarchy",
            poll("waiting"),
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );
        {
            let a = world.get_mut("SKY1").unwrap();
            a.position = [-200.0, -200.0, 150.0];
            a.altitude = 150.0;
        }
        world.tick(0.05); // derives orbit_center around [-200, -200]

        // Resume cruising and fly far away to [200, 200].
        apply_poll(
            &mut world,
            "omarchy",
            poll("running"),
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );
        {
            let a = world.get_mut("SKY1").unwrap();
            a.position = [200.0, 200.0, 150.0];
            a.altitude = 150.0;
        }
        world.tick(0.05);
        let before = world.get_mut("SKY1").unwrap().position;

        // Re-enter Holding via apply_poll: must orbit near [200, 200], never snap back to [-200, -200].
        apply_poll(
            &mut world,
            "omarchy",
            poll("waiting"),
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );
        world.tick(0.05);
        let after = world.get_mut("SKY1").unwrap().position;
        let jump = wrap_centered(after[0] - before[0]).hypot(wrap_centered(after[1] - before[1]));
        assert!(
            jump < 5.0,
            "re-entering hold teleported aircraft by {jump} units ({before:?} -> {after:?})"
        );
    }

    #[test]
    fn landing_thread_moving_to_stale_pool_does_not_spawn_duplicate_aircraft() {
        let mut world = World::new();
        let (mut sky, mut taxi, mut gates, mut wing, mut runway) = (
            Callsigns::default(),
            TaxiwayCallsigns::default(),
            GateCallsigns::default(),
            Wingmen::default(),
            RunwayOps::default(),
        );
        // Poll 1: flying.
        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted: 0,
                local_environment_id: None,
                airports: vec![Airport::home("local", "omarchy")],
                flying: vec![(make_entry("t-dup", "Arrival", "running"), 0)],
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
        // Poll 2: stops -> enters taxiway and begins Landing as SKY1.
        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted: 0,
                local_environment_id: None,
                airports: vec![Airport::home("local", "omarchy")],
                flying: vec![],
                taxiway: vec![make_entry("t-dup", "Arrival", "completed")],
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
        assert_eq!(world.aircraft.len(), 1);
        assert_eq!(world.get_mut("SKY1").unwrap().state, State::Landing);

        // Poll 3: pushed into stale pool while still Landing -> must NOT spawn GT1 alongside SKY1!
        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted: 0,
                local_environment_id: None,
                airports: vec![Airport::home("local", "omarchy")],
                flying: vec![],
                taxiway: vec![],
                stale: vec![make_entry("t-dup", "Arrival", "completed")],
                done_or_archived: HashSet::new(),
                subagents: vec![],
            },
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );
        assert_eq!(
            world.aircraft.len(),
            1,
            "duplicate aircraft spawned while SKY1 was still landing: {:?}",
            world
                .aircraft
                .iter()
                .map(|a| (&a.callsign, a.state))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn unsettling_departing_thread_reclaims_airframe_without_duplication() {
        let mut world = World::new();
        let (mut sky, mut taxi, mut gates, mut wing, mut runway) = (
            Callsigns::default(),
            TaxiwayCallsigns::default(),
            GateCallsigns::default(),
            Wingmen::default(),
            RunwayOps::default(),
        );
        // Poll 1: flying.
        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted: 0,
                local_environment_id: None,
                airports: vec![Airport::home("local", "omarchy")],
                flying: vec![(make_entry("t-revive", "Revive Me", "running"), 0)],
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
        let original_id = world.get_mut("SKY1").unwrap().id;
        // Poll 2: settled -> SKY1 departs.
        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted: 0,
                local_environment_id: None,
                airports: vec![Airport::home("local", "omarchy")],
                flying: vec![],
                taxiway: vec![],
                stale: vec![],
                done_or_archived: HashSet::from(["t-revive".to_string()]),
                subagents: vec![],
            },
            &mut sky,
            &mut taxi,
            &mut gates,
            &mut wing,
            &mut runway,
        );
        assert_eq!(world.get_mut("SKY1").unwrap().state, State::Departing);

        // Poll 3: unsettled before despawn -> reclaims same airframe (no duplicate!).
        apply_poll(
            &mut world,
            "omarchy",
            PollData {
                omitted: 0,
                local_environment_id: None,
                airports: vec![Airport::home("local", "omarchy")],
                flying: vec![],
                taxiway: vec![make_entry("t-revive", "Revive Me", "idle")],
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
        assert_eq!(
            world.aircraft.len(),
            1,
            "unsettled thread must not duplicate departing aircraft"
        );
        assert_eq!(world.aircraft[0].id, original_id);
        assert_eq!(world.aircraft[0].state, State::Landing);
        assert!(!world.aircraft[0].is_departing());
    }
}
