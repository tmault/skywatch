//! Scheduled jobs as Reaper drones.
//!
//! A scheduled job parks on its airport's small Reaper ramp, taxis out to a
//! holding point when its next run is due, takes its turn on the runway when
//! the run starts, loiters over its sector while running, then lands and
//! taxis back to the same ramp slot. Everything here is read-only
//! visualisation: nothing in this module can start, change or stop a job.

use super::{Aircraft, Airport, Kind, ModelClass, RunwayLayout, State, World};
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// Home Reaper ramp: a small hangar apron north of the arrival return loop,
/// away from the terminal gates and Taxiway Alpha. Slots face the ramp spine.
pub const RAMP_SLOTS_HOME: [([f32; 2], f32); 4] = [
    ([68.0, -104.0], 0.0),
    ([100.0, -104.0], PI),
    ([68.0, -126.0], 0.0),
    ([100.0, -126.0], PI),
];
/// Satellite Reaper ramp slots relative to the island centre (north of the
/// apron, facing the apron lane).
pub const RAMP_SLOTS_SATELLITE: [([f32; 2], f32); 2] = [([30.0, -46.0], PI), ([30.0, -66.0], PI)];
/// Painted ramp spine joining the home ramp to the arrival return loop.
pub(super) const HOME_RAMP_SPINE_X: f32 = 84.0;
/// Home arrival return loop latitude and apron lane (see `far_exit_route`).
const HOME_LOOP_Z: f32 = super::HOME_EXIT_RETURN_Z;
const HOME_APRON_X: f32 = super::APRON_X;
/// Home holding point for a due job: the south apron connector, short of
/// Alpha and the runway hold bay, facing the runway.
const HOME_HOLD: ([f32; 2], f32) = ([96.0, 46.0], PI);
/// A job this close to its next run (s) starts taxiing to the holding point.
pub const DUE_WINDOW_SECS: i64 = 120;
/// A due job that has not started this long after its run time returns to the ramp.
const OVERDUE_GIVE_UP_SECS: i64 = 300;

/// Where the job is in its lifecycle, for labels and the radio panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    /// Enabled and idle, engines off on the ramp.
    #[default]
    Parked,
    /// Paused in T3: parked, dimmed.
    Disabled,
    /// Due soon: taxiing out to the holding point.
    TaxiOut,
    /// Due soon: holding short, waiting for the run to start.
    HoldingShort,
    /// Run started: taxiing to the runway, taking its turn and climbing out.
    Launching,
    /// Run in progress: loitering over its sector.
    OnStation,
    /// Run finished: approach, landing and taxi back to the ramp.
    Returning,
}

/// Read-only description of a scheduled job. Only safe summary fields are
/// kept: never the prompt, webhook tokens/secrets, filters or command lines.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScheduledJob {
    /// Humanised schedule ("every 5 min", "daily at 00:05").
    pub schedule: String,
    pub enabled: bool,
    /// A run is in progress right now.
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    /// `never`, `running`, `succeeded`, `failed`, ... as reported by T3.
    pub last_run_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_count: Option<u64>,
    #[serde(default)]
    pub phase: JobPhase,
}

impl ScheduledJob {
    pub fn last_run_failed(&self) -> bool {
        self.last_run_status.eq_ignore_ascii_case("failed")
    }

    /// The lifecycle stage this job wants at `now` (Unix seconds).
    fn desired(&self, now: i64) -> JobTarget {
        if self.running {
            return JobTarget::Fly;
        }
        if !self.enabled {
            return JobTarget::Park;
        }
        let until = self
            .next_run_at
            .as_deref()
            .and_then(crate::time::parse_rfc3339)
            .map(|t| t - now);
        match until {
            Some(s) if s <= DUE_WINDOW_SECS && s > -OVERDUE_GIVE_UP_SECS => JobTarget::HoldShort,
            _ => JobTarget::Park,
        }
    }
}

/// Lifecycle stage a Reaper is steered toward.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum JobTarget {
    #[default]
    Park,
    HoldShort,
    Fly,
}

