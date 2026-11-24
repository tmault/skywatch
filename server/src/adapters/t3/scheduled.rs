//! Scheduled jobs flown as Reaper drones (read-only).
//!
//! T3's own scheduled tasks are the source on every OS: the local projection
//! database, and linked devices through the MCP bridge. Only summary columns
//! are read; task prompts, webhook tokens and secrets never leave T3, and
//! nothing here can run, enable, disable or edit a task. OS schedulers
//! (cron, systemd timers, launchd, Task Scheduler) plug in later behind
//! [`JobSource`]; see `docs/scheduled-jobs.md`.

use super::{airport_for_key, bind_conversation, classify_zone, remote, run_sqlite_json};
use crate::world::{humanize_schedule, Aircraft, Airport, ScheduledJob, World};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Only the columns Skywatch shows. Never `prompt`, `webhook_token`,
/// `webhook_secret`, workspace or runtime settings.
pub(super) const SCHEDULED_TASKS_SQL: &str = "SELECT s.task_id, s.title, s.enabled, s.schedule_json, s.thread_id, \
       s.next_run_at, s.last_run_at, s.last_run_status, s.run_count, \
       json_extract(s.model_selection_json, '$.model') AS model, p.title AS project \
       FROM scheduled_tasks s LEFT JOIN projection_projects p ON p.project_id = s.project_id \
       ORDER BY s.created_at, s.task_id;";

/// One scheduled job, normalised from any source, with safe fields only.
#[derive(Clone, Debug)]
pub(super) struct JobEntry {
    /// Task id locally, `remote::key(environment, task id)` for linked devices.
    pub key: String,
    pub title: String,
    pub project: Option<String>,
    pub thread_id: Option<String>,
    pub model: Option<String>,
    pub job: ScheduledJob,
}

fn bool_ish<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::Bool(b)) => b,
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    })
}

/// A local `scheduled_tasks` row (see [`SCHEDULED_TASKS_SQL`]).
#[derive(Deserialize)]
struct TaskRow {
    task_id: String,
    title: String,
    #[serde(deserialize_with = "bool_ish")]
    enabled: bool,
    schedule_json: String,
    thread_id: Option<String>,
    next_run_at: Option<String>,
    last_run_at: Option<String>,
    last_run_status: String,
    run_count: Option<u64>,
    model: Option<String>,
    project: Option<String>,
}

/// A linked device's task as the bridge relays `list_scheduled_tasks`.
/// Unknown fields (a prompt, if a bridge ever sent one) are dropped here.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteTask {
    scheduled_task_id: String,
    title: String,
    #[serde(default, deserialize_with = "bool_ish")]
    enabled: bool,
    #[serde(default)]
    schedule: Value,
    bound_thread_id: Option<String>,
    project: Option<String>,
    next_run_at: Option<String>,
    last_run_at: Option<String>,
    #[serde(default)]
    last_run_status: Option<String>,
    run_count: Option<u64>,
}

fn job(schedule: String, enabled: bool, next: Option<String>, last: Option<String>, status: String, count: Option<u64>) -> ScheduledJob {
    ScheduledJob {
        schedule,
        enabled,
        // T3 marks a task `running` from run start until it records the result.
        running: status.eq_ignore_ascii_case("running"),
        next_run_at: next,
        last_run_at: last,
        last_run_status: status,
        run_count: count,
        ..ScheduledJob::default()
    }
}

/// Parse local rows (the JSON `run_sqlite_json` yields).
pub(super) fn parse_local_tasks(raw: &str) -> Vec<JobEntry> {
    let rows: Vec<TaskRow> = serde_json::from_str(raw).unwrap_or_default();
    rows.into_iter()
        .map(|r| JobEntry {
            key: r.task_id,
            title: r.title,
            project: r.project,
            thread_id: r.thread_id,
            model: r.model,
            job: job(humanize_schedule(&r.schedule_json), r.enabled, r.next_run_at, r.last_run_at, r.last_run_status, r.run_count),
        })
        .collect()
}

/// Local T3 scheduled tasks, read-only from the projection database.
pub(super) fn poll_local_tasks() -> Option<Vec<JobEntry>> {
    run_sqlite_json(SCHEDULED_TASKS_SQL).map(|raw| parse_local_tasks(&raw))
}

