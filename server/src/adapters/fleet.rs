//! Shared session, device and job reconciliation. Transport adapters normalize before entering this module.

use super::remote;

use crate::time::parse_rfc3339;
use crate::world::{
    default_sectors, formation_position, provider_info, Aircraft, Airport, ChatLinks, Kind,
    ModelClass, Sector, State, World, GATE_HEADING, HOME_GATES, PROJECT_SECTOR_RADIUS,
    PROJECT_SECTOR_SLOTS, RUNWAY_HEADING, RUNWAY_THRESHOLD, TAXIWAY_QUEUE,
};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

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

pub(super) fn deserialize_opt_bool_ish<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
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

/// Accepts T3's pending background work as an array (CLI/MCP) or as the JSON
/// text SQLite's `json_group_array` yields; anything else means none.
pub(super) fn deserialize_background_tasks<'de, D>(
    deserializer: D,
) -> Result<Vec<BackgroundTask>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    let val = match val {
        Some(serde_json::Value::String(text)) => serde_json::from_str(&text).unwrap_or_default(),
        Some(v) => v,
        None => serde_json::Value::Null,
    };
    Ok(match val {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect(),
        _ => Vec::new(),
    })
}

/// Work a T3 agent left running after its run ended (T3's `pendingBackgroundTasks`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
pub(super) struct BackgroundTask {
    #[serde(default, alias = "taskId")]
    pub(super) task_id: Option<String>,
    /// `command`, `monitor`, `subagent` or `background_task`.
    #[serde(default)]
    pub(super) kind: Option<String>,
    /// The agent said it is waiting on this task.
    #[serde(default, deserialize_with = "deserialize_opt_bool_ish")]
    pub(super) awaited: Option<bool>,
}

/// T3 prefixes the background task it publishes for a watched pull request.
const PULL_REQUEST_WATCH_TASK_ID: &str = "pull-request-watch:";

impl BackgroundTask {
    /// Mirrors T3's `backgroundWorkHoldsCompletion`: awaited work, sub-agents and
    /// background tool calls keep the thread working after its run ends.
    pub(super) fn holds_thread(&self) -> bool {
        self.awaited == Some(true)
            || matches!(self.kind.as_deref(), Some("subagent" | "background_task"))
    }

    pub(super) fn wait_reason(&self) -> &'static str {
        match self.kind.as_deref() {
            Some("subagent") => "waiting on sub-agent",
            Some("background_task") => "waiting on background tool",
            Some("monitor")
                if self
                    .task_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with(PULL_REQUEST_WATCH_TASK_ID)) =>
            {
                "watching pull request checks"
            }
            Some("monitor") => "waiting on monitor",
            _ => "waiting on background command",
        }
    }

    /// Lower ranks name the more specific wait when several tasks hold a thread.
    pub(super) fn reason_rank(&self) -> u8 {
        match self.kind.as_deref() {
            Some("subagent") => 0,
            Some("background_task") => 1,
            Some("command") => 2,
            _ => 3,
        }
    }
}