/// Humanise T3's `schedule_json` without inventing fields. Only the
/// documented trigger fields are read; anything else (webhook filters,
/// signatures, dedupe keys) is ignored and never shown.
pub fn humanize_schedule(raw: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return "schedule unavailable".into();
    };
    match v.get("type").and_then(|t| t.as_str()) {
        Some("interval") => match v.get("everyMs").and_then(|n| n.as_u64()) {
            Some(ms) if ms >= 1000 => format!("every {}", humanize_duration(ms / 1000)),
            _ => "interval".into(),
        },
        Some("fixed_time") => {
            let time = v
                .get("timeOfDay")
                .and_then(|t| t.as_str())
                .filter(|t| t.len() <= 8 && t.chars().all(|c| c.is_ascii_digit() || c == ':'))
                .unwrap_or("?");
            let days: Option<Vec<u64>> = v
                .get("weekdays")
                .and_then(|w| w.as_array())
                .map(|w| w.iter().filter_map(|d| d.as_u64()).filter(|d| *d <= 6).collect());
            format!("{} at {time}", humanize_weekdays(days))
        }
        Some("once") => match v.get("runAt").and_then(|t| t.as_str()).and_then(crate::time::parse_rfc3339) {
            Some(ts) => {
                let s = crate::time::format_rfc3339(ts);
                format!("once on {} {} UTC", &s[0..10], &s[11..16])
            }
            None => "once".into(),
        },
        Some("webhook") => "on webhook".into(),
        _ => "custom schedule".into(),
    }
}

fn humanize_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    let plural = |n: u64, unit: &str| if n == 1 { format!("1 {unit}") } else { format!("{n} {unit}s") };
    match (d, h, m) {
        (0, 0, 0) => format!("{secs} s"),
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => plural(h, "hour"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, 0, 0) => plural(d, "day"),
        (d, h, _) => format!("{d} d {h} h"),
    }
}

fn humanize_weekdays(days: Option<Vec<u64>>) -> String {
    const NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let Some(mut days) = days.filter(|d| !d.is_empty()) else {
        return "daily".into();
    };
    days.sort_unstable();
    days.dedup();
    match days.as_slice() {
        [0, 1, 2, 3, 4, 5, 6] => "daily".into(),
        [1, 2, 3, 4, 5] => "weekdays".into(),
        [0, 6] => "weekends".into(),
        _ => days.iter().map(|d| NAMES[*d as usize]).collect::<Vec<_>>().join(", "),
    }
}

impl RunwayLayout {
    fn island_center(&self) -> [f32; 2] {
        // Inverse of `RunwayLayout::satellite`.
        [self.runway_x - 4.0, self.threshold[1] - 30.0]
    }

    /// Reaper ramp parking slots `(position, heading)` for this airport.
    pub fn ramp_slots(&self) -> Vec<([f32; 2], f32)> {
        if self.is_home {
            RAMP_SLOTS_HOME.to_vec()
        } else {
            let [cx, cz] = self.island_center();
            RAMP_SLOTS_SATELLITE
                .iter()
                .map(|([x, z], h)| ([cx + x, cz + z], *h))
                .collect()
        }
    }

    /// True for points on this airport's Reaper ramp (north of the apron).
    pub fn on_ramp(&self, p: [f32; 2]) -> bool {
        if self.is_home {
            p[1] < -97.0 && (58.0..112.0).contains(&p[0])
        } else {
            let [cx, cz] = self.island_center();
            p[1] < cz - 38.0 && p[0] > cx + 10.0
        }
    }

    /// Waypoints that take a ramp aircraft at `cur` onto the lane leading to
    /// the runway, and the point where the regular routing takes over.
    pub fn ramp_exit(&self, cur: [f32; 2]) -> (Vec<[f32; 2]>, [f32; 2]) {
        if !self.on_ramp(cur) {
            return (Vec::new(), cur);
        }
        if self.is_home {
            let mut route = Vec::new();
            if (cur[0] - HOME_RAMP_SPINE_X).abs() > 1.0 {
                route.push([HOME_RAMP_SPINE_X, cur[1]]);
            }
            route.push([HOME_RAMP_SPINE_X, HOME_LOOP_Z]);
            route.push([HOME_APRON_X, HOME_LOOP_Z]);
            let end = [HOME_APRON_X, HOME_LOOP_Z];
            (route, end)
        } else {
            let end = [self.apron_x, cur[1]];
            (vec![end], end)
        }
    }

