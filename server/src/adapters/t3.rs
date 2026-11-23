//! T3 adapter: mirrors recent T3 threads as aircraft.
//!
//! Enable with `SKYWATCH_T3=1` (any value except `0`/`false`). Polls the
//! `t3` CLI for the most recent threads, maps their run state to flight
//! state, and relays radio-panel messages back into the owning thread via
//! `t3 thread send`.

use crate::world::{Aircraft, Kind, ModelClass, State, World, GRAVEYARD_CENTER};
use crate::ws::{ClientFrame, SharedWorld};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Poll interval for the thread list.
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// How many of the most recent threads to visualise.
const MAX_AIRCRAFT: usize = 5;
/// Threads untouched for longer than this are retired to the boneyard.
const STALE_AFTER_SECS: i64 = 24 * 3600;
/// How many retired threads to queue at the graveyard apron.
const MAX_GRAVEYARD: usize = 6;

#[derive(Deserialize)]
struct ThreadListEntry {
    #[serde(rename = "threadId")]
    thread_id: String,
    title: String,
    status: String,
    model: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
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

/// Stable callsign allocation for retired threads: thread id -> GYn.
#[derive(Default)]
struct GraveyardCallsigns {
    by_thread: HashMap<String, String>,
    next: usize,
}

impl GraveyardCallsigns {
    fn get(&mut self, thread_id: &str) -> String {
        self.by_thread.get(thread_id).cloned().unwrap_or_else(|| {
            self.next += 1;
            let cs = format!("GY{}", self.next);
            self.by_thread.insert(thread_id.to_string(), cs.clone());
            cs
        })
    }
}

/// Days since the Unix epoch for a civil (year, month, day) date.
/// Howard Hinnant's `days_from_civil` algorithm.
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
/// Fractional seconds and the trailing Z are ignored; T3 always emits UTC.
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

/// A thread is stale when its last update is older than 24 hours. Unknown
/// timestamps are treated as fresh so we never retire a thread blindly.
fn is_stale(updated_at: &Option<String>, now: i64) -> bool {
    updated_at
        .as_deref()
        .and_then(parse_rfc3339)
        .map(|t| now - t >= STALE_AFTER_SECS)
        .unwrap_or(false)
}

/// Queue slot i on the graveyard apron: a 3-wide, 2-deep parking grid.
fn graveyard_slot(i: usize) -> [f32; 2] {
    [
        GRAVEYARD_CENTER[0] + (i % 3) as f32 * 26.0,
        GRAVEYARD_CENTER[1] + (i / 3) as f32 * 26.0,
    ]
}

fn model_class(model: &Option<String>) -> ModelClass {
    let m = model.as_deref().unwrap_or("").to_lowercase();
    if m.contains("local") || m.contains("ollama") {
        ModelClass::Local
    } else {
        ModelClass::Frontier
    }
}

/// Map a thread's list entry + detail to (state, hold_reason).
fn flight_state(status: &str, pending_requests: i64) -> (State, Option<String>) {
    match status {
        "running" if pending_requests > 0 => {
            (State::Holding, Some("awaiting your input".to_string()))
        }
        "running" => (State::Cruising, None),
        "failed" => (State::Landed, Some("run failed".to_string())),
        "interrupted" => (State::Landed, Some("interrupted".to_string())),
        "cancelled" => (State::Landed, Some("cancelled".to_string())),
        // Completed threads keep flying: mission done, nothing pending.
        _ => (State::Cruising, None),
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

/// Reconcile the world with the current thread list.
fn reconcile(
    world: &mut World,
    callsigns: &mut Callsigns,
    graveyard: &mut GraveyardCallsigns,
) -> Vec<(String, String)> {
    let Some(raw) = run_t3(&["thread", "list", "--json"]) else {
        tracing::warn!("t3 thread list failed; keeping world as-is");
        return Vec::new();
    };
    let entries: Vec<ThreadListEntry> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("t3 thread list parse error: {e}");
            return Vec::new();
        }
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // List is newest-first. Fresh threads fly; stale ones queue at the boneyard.
    let recent: Vec<&ThreadListEntry> = entries
        .iter()
        .filter(|e| !is_stale(&e.updated_at, now))
        .take(MAX_AIRCRAFT)
        .collect();
    let stale: Vec<&ThreadListEntry> = entries
        .iter()
        .filter(|e| is_stale(&e.updated_at, now))
        .take(MAX_GRAVEYARD)
        .collect();

    // Thread ids to keep, and their fresh state.
    let mut keep: HashMap<String, (State, Option<String>, Option<String>, ModelClass)> =
        HashMap::new();
    let mut new_threads: Vec<(String, String, ModelClass)> = Vec::new();

    for entry in &recent {
        // Detail poll for pending requests (holding detection).
        let pending = run_t3(&[
            "thread",
            "show",
            &entry.thread_id,
            "--messages",
            "0",
            "--json",
        ])
        .and_then(|raw| serde_json::from_str::<ThreadShow>(&raw).ok())
        .map(|show| show.thread.pending_request_count)
        .unwrap_or(0);

        let (state, reason) = flight_state(&entry.status, pending);
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

    // Update existing / create new.
    let mut order: Vec<String> = callsigns.by_thread.keys().cloned().collect();
    order.extend(new_threads.iter().map(|(id, _, _)| id.clone()));
    let mut radio_events = Vec::new();

    for thread_id in &order {
        let Some((state, reason, title, _model_class)) = keep.get(thread_id) else {
            continue;
        };
        let cs = callsigns.get(thread_id);
        if let Some(a) = world.get_mut(&cs) {
            if a.state != *state {
                radio_events.push((
                    cs.clone(),
                    match state {
                        State::Holding => "entering hold".to_string(),
                        State::Landed => "landing — blocked".to_string(),
                        State::Cruising => "cruising".to_string(),
                        _ => "updating".to_string(),
                    },
                ));
            }
            a.state = *state;
            a.hold_reason = reason.clone();
            a.title = title.clone();
        } else if let Some((_, t, mc)) = new_threads.iter().find(|(id, _, _)| id == thread_id) {
            let mut a = Aircraft::with_model(&cs, Kind::Agent, *mc);
            a.title = Some(t.clone());
            if let Some((_, reason, title, _)) = keep.get(thread_id) {
                a.hold_reason = reason.clone();
                a.title = title.clone();
            }
            a.state = *state;
            // Assign topic sector round-robin over the known sectors.
            let names = ["Deploy", "Refactor", "Monitor"];
            let _ = a.assign(names[callsigns.next % names.len()]);
            world.add(a);
        }
    }

    // --- Graveyard: stale threads queued, parked and faded at the apron ---
    let stale_ids: HashSet<String> = stale.iter().map(|e| e.thread_id.clone()).collect();

    // Retire GY aircraft whose threads revived (touched again) or dropped out.
    let gy_drop: Vec<String> = graveyard
        .by_thread
        .keys()
        .filter(|id| !stale_ids.contains(*id))
        .cloned()
        .collect();
    for id in gy_drop {
        if let Some(cs) = graveyard.by_thread.remove(&id) {
            world.remove_by_callsign(&cs);
            radio_events.push((cs, "revived — returning to service".to_string()));
        }
    }

    // Queue the stale threads in slot order, parked facing the airport.
    for (i, entry) in stale.iter().enumerate() {
        let cs = graveyard.get(&entry.thread_id);
        let slot = graveyard_slot(i);
        if let Some(a) = world.get_mut(&cs) {
            a.title = Some(entry.title.clone());
            a.position[0] = slot[0];
            a.position[1] = slot[1];
        } else {
            let mut a = Aircraft::with_model(&cs, Kind::Agent, model_class(&entry.model));
            a.title = Some(entry.title.clone());
            a.state = State::Parked;
            a.position = [slot[0], slot[1], 0.0];
            a.heading = std::f32::consts::PI; // nose toward the island airport
            a.topic = None;
            world.add(a);
            radio_events.push((cs.clone(), "retired to the boneyard".to_string()));
        }
    }

    radio_events
}

/// Adapter task: polls T3 and reconciles the world; also relays client
/// messages into the owning threads.
pub async fn run(world: SharedWorld, mut rx: mpsc::Receiver<ClientFrame>) {
    let mut callsigns = Callsigns::default();
    let mut graveyard = GraveyardCallsigns::default();

    // First reconcile immediately so the world is populated on startup.
    {
        let mut w = world.lock().await;
        let events = reconcile(&mut w, &mut callsigns, &mut graveyard);
        for (cs, what) in &events {
            tracing::info!(callsign = cs, what, "t3 aircraft update");
        }
    }

    loop {
        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                let mut w = world.lock().await;
                let events = reconcile(&mut w, &mut callsigns, &mut graveyard);
                for (cs, what) in &events {
                    tracing::info!(callsign = cs, what, "t3 aircraft update");
                }
            }
            Some(frame) = rx.recv() => {
                let (aircraft_id, text) = match frame {
                    ClientFrame::Message { aircraft_id, text } => (aircraft_id, text),
                    ClientFrame::Approve { aircraft_id } => (aircraft_id, "Approved — please continue.".to_string()),
                };
                // Resolve aircraft id -> callsign -> thread id.
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
                // Look up the owning thread: active fleet first, then the
                // boneyard (messaging a retired thread revives it next poll).
                let thread_id = callsigns
                    .by_thread
                    .iter()
                    .find(|(_, cs)| **cs == callsign)
                    .map(|(id, _)| id.clone())
                    .or_else(|| {
                        graveyard
                            .by_thread
                            .iter()
                            .find(|(_, cs)| **cs == callsign)
                            .map(|(id, _)| id.clone())
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
        // 1970-01-01T00:00:00Z is the epoch.
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-02T00:00:00Z"), Some(86400));
        // Fractional seconds are ignored.
        assert_eq!(parse_rfc3339("2026-11-01T17:39:45.542Z").is_some(), true);
        assert_eq!(parse_rfc3339("garbage"), None);
    }

    #[test]
    fn staleness_boundary_is_24_hours() {
        let now = 1_000_000_000i64;
        let ts = |secs_ago: i64| {
            // Build an RFC3339 string for (now - secs_ago) via civil math.
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
        // 23h59m old → fresh; 24h old → stale; unknown → fresh.
        assert!(!is_stale(&ts(24 * 3600 - 60), now));
        assert!(is_stale(&ts(24 * 3600), now));
        assert!(is_stale(&ts(48 * 3600), now));
        assert!(!is_stale(&None, now));
    }

    /// Inverse of days_from_civil, for building test timestamps.
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
    fn graveyard_slots_form_a_queue_grid() {
        let s0 = graveyard_slot(0);
        let s1 = graveyard_slot(1);
        let s3 = graveyard_slot(3);
        assert_eq!(s0, GRAVEYARD_CENTER);
        assert_eq!(s1[0], GRAVEYARD_CENTER[0] + 26.0);
        assert_eq!(s3[1], GRAVEYARD_CENTER[1] + 26.0);
    }
}
