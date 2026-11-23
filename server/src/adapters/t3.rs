//! T3 adapter: mirrors recent T3 threads as aircraft.
//!
//! Enable with `SKYWATCH_T3=1` (any value except `0`/`false`). Polls the
//! `t3` CLI for the most recent threads, maps their run state to flight
//! state, and relays radio-panel messages back into the owning thread via
//! `t3 thread send`.

use crate::world::{Aircraft, Kind, ModelClass, State, World};
use crate::ws::{ClientFrame, SharedWorld};
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::sync::mpsc;

/// Poll interval for the thread list.
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// How many of the most recent threads to visualise.
const MAX_AIRCRAFT: usize = 5;

#[derive(Deserialize)]
struct ThreadListEntry {
    #[serde(rename = "threadId")]
    thread_id: String,
    title: String,
    status: String,
    model: Option<String>,
    #[serde(rename = "updatedAt")]
    _updated_at: Option<String>,
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
fn reconcile(world: &mut World, callsigns: &mut Callsigns) -> Vec<(String, String)> {
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

    // Most recent first (list is already newest-first; trim to MAX_AIRCRAFT).
    let recent: Vec<&ThreadListEntry> = entries.iter().take(MAX_AIRCRAFT).collect();

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
        let Some((state, reason, title, model_class)) = keep.get(thread_id) else {
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

    radio_events
}

/// Adapter task: polls T3 and reconciles the world; also relays client
/// messages into the owning threads.
pub async fn run(world: SharedWorld, mut rx: mpsc::Receiver<ClientFrame>) {
    let mut callsigns = Callsigns::default();

    // First reconcile immediately so the world is populated on startup.
    {
        let mut w = world.lock().await;
        let events = reconcile(&mut w, &mut callsigns);
        for (cs, what) in &events {
            tracing::info!(callsign = cs, what, "t3 aircraft update");
        }
    }

    loop {
        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                let mut w = world.lock().await;
                let events = reconcile(&mut w, &mut callsigns);
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
                let Some(thread_id) = callsigns
                    .by_thread
                    .iter()
                    .find(|(_, cs)| **cs == callsign)
                    .map(|(id, _)| id.clone())
                else {
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