    /// Holding point where a due job waits, short of the runway, for its run.
    pub fn reaper_hold(&self) -> ([f32; 2], f32) {
        if self.is_home {
            HOME_HOLD
        } else {
            ([self.apron_x, self.threshold[1]], PI)
        }
    }

    /// Painted route from a ramp slot out to the holding point.
    fn hold_route(&self, from: [f32; 2]) -> Vec<[f32; 2]> {
        let (mut route, cur) = self.ramp_exit(from);
        let (hold, _) = self.reaper_hold();
        if self.is_home {
            if (cur[0] - HOME_APRON_X).abs() > 1.0 {
                route.push([HOME_APRON_X, cur[1]]);
            }
            route.push([HOME_APRON_X, hold[1]]);
        } else if (cur[0] - self.apron_x).abs() > 1.0 {
            route.push([self.apron_x, cur[1]]);
        }
        route.push(hold);
        route
    }

    /// Painted route from anywhere on the hold route back to a ramp slot.
    fn ramp_return_route(&self, from: [f32; 2], slot: [f32; 2]) -> Vec<[f32; 2]> {
        let mut route = Vec::new();
        if self.is_home {
            let on_spine = self.on_ramp(from)
                || ((from[0] - HOME_RAMP_SPINE_X).abs() < 1.0 && from[1] < HOME_LOOP_Z + 1.0);
            if !on_spine {
                route.push([HOME_APRON_X, from[1]]);
                route.push([HOME_APRON_X, HOME_LOOP_Z]);
                route.push([HOME_RAMP_SPINE_X, HOME_LOOP_Z]);
            }
            route.push([HOME_RAMP_SPINE_X, slot[1]]);
        } else {
            route.push([self.apron_x, from[1]]);
            route.push([self.apron_x, slot[1]]);
        }
        route.push(slot);
        route
    }
}

impl Aircraft {
    /// A scheduled-job Reaper parked on `slot` at `airport`'s ramp.
    pub fn reaper(callsign: &str, airport: &Airport, slot: ([f32; 2], f32), job: ScheduledJob) -> Self {
        let mut a = Self::with_model(callsign, Kind::Drone, ModelClass::Local);
        a.set_airport(airport);
        a.device = Some(airport.name.clone());
        a.state = State::Parked;
        a.position = [slot.0[0], slot.0[1], 0.0];
        a.altitude = 0.0;
        a.prev_alt = 0.0;
        a.heading = slot.1;
        a.speed = 0.0;
        a.ramp_slot = Some(slot);
        a.scheduled_job = Some(job);
        a.update_job_phase();
        a
    }

    /// The job went away while this drone was out: give up the ramp slot
    /// and fly off into the horizon instead of vanishing mid-air.
    pub fn retire_job(&mut self) {
        self.scheduled_job = None;
        self.ramp_slot = None;
        self.park_heading = None;
        self.depart();
    }

    /// Reconnect an authoritatively removed/evicted job before its aircraft
    /// has finished departing. Keep its position and callsign continuous.
    pub fn restore_reaper(&mut self, slot: ([f32; 2], f32)) {
        self.ramp_slot = Some(slot);
        self.departing_forever = false;
        self.departure_heading = None;
        if self.state == State::Departing {
            self.cleared_for_takeoff = false;
            self.state = if self.altitude > 0.0 { State::Cruising } else { State::Takeoff };
            self.joining = self.altitude > 0.0;
        } else if matches!(self.state, State::Taxi | State::Ready)
            && (self.taxi_next == Some(State::Departing) || self.cleared_for_takeoff)
        {
            // Abort a ground farewell departure and route back to the ramp slot;
            // `steer_job` may redirect to `Fly` or `HoldShort` right after.
            let here = [self.position[0], self.position[1]];
            let route = self.airport_layout.ramp_return_route(here, slot.0);
            self.taxi_route(route, State::Parked, slot.1);
        }
    }