#[derive(serde::Serialize, Deserialize, Default)]
pub(super) struct SessionEntry {
    #[serde(rename = "threadId", alias = "thread_id")]
    pub(super) thread_id: String,
    pub(super) title: String,
    #[serde(default)]
    pub(super) project: Option<String>,
    #[serde(default)]
    pub(super) status: String,
    pub(super) model: Option<String>,
    #[serde(rename = "updatedAt", alias = "updated_at")]
    pub(super) updated_at: Option<String>,
    #[serde(default, alias = "pendingRequestCount")]
    pub(super) pending: i64,
    /// Kind of the unresolved runtime request T3 shows first (`user_input`,
    /// `auth_refresh`, `dynamic_tool_call`, or a provider approval kind).
    #[serde(default)]
    pub(super) pending_kind: Option<String>,
    #[serde(
        default,
        alias = "pendingBackgroundTasks",
        deserialize_with = "deserialize_background_tasks"
    )]
    pub(super) background_tasks: Vec<BackgroundTask>,
    /// The running agent is blocked in `t3_thread_wait` / a waiting delegation.
    #[serde(
        default,
        alias = "blockedOnThreadWait",
        deserialize_with = "deserialize_opt_bool_ish"
    )]
    pub(super) blocked_on_thread_wait: Option<bool>,
    #[serde(default)]
    pub(super) settled: bool,
    #[serde(rename = "settledAt", alias = "settled_at", default)]
    pub(super) settled_at: Option<String>,
    #[serde(default, alias = "latestRunId")]
    pub(super) latest_run_id: Option<String>,
    #[serde(default)]
    pub(super) latest_run_provider: Option<String>,
    #[serde(default)]
    pub(super) latest_run_model: Option<String>,
    #[serde(default, alias = "providerInstanceId")]
    pub(super) latest_run_provider_instance: Option<String>,
    #[serde(default, alias = "parentThreadId")]
    pub(super) parent_thread_id: Option<String>,
    #[serde(default, alias = "relationshipToParent")]
    pub(super) relationship_to_parent: Option<String>,
    #[serde(default)]
    pub(super) limit_recovery_run_id: Option<String>,
    #[serde(default)]
    pub(super) limit_recovery_reset_at: Option<String>,
    #[serde(default, deserialize_with = "deserialize_opt_bool_ish")]
    pub(super) limit_recovery_auto_resume: Option<bool>,
    #[serde(default)]
    pub(super) usage_limit_reset_at: Option<String>,
    #[serde(default)]
    pub(super) usage_limit_code: Option<String>,
    /// Set when another environment's thread delegated this one. T3's sidebar
    /// nests such threads under that parent (and its settled state) instead of
    /// listing them on their own; SQLite gives the parent id, the CLI an object.
    #[serde(default, alias = "delegatedFrom")]
    pub(super) delegated_from: Option<serde_json::Value>,
}

impl SessionEntry {
    pub(super) fn is_delegated(&self) -> bool {
        self.delegated_from.as_ref().is_some_and(|v| !v.is_null())
    }

    /// T3 reports a thread whose latest run ended as `waiting` while awaited
    /// background work outlives that run (its `threadStatus`); the run row alone
    /// still says `completed`. Failed runs stay failed, as in T3.
    pub(super) fn settle_background_wait(&mut self) {
        let settled = matches!(
            self.status.as_str(),
            "completed" | "cancelled" | "interrupted" | "idle"
        );
        if settled
            && self
                .background_tasks
                .iter()
                .any(BackgroundTask::holds_thread)
        {
            self.status = "waiting".to_string();
        }
    }

    /// A parent whose own run ended keeps flying while its sub-agents work.
    pub(super) fn wait_on_subagents(&mut self) {
        if !is_working_status(&self.status) {
            self.status = "waiting".to_string();
            self.background_tasks.push(BackgroundTask {
                task_id: None,
                kind: Some("subagent".to_string()),
                awaited: Some(true),
            });
        }
    }
}

/// Flight state plus the most specific reason T3's data gives for a hold.
pub(super) fn entry_flight_state(e: &SessionEntry) -> (State, Option<String>) {
    let (state, reason) = flight_state(&e.status, e.pending);
    if !is_working_status(&e.status) {
        return (state, reason);
    }
    if e.pending > 0 {
        let why = match e.pending_kind.as_deref() {
            None | Some("user_input") => "awaiting your input",
            Some("auth_refresh") => "waiting on sign-in",
            Some("dynamic_tool_call") => "waiting on tool call",
            Some(_) => "waiting on approval",
        };
        return (State::Holding, Some(why.to_string()));
    }
    if e.blocked_on_thread_wait == Some(true) {
        return (
            State::Holding,
            Some("waiting on delegated thread".to_string()),
        );
    }
    if e.status == "waiting" {
        if let Some(task) = e
            .background_tasks
            .iter()
            .filter(|t| t.holds_thread())
            .min_by_key(|t| t.reason_rank())
        {
            return (State::Holding, Some(task.wait_reason().to_string()));
        }
    }
    (state, reason)
}

/// A delegated thread only gets its own aircraft while it is doing work.
pub(super) fn shown_on_its_own(
    e: &SessionEntry,
    parents_with_active_subagents: &HashSet<String>,
) -> bool {
    !e.is_delegated()
        || is_working_status(&e.status)
        || detect_token_wait(e).is_some()
        || parents_with_active_subagents.contains(&e.thread_id)
}