/// A linked device's tasks, keyed by environment so equal task ids on two
/// devices stay distinct.
pub(super) fn peer_tasks(peer: &remote::Peer) -> Vec<JobEntry> {
    peer.scheduled_tasks
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter_map(|v| serde_json::from_value::<RemoteTask>(v.clone()).ok())
        .map(|t| JobEntry {
            key: remote::key(&peer.environment_id, &t.scheduled_task_id),
            title: t.title,
            project: t.project,
            thread_id: t.bound_thread_id,
            model: None,
            job: job(
                humanize_schedule(&t.schedule.to_string()),
                t.enabled,
                t.next_run_at,
                t.last_run_at,
                t.last_run_status.unwrap_or_else(|| "never".into()),
                t.run_count,
            ),
        })
        .collect()
}

/// A read-only job from an OS scheduler. By construction it carries only the
/// job's name and timing: never its command line, arguments or environment.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OsJob {
    pub name: String,
    pub next_run_at: Option<String>,
    pub last_run_at: Option<String>,
    /// `succeeded`, `failed` or `never`.
    pub last_result: String,
}

/// A future, opt-in OS scheduler adapter (crontab / systemd timers on Linux,
/// launchd on macOS, Task Scheduler on Windows). Read-only.
pub(super) trait JobSource {
    /// Short source label, e.g. `systemd`.
    fn name(&self) -> &'static str;
    fn list(&self) -> Vec<OsJob>;
}

/// Placeholder used when `SKYWATCH_OS_JOBS` opts in before any OS adapter exists.
struct NotYetAvailable;

impl JobSource for NotYetAvailable {
    fn name(&self) -> &'static str {
        "os"
    }
    fn list(&self) -> Vec<OsJob> {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| tracing::info!("SKYWATCH_OS_JOBS is set, but OS scheduler adapters are not available yet"));
        Vec::new()
    }
}

fn os_sources() -> Vec<Box<dyn JobSource>> {
    match std::env::var("SKYWATCH_OS_JOBS").as_deref() {
        Ok("1") | Ok("true") => vec![Box::new(NotYetAvailable)],
        _ => Vec::new(),
    }
}

/// Convert OS jobs into local Reaper entries (home airport, no chat).
pub(super) fn os_entries(sources: &[Box<dyn JobSource>]) -> Vec<JobEntry> {
    sources
        .iter()
        .flat_map(|src| {
            src.list().into_iter().map(move |j| JobEntry {
                key: format!("os:{}:{}", src.name(), j.name),
                title: j.name,
                project: None,
                thread_id: None,
                model: None,
                job: job(format!("{} timer", src.name()), true, j.next_run_at, j.last_run_at, j.last_result, None),
            })
        })
        .collect()
}

pub(super) fn poll_os_jobs() -> Vec<JobEntry> {
    os_entries(&os_sources())
}

/// Stable callsigns: job key -> REAPn, kept across polls and reconnects.
#[derive(Default)]
pub(super) struct ReaperCallsigns {
    pub by_key: HashMap<String, String>,
    next: usize,
}

impl ReaperCallsigns {
    fn get(&mut self, key: &str) -> String {
        if let Some(cs) = self.by_key.get(key) {
            return cs.clone();
        }
        self.next += 1;
        let cs = format!("REAP{}", self.next);
        self.by_key.insert(key.to_owned(), cs.clone());
        cs
    }

    pub fn owns(&self, callsign: &str) -> bool {
        self.by_key.values().any(|cs| cs == callsign)
    }
}