    #[cfg(test)]
    pub fn ramp_slot(&self) -> Option<([f32; 2], f32)> {
        self.ramp_slot
    }

    fn at(&self, p: [f32; 2], tol: f32) -> bool {
        (self.position[0] - p[0]).hypot(self.position[1] - p[1]) < tol
    }

    /// Start a ground taxi along explicit painted waypoints.
    fn taxi_route(&mut self, route: Vec<[f32; 2]>, next: State, heading: f32) {
        self.exiting_runway = false;
        self.cleared_for_takeoff = false;
        self.arrival_slot = None;
        self.state = State::Taxi;
        self.taxi_next = Some(next);
        self.ground_timer = 0.0;
        self.park_heading = Some(heading);
        self.taxi_waypoints = route;
        self.taxi_curve = None;
    }

    fn airborne(&self) -> bool {
        matches!(self.state, State::Cruising | State::Holding)
            || (self.state == State::Takeoff && self.altitude > 0.0)
    }

    /// Move one lifecycle step toward `target`. Never copies coordinates:
    /// every change is a taxi, takeoff, go-around or approach.
    fn steer_job(&mut self, target: JobTarget) {
        let Some(slot) = self.ramp_slot else { return };
        let layout = self.airport_layout;
        let (_, hold_heading) = layout.reaper_hold();
        let here = [self.position[0], self.position[1]];
        self.job_target = target;
        match target {
            JobTarget::Fly => match self.state {
                State::Parked => {
                    self.park_heading = None;
                    self.release();
                }
                State::Ready | State::Taxi
                    if !self.cleared_for_takeoff && !self.exiting_runway && self.arrival_slot.is_none() =>
                {
                    self.park_heading = None;
                    self.release();
                }
                // A run started again before the drone got home: go around.
                State::Landing | State::Landed => {
                    self.park_heading = None;
                    self.go_around();
                }
                State::Taxi if self.exiting_runway => {
                    self.park_heading = None;
                    self.release();
                }
                _ => {}
            },
            JobTarget::HoldShort => match self.state {
                State::Parked if self.at(slot.0, 3.0) => {
                    self.taxi_route(layout.hold_route(here), State::Ready, hold_heading);
                }
                State::Parked => {
                    self.taxi_route(layout.ramp_return_route(here, slot.0), State::Parked, slot.1);
                }
                _ => {}
            },
            JobTarget::Park => match self.state {
                _ if self.airborne() => {
                    self.begin_approach(Some(slot));
                }
                State::Ready | State::Taxi
                    if !self.exiting_runway && self.arrival_slot.is_none() =>
                {
                    // Even if already cleared for takeoff (short job finished before
                    // liftoff), cancel the launch clearance and return to the ramp.
                    let homing = !self.cleared_for_takeoff
                        && self.taxi_next == Some(State::Parked)
                        && self.taxi_destination().is_some_and(|d| (d[0] - slot.0[0]).hypot(d[1] - slot.0[1]) < 1.0);
                    if !homing {
                        self.taxi_route(layout.ramp_return_route(here, slot.0), State::Parked, slot.1);
                    }
                }
                State::Parked if !self.at(slot.0, 3.0) => {
                    self.taxi_route(layout.ramp_return_route(here, slot.0), State::Parked, slot.1);
                }
                State::Parked => {
                    self.arrival_slot = None;
                    self.park_heading = Some(slot.1);
                }
                _ => {}
            },
        }
        self.update_job_phase();
    }