/// Active token/usage-limit wait detected for a T3 thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TokenWait {
    pub(super) reason: String,
    pub(super) reset_at: Option<String>,
    pub(super) model_override: Option<String>,
    pub(super) provider_hint: Option<String>,
    pub(super) scope: String,
}

#[derive(Deserialize)]
pub(super) struct ChildEntry {
    pub(super) subagent_id: String,
    pub(super) thread_id: String,
    pub(super) child_thread_id: Option<String>,
    pub(super) chat_thread_id: Option<String>,
    #[serde(default)]
    pub(super) environment_id: Option<String>,
    pub(super) status: String,
    pub(super) updated_at: Option<String>,
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) model: Option<String>,
}

/// Stable callsign allocation: thread id -> SKYn, in first-sight order.
#[derive(Default)]
pub(super) struct Callsigns {
    pub(super) by_thread: HashMap<String, String>,
    pub(super) next: usize,
}

impl Callsigns {
    pub(super) fn get(&mut self, thread_id: &str) -> String {
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
pub(super) struct TaxiwayCallsigns {
    pub(super) by_thread: HashMap<String, String>,
    pub(super) next: usize,
}

impl TaxiwayCallsigns {
    pub(super) fn get(&mut self, thread_id: &str) -> String {
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
pub(super) struct GateCallsigns {
    pub(super) by_thread: HashMap<String, String>,
    /// Sticky gate index per thread: a plane stays put when another leaves.
    pub(super) slots: HashMap<String, usize>,
    pub(super) next: usize,
}

impl GateCallsigns {
    pub(super) fn get(&mut self, thread_id: &str) -> String {
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
    pub(super) fn assign_ground_slots(
        &mut self,
        taxiway: &[SessionEntry],
        stale: &[SessionEntry],
        airports: &[Airport],
    ) {
        let sat_taxi: Vec<&SessionEntry> = taxiway
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
    pub(super) fn assign_slots(&mut self, entries: &[SessionEntry], airports: &[Airport]) {
        self.assign_ground_slots(&[], entries, airports);
    }
}

/// Threads whose aircraft is flying an approach to land and backtrack to the
/// taxiway queue: thread id -> airborne callsign (SKYn).
#[derive(Default)]
pub(super) struct RunwayOps {
    pub(super) arrivals: HashMap<String, String>,
    /// False until the first poll is applied: threads already running at
    /// startup appear in the air rather than all queueing for takeoff at once.
    pub(super) primed: bool,
}

/// Track active fighter wingmen keyed by subagent_id -> (callsign, child_or_parent_thread_id).
#[derive(Default)]
pub(super) struct Wingmen {
    pub(super) by_subagent: HashMap<String, (String, String)>,
}

/// Normalize the current run's structured T3 usage-limit error or armed recovery.
pub(super) fn detect_token_wait(entry: &SessionEntry) -> Option<TokenWait> {
    if entry.pending > 0 || entry.settled || !matches!(entry.status.as_str(), "waiting" | "failed")
    {
        return None;
    }

    let recovery_armed = entry.limit_recovery_auto_resume == Some(true)
        && entry.limit_recovery_run_id.is_some()
        && entry.limit_recovery_run_id == entry.latest_run_id;
    if !recovery_armed && entry.usage_limit_code.is_none() && entry.usage_limit_reset_at.is_none() {
        return None;
    }
    let reset_at = if recovery_armed {
        entry
            .limit_recovery_reset_at
            .as_ref()
            .or(entry.usage_limit_reset_at.as_ref())
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
        }
        .into(),
        reset_at: reset_at.cloned(),
        model_override: entry.latest_run_model.clone(),
        provider_hint: entry.latest_run_provider.clone(),
        scope,
    })
}

/// A thread is stale when its last update is older than 24 hours.
pub(super) fn is_stale(updated_at: &Option<String>, now: i64) -> bool {
    updated_at
        .as_deref()
        .and_then(parse_rfc3339)
        .map(|t| now - t >= STALE_AFTER_SECS)
        .unwrap_or(false)
}

/// Gate slot i at the home device airport (6 gates along the terminal).
#[cfg(test)]
pub(super) fn home_gate_slot(i: usize) -> [f32; 2] {
    HOME_GATES[i % HOME_GATES.len()]
}

pub(super) fn local_device_name() -> String {
    crate::platform::device_name()
}

/// Hybrid zone classifier:
/// - Named T3 projects (anything other than "No project") get their own dynamic project zone.
/// - "No project" threads are routed into Build, Homelab, AI & Models, or Research by title keywords.
///
/// Returns `(zone_name, is_project_zone)`.
pub(super) fn classify_zone(project: Option<&str>, title: &str) -> (String, bool) {
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

pub(super) fn model_class(model: &Option<String>) -> ModelClass {
    let m = model.as_deref().unwrap_or("").to_lowercase();
    if m.contains("local") || m.contains("ollama") {
        ModelClass::Local
    } else {
        ModelClass::Frontier
    }
}

pub(super) fn is_working_status(status: &str) -> bool {
    matches!(
        status,
        "running" | "starting" | "preparing" | "queued" | "waiting"
    )
}

pub(super) fn flight_state(status: &str, pending_requests: i64) -> (State, Option<String>) {
    match status {
        "waiting" => (State::Holding, Some("awaiting your input".to_string())),
        "running" | "starting" | "preparing" | "queued" if pending_requests > 0 => {
            (State::Holding, Some("awaiting your input".to_string()))
        }
        "running" | "starting" | "preparing" | "queued" => (State::Cruising, None),
        "failed" => (
            State::Ready,
            Some("run failed — waiting on taxiway".to_string()),
        ),
        "interrupted" => (
            State::Ready,
            Some("interrupted — waiting on taxiway".to_string()),
        ),
        "cancelled" => (
            State::Ready,
            Some("cancelled — waiting on taxiway".to_string()),
        ),
        // Non-working threads never fly; <24h queue on Taxiway Alpha, >24h park at gates.
        _ => (State::Ready, None),
    }
}

pub(super) struct PollData {
    pub(super) omitted: usize,
    pub(super) local_environment_id: Option<String>,
    pub(super) airports: Vec<Airport>,
    /// Actively working or token-waiting threads (flying, holding, or refueling).
    pub(super) flying: Vec<(SessionEntry, i64)>,
    /// Recent (<24h) non-running threads queued on Taxiway Alpha waiting for takeoff.
    pub(super) taxiway: Vec<SessionEntry>,
    /// Stale (>24h untouched) threads parked at the extended terminal gates.
    pub(super) stale: Vec<SessionEntry>,
    /// Thread IDs confirmed settled (marked done), archived, or deleted in T3.
    pub(super) done_or_archived: HashSet<String>,
    pub(super) subagents: Vec<ChildEntry>,
}

type Pools = (
    Vec<(SessionEntry, i64)>,
    Vec<SessionEntry>,
    Vec<SessionEntry>,
    Vec<ChildEntry>,
    usize,
);

pub(super) fn partition_pools(
    entries: Vec<SessionEntry>,
    all_subagents: Vec<ChildEntry>,
    airports: &[Airport],
    now: i64,
) -> Pools {
    let parents_with_active_subagents: HashSet<String> = all_subagents
        .iter()
        .filter(|r| is_working_status(&r.status))
        .map(|r| r.thread_id.clone())
        .collect();
    // Delegated threads belong to the thread that delegated them (T3 nests them
    // under it in the sidebar), so they only appear while actually working —
    // never queued on the taxiway or parked at a gate on their own.
    let entries: Vec<SessionEntry> = entries
        .into_iter()
        .map(|mut e| {
            e.settle_background_wait();
            e
        })
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
        } else if is_working_status(&e.status)
            || parents_with_active_subagents.contains(&e.thread_id)
        {
            if *working_count.entry(env.clone()).or_default() < MAX_AIRCRAFT {
                let mut e = e;
                if parents_with_active_subagents.contains(&e.thread_id) {
                    e.wait_on_subagents();
                }
                let pending = e.pending;
                flying.push((e, pending));
                *working_count.entry(env).or_default() += 1;
            }
        } else {
            let count_in = |v: &Vec<SessionEntry>| {
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
    let subagents: Vec<ChildEntry> = all_subagents
        .into_iter()
        .filter(|r| flying_ids.contains(&r.thread_id))
        .collect();
    let omitted = total_entries.saturating_sub(flying.len() + taxiway.len() + stale.len());
    (flying, taxiway, stale, subagents, omitted)
}

pub(super) fn retarget_ground(a: &mut Aircraft, slot: [f32; 2], heading: f32, next: State) {
    // Airborne or mid runway sequence: the lifecycle owners handle it.
    if a.altitude > 0.5 || !matches!(a.state, State::Ready | State::Parked | State::Taxi) {
        return;
    }
    let cur = [a.position[0], a.position[1]];
    let at_slot = (cur[0] - slot[0]).hypot(cur[1] - slot[1]) < 3.0;
    let wants_this_slot = a.state == State::Taxi
        && a.taxi_destination()
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

pub(super) fn airport_for_key<'a>(airports: &'a [Airport], key: &str) -> Option<&'a Airport> {
    if let Some((env, _)) = remote::owner(key) {
        airports.iter().find(|ap| ap.id == env)
    } else {
        airports.iter().find(|ap| ap.id == "local")
    }
}

pub(super) fn taxiway_slot(
    entries: &[SessionEntry],
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

pub(super) fn slot_position(
    thread_id: &str,
    slot: usize,
    airports: &[Airport],
    taxi: bool,
) -> ([f32; 2], f32) {
    let env = remote::owner(thread_id).map(|o| o.0);
    if let Some(ap) = env.and_then(|id| airports.iter().find(|ap| ap.id == id)) {
        return (ap.gates[slot % ap.gates.len()], GATE_HEADING);
    }
    if taxi {
        TAXIWAY_QUEUE[slot % TAXIWAY_QUEUE.len()]
    } else {
        (HOME_GATES[slot % HOME_GATES.len()], GATE_HEADING)
    }
}

pub(super) struct KeepInfo {
    pub(super) state: State,
    pub(super) reason: Option<String>,
    pub(super) title: Option<String>,
    pub(super) model_class: ModelClass,
    pub(super) model: Option<String>,
    pub(super) provider: Option<String>,
    pub(super) reset_at: Option<String>,
    pub(super) refuel_scope: Option<String>,
    pub(super) zone: String,
}

pub(super) fn conversation_links(environment: &str, thread: &str) -> Option<ChatLinks> {
    // Match T3's hosted link encoding. In particular, percent escapes already
    // in a T3 thread ID are literal and must survive one URL decode.
    const THREAD_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_');
    if [environment, thread]
        .iter()
        .any(|id| matches!(*id, "" | "." | ".."))
    {
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

pub(super) fn bind_conversation(
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
#[allow(clippy::too_many_arguments)] // Preserve the existing independently tested reconciliation interface.
pub(super) fn apply_poll(
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

    for (entry, _) in &data.flying {
        let token_wait = detect_token_wait(entry);
        let (state, reason, reset_at, eff_model, prov_hint, refuel_scope) =
            if let Some(tw) = token_wait {
                (
                    State::Refueling,
                    Some(tw.reason),
                    tw.reset_at,
                    tw.model_override.or_else(|| entry.model.clone()),
                    tw.provider_hint,
                    Some(tw.scope),
                )
            } else {
                let (st, r) = entry_flight_state(entry);
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
                // Park where this thread sits on the taxiway: its home queue
                // slot, or its gate at a satellite airport.
                let arrival_slot = data
                    .taxiway
                    .iter()
                    .position(|e| e.thread_id == id)
                    .map(|i| taxiway_slot(&data.taxiway, i, &world.airports, gates))
                    .unwrap_or(TAXIWAY_QUEUE[0]);
                if world
                    .get_mut(&cs)
                    .map(|a| a.begin_approach(Some(arrival_slot)))
                    .unwrap_or(false)
                {
                    // Turn finished: fly the approach, land, vacate at the far
                    // end of the runway and taxi to the owning airport's slot.
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
                            State::Refueling => {
                                "connecting to aerial tanker — waiting for tokens".to_string()
                            }
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
                // Still landing or taxiing from the far-end exit: it becomes this TX when it arrives.
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
        let (_, reason) = entry_flight_state(entry);
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
    let mut by_parent: HashMap<String, Vec<ChildEntry>> = HashMap::new();
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