/// Reconcile Reaper drones with the polled jobs, then steer each one toward
/// its lifecycle stage at `now` (Unix seconds).
pub(super) fn apply_jobs(
    world: &mut World,
    home_name: &str,
    local_environment: Option<&str>,
    jobs: Vec<JobEntry>,
    reapers: &mut ReaperCallsigns,
    now: i64,
) {
    // Jobs can be the only activity in a project. Give those projects a sector
    // as well, retaining the thread sectors already allocated by apply_poll.
    let mut projects: Vec<String> = jobs.iter().filter_map(|j| {
        let (zone, is_project) = classify_zone(j.project.as_deref(), &j.title);
        is_project.then_some(zone)
    }).collect();
    projects.sort();
    projects.dedup();
    for name in projects {
        if world.sectors.iter().any(|s| s.name == name) { continue; }
        let slot = world.sectors.iter().filter(|s| s.kind == "project").count();
        if let Some(center) = super::PROJECT_SECTOR_SLOTS.get(slot) {
            world.sectors.push(super::Sector { name, center: *center,
                radius: super::PROJECT_SECTOR_RADIUS, kind: "project".into() });
        }
    }
    // Per airport: running jobs first, then drones already shown (no
    // reshuffling), then the soonest next run. The rest are counted.
    let shown: HashSet<String> = world.iter().filter(|a| a.scheduled_job.is_some()).map(|a| a.callsign.clone()).collect();
    let mut by_airport: HashMap<String, Vec<JobEntry>> = HashMap::new();
    let mut omitted = 0;
    for j in jobs {
        match airport_for_key(&world.airports, &j.key) {
            Some(ap) => by_airport.entry(ap.id.clone()).or_default().push(j),
            None => omitted += 1,
        }
    }
    let mut keep: Vec<(JobEntry, Airport)> = Vec::new();
    for (ap_id, mut list) in by_airport {
        let ap = world.airports.iter().find(|a| a.id == ap_id).cloned().expect("airport");
        let next = |j: &JobEntry| j.job.next_run_at.as_deref().and_then(crate::time::parse_rfc3339).unwrap_or(i64::MAX);
        list.sort_by_key(|j| {
            let on_screen = reapers.by_key.get(&j.key).is_some_and(|cs| shown.contains(cs));
            (!j.job.running, !on_screen, !j.job.enabled, next(j), j.key.clone())
        });
        let cap = world.ramp_capacity(&ap_id);
        omitted += list.len().saturating_sub(cap);
        keep.extend(list.into_iter().take(cap).map(|j| (j, ap.clone())));
    }

    // Drones whose job disappeared or fell outside capacity: a parked one
    // simply leaves the ramp; one in the air flies off rather than vanishing.
    let kept: HashSet<String> = keep.iter().map(|(j, _)| reapers.get(&j.key)).collect();
    let dropped: Vec<String> = world
        .iter()
        .filter(|a| a.scheduled_job.is_some() && !kept.contains(&a.callsign))
        .map(|a| a.callsign.clone())
        .collect();
    for cs in dropped {
        let on_ground = world.get_mut(&cs).is_some_and(|a| a.altitude <= 0.0 && !a.in_runway_sequence());
        if on_ground {
            world.remove_by_callsign(&cs);
        } else if let Some(a) = world.get_mut(&cs) {
            a.retire_job();
        }
    }

    for (entry, ap) in keep {
        let cs = reapers.get(&entry.key);
        if world.get_mut(&cs).is_none() {
            let Some(slot) = world.free_ramp_slot(&ap.id) else {
                omitted += 1;
                continue;
            };
            world.add(Aircraft::reaper(&cs, &ap, slot, entry.job.clone()));
        }
        if world.get_mut(&cs).is_some_and(|a| a.scheduled_job.is_none()) {
            let Some(slot) = world.free_ramp_slot(&ap.id) else {
                omitted += 1;
                continue;
            };
            world.get_mut(&cs).expect("existing reaper").restore_reaper(slot);
        }
        // Loiter over the job's project sector when it has one on the map,
        // otherwise over the domain zone its title suggests.
        let (project_zone, is_project) = classify_zone(entry.project.as_deref(), &entry.title);
        let zone = if is_project && world.sectors.iter().any(|s| s.name == project_zone) {
            project_zone
        } else {
            classify_zone(None, &entry.title).0
        };
        let a = world.get_mut(&cs).expect("reaper exists");
        a.set_airport(&ap);
        a.device = Some(if ap.is_home { home_name.to_owned() } else { ap.name.clone() });
        a.title = Some(entry.title.clone());
        a.model = entry.model.clone();
        a.assign(&zone);
        let phase = a.scheduled_job.as_ref().map(|j| j.phase).unwrap_or_default();
        a.scheduled_job = Some(ScheduledJob { phase, ..entry.job.clone() });
        a.hold_reason = entry.job.last_run_failed().then(|| "last scheduled run failed".to_owned());
        let (environment, thread) = match remote::owner(&entry.key) {
            Some((env, _)) => (Some(env), entry.thread_id.clone()),
            None => (local_environment.map(str::to_owned), entry.thread_id.clone()),
        };
        bind_conversation(a, Some(&ap), environment.as_deref(), thread.as_deref());
    }
    world.omitted_jobs = omitted;
    world.steer_reapers(now);
}