    fn update_job_phase(&mut self) {
        let target = self.job_target;
        let state = self.state;
        let airborne = self.airborne();
        let rolling_for_takeoff = state == State::Takeoff && !airborne;
        let taxiing_out = state == State::Taxi
            && self.taxi_next == Some(State::Ready)
            && !self.exiting_runway
            && self.arrival_slot.is_none();
        let Some(job) = self.scheduled_job.as_mut() else { return };
        job.phase = match target {
            JobTarget::Fly if airborne => JobPhase::OnStation,
            JobTarget::Fly => JobPhase::Launching,
            _ if rolling_for_takeoff => JobPhase::Launching,
            JobTarget::HoldShort if state == State::Ready => JobPhase::HoldingShort,
            JobTarget::HoldShort if state == State::Parked => {
                if job.enabled { JobPhase::Parked } else { JobPhase::Disabled }
            }
            JobTarget::HoldShort if taxiing_out => JobPhase::TaxiOut,
            JobTarget::HoldShort => JobPhase::Returning,
            JobTarget::Park if state == State::Parked => {
                if job.enabled { JobPhase::Parked } else { JobPhase::Disabled }
            }
            JobTarget::Park => JobPhase::Returning,
        };
    }
}

impl World {
    /// A free Reaper ramp slot at `airport_id`, keeping every other drone's slot.
    pub fn free_ramp_slot(&self, airport_id: &str) -> Option<([f32; 2], f32)> {
        let layout = self
            .airports
            .iter()
            .find(|ap| ap.id == airport_id)
            .map(Airport::runway_layout)
            .unwrap_or_default();
        layout.ramp_slots().into_iter().find(|(p, _)| {
            !self.aircraft.iter().any(|a| {
                a.airport_id() == airport_id
                    && a.ramp_slot.is_some_and(|(q, _)| (q[0] - p[0]).hypot(q[1] - p[1]) < 1.0)
            })
        })
    }

    /// Ramp capacity at `airport_id`.
    pub fn ramp_capacity(&self, airport_id: &str) -> usize {
        self.airports
            .iter()
            .find(|ap| ap.id == airport_id)
            .map(|ap| ap.runway_layout().ramp_slots().len())
            .unwrap_or(RAMP_SLOTS_HOME.len())
    }

    /// Steer every scheduled-job Reaper toward its job's lifecycle stage at
    /// `now` (Unix seconds). One due job per airport holds short at a time;
    /// the others wait on the ramp until the holding point is free.
    pub fn steer_reapers(&mut self, now: i64) {
        let mut wants: Vec<(usize, JobTarget, i64)> = Vec::new();
        for (i, a) in self.aircraft.iter().enumerate() {
            let Some(job) = &a.scheduled_job else { continue };
            if a.data_stale {
                continue;
            }
            let mut target = job.desired(now);
            // Frequent jobs stay on station between back-to-back runs.
            if target == JobTarget::HoldShort && a.airborne() {
                target = JobTarget::Fly;
            }
            let next = job
                .next_run_at
                .as_deref()
                .and_then(crate::time::parse_rfc3339)
                .unwrap_or(i64::MAX);
            wants.push((i, target, next));
        }
        // Grant the holding point per airport: keep the current holder,
        // otherwise the soonest-due job.
        let mut holder: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut contenders: Vec<&(usize, JobTarget, i64)> =
            wants.iter().filter(|(_, t, _)| *t == JobTarget::HoldShort).collect();
        contenders.sort_by_key(|(i, _, next)| {
            let a = &self.aircraft[*i];
            let committed = a.job_target == JobTarget::HoldShort && a.state != State::Parked;
            (!committed, *next, a.id)
        });
        for (i, _, _) in contenders {
            holder.entry(self.aircraft[*i].airport_id().to_string()).or_insert(*i);
        }
        for (i, target, _) in wants {
            let a = &mut self.aircraft[i];
            let target = if target == JobTarget::HoldShort && holder.get(a.airport_id()) != Some(&i) {
                JobTarget::Park
            } else {
                target
            };
            a.steer_job(target);
        }
    }

    /// Keep each Reaper's lifecycle phase in step with its motion.
    pub(super) fn refresh_reaper_phases(&mut self) {
        for a in &mut self.aircraft {
            if a.scheduled_job.is_some() {
                if a.state == State::Parked && a.job_target == JobTarget::Park {
                    a.arrival_slot = None;
                }
                a.update_job_phase();
            }
        }
    }
}
