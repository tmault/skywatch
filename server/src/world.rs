mod ground_route;
mod reaper;
mod tanker;

pub use reaper::{humanize_schedule, ScheduledJob};
#[cfg(test)]
pub use reaper::JobPhase;

use rand::Rng;
use serde::{Deserialize, Serialize};

/// What kind of entity this is: a piloted agent, an autonomous drone, a
/// sub-agent fighter jet escorting a parent aircraft, or a provider aerial
/// refuelling tanker orbiting high above the airspace zones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Agent,
    Drone,
    Fighter,
    Tanker,
}

/// Lifecycle state of an aircraft in the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Cruising,
    Holding,
    /// Waiting for provider tokens/quota: flying in formation behind the
    /// provider's aerial refuelling tanker with a hose connected.
    Refueling,
    Landed,
    /// Rolling on the ground between runway and parking gate.
    Taxi,
    /// Parked at a terminal gate, engines off.
    Parked,
    /// Lined up on the runway, holding short for takeoff clearance.
    Ready,
    Takeoff,
    Landing,
    /// Thread marked done / archived: taking off and flying into the horizon forever.
    Departing,
}

/// The class of model powering an agent: local props vs frontier jets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelClass {
    /// Little private plane: local model (e.g. Ollama on the devbox).
    #[default]
    Local,
    /// Big jet: frontier model (e.g. Claude, GPT).
    Frontier,
}

/// A device airport island in the archipelago. Each connected machine has its
/// own airport with runway, tower, terminal and parking gates.
#[derive(Clone, Debug, Serialize)]
pub struct Airport {
    pub id: String,
    pub name: String,
    /// Centre `[x, z]` in world coordinates.
    pub center: [f32; 2],
    /// Main local machine island at the centre of the map.
    pub is_home: bool,
    /// Whether the device is currently reachable.
    pub online: bool,
    /// World `[x, z]` coordinates of parking gates at the terminal.
    pub gates: Vec<[f32; 2]>,
}

impl Airport {
    /// Build the home island airport at `[0, 0]` with 6 terminal gates.
    pub fn home(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            center: [0.0, 0.0],
            is_home: true,
            online: true,
            gates: HOME_GATES.to_vec(),
        }
    }

    /// Build a satellite device airport at `center` with 4 terminal gates.
    pub fn satellite(
        id: impl Into<String>,
        name: impl Into<String>,
        center: [f32; 2],
        online: bool,
    ) -> Self {
        let gates = (0..4)
            .map(|i| [center[0] + 24.0, center[1] - 24.0 + i as f32 * 16.0])
            .collect();
        Self {
            id: id.into(),
            name: name.into(),
            center,
            is_home: false,
            online,
            gates,
        }
    }

    /// Runway, approach, and apron geometry matching the client's island layout.
    pub fn runway_layout(&self) -> RunwayLayout {
        if self.is_home {
            RunwayLayout::home()
        } else {
            RunwayLayout::satellite(self.center)
        }
    }
}

/// Runway and ground-routing coordinates for one airport island.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunwayLayout {
    pub is_home: bool,
    pub runway_x: f32,
    pub threshold: [f32; 2],
    pub touchdown_z: f32,
    pub liftoff_z: f32,
    pub end_z: f32,
    /// Far-end runway exit: arrivals roll out to here and vacate the runway.
    pub exit_z: f32,
    pub final_approach_fix: [f32; 2],
    pub approach_alt: f32,
    pub taxiway_x: f32,
    pub apron_x: f32,
}

impl Default for RunwayLayout {
    fn default() -> Self {
        Self::home()
    }
}

impl RunwayLayout {
    /// Distance along the extended centerline from the final approach fix to touchdown.
    pub fn final_len(&self) -> f32 {
        wrap_centered(self.final_approach_fix[1] - self.touchdown_z).max(FLARE_DISTANCE)
    }

    /// A runway occupancy this close (u) to touchdown forces a go-around.
    pub fn goaround_dist(&self) -> f32 {
        if self.is_home { GOAROUND_BLOCKED_DIST } else { SATELLITE_GOAROUND_DIST }
    }

    /// Far-end exit and return taxi route to `dest`: vacate the runway at
    /// the opposite end from the departure entrance, then follow painted
    /// taxiways (never the runway or the departure queue lane) back to `dest`.
    pub fn far_exit_route(&self, dest: [f32; 2]) -> Vec<[f32; 2]> {
        let route = if self.is_home && self.on_ramp(dest) {
            // Reaper ramp: the return loop's spine leads north onto the ramp.
            vec![
                [self.runway_x, self.exit_z],
                [HOME_EXIT_LANE_X, self.exit_z],
                [HOME_EXIT_LANE_X, HOME_EXIT_RETURN_Z],
                [reaper::HOME_RAMP_SPINE_X, HOME_EXIT_RETURN_Z],
                [reaper::HOME_RAMP_SPINE_X, dest[1]],
                dest,
            ]
        } else if self.is_home {
            // North connector loops past Alpha's last holding slot, then the
            // apron lane brings arrivals south to their slot's latitude.
            vec![
                [self.runway_x, self.exit_z],
                [HOME_EXIT_LANE_X, self.exit_z],
                [HOME_EXIT_LANE_X, HOME_EXIT_RETURN_Z],
                [self.apron_x, HOME_EXIT_RETURN_Z],
                [self.apron_x, dest[1]],
                dest,
            ]
        } else {
            vec![
                [self.runway_x, self.exit_z],
                [self.apron_x, self.exit_z],
                [self.apron_x, dest[1]],
                dest,
            ]
        };
        let mut out: Vec<[f32; 2]> = Vec::with_capacity(route.len());
        for p in route {
            if out.last().is_none_or(|q: &[f32; 2]| (q[0] - p[0]).hypot(q[1] - p[1]) > 0.5) {
                out.push(p);
            }
        }
        out
    }

    pub const fn home() -> Self {
        Self {
            is_home: true,
            runway_x: RUNWAY_THRESHOLD[0],
            threshold: RUNWAY_THRESHOLD,
            touchdown_z: TOUCHDOWN_Z,
            liftoff_z: RUNWAY_LIFTOFF_Z,
            end_z: RUNWAY_END_Z,
            exit_z: RUNWAY_EXIT_Z,
            final_approach_fix: FINAL_APPROACH_FIX,
            approach_alt: APPROACH_ALT,
            taxiway_x: TAXIWAY_X,
            apron_x: APRON_X,
        }
    }

    /// Satellite island layout matching `buildSatelliteAirport` in `web/main.js`
    /// (runway at local `x = 4`, `z ∈ [-36.5, 36.5]`; gates at local `x = 24`).
    pub fn satellite(center: [f32; 2]) -> Self {
        let rx = center[0] + 4.0;
        let cz = center[1];
        let thresh_z = cz + 30.0;
        Self {
            is_home: false,
            runway_x: rx,
            threshold: [rx, thresh_z],
            touchdown_z: cz + 22.0,
            liftoff_z: cz - 14.0,
            end_z: cz - 36.5,
            // North-end connector straight across onto the apron lane.
            exit_z: cz - 30.5,
            // Same three-degree glide as home on a shorter final. Southern
            // satellites' finals extend past the map edge over open water;
            // approaches fly in the airport's chart, so they never wrap there.
            final_approach_fix: [rx, wrap_centered(cz + 22.0 + SATELLITE_FINAL_LEN)],
            approach_alt: GLIDE_SLOPE * (SATELLITE_FINAL_LEN - FLARE_DISTANCE / 2.0),
            taxiway_x: center[0] + 16.0,
            apron_x: center[0] + 16.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatLinks {
    pub web: String,
    pub desktop: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aircraft {
    pub id: u64,
    pub callsign: String,
    pub kind: Kind,
    /// Airframe class: what model powers this agent (agents only).
    #[serde(default)]
    pub model_class: ModelClass,
    /// Raw model id (e.g. "claude-opus-5-5"); the client derives the
    /// provider livery from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub state: State,
    /// `[x, z, altitude]` in world units (client maps to Three.js `(x, altitude, z)`).
    pub position: [f32; 3],
    pub heading: f32,
    pub altitude: f32,
    /// Landing attitude in radians, positive nose up; independent of snapshot cadence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing_pitch: Option<f32>,
    pub hold_reason: Option<String>,
    /// Topic sector this aircraft works in (None = unassigned).
    #[serde(default)]
    pub topic: Option<String>,
    /// Human-readable subject (thread title or sub-agent task).
    #[serde(default)]
    pub title: Option<String>,
    /// Owning device / airport name (e.g. "omarchy", "M5 Air").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_links: Option<ChatLinks>,
    #[serde(default)]
    pub data_stale: bool,
    /// Parent aircraft id for escorting fighter wingmen (sub-agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<u64>,
    /// Wingman formation slot: 0 = left wing, 1 = right wing, 2 = outer left, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wing_slot: Option<usize>,
    /// Canonical provider key (e.g. "anthropic", "openai", "google") for
    /// tankers and refuelling aircraft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Tanker aircraft id assigned for rendezvous while `State::Refueling`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tanker_id: Option<u64>,
    /// True after the receiver has matched its assigned formation slot.
    #[serde(default)]
    pub refuel_connected: bool,
    /// Internal limit scope. Provider branding is independent of quota ownership.
    #[serde(skip)]
    pub(crate) refuel_scope: Option<String>,
    /// Refuelling hose/formation slot behind the tanker:
    /// 0 = center boom, 1 = port wing pod, 2 = starboard wing pod, 3+ = echelon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refuel_slot: Option<usize>,
    /// Optional RFC3339 timestamp when provider tokens/quota reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refuel_reset_at: Option<String>,
    /// Scheduled job this Reaper drone visualises (read-only; never a thread).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_job: Option<ScheduledJob>,
    /// Sticky parking slot `(position, heading)` on the owning airport's Reaper ramp.
    #[serde(skip)]
    ramp_slot: Option<([f32; 2], f32)>,
    /// Lifecycle stage the Reaper is being steered toward.
    #[serde(skip)]
    job_target: reaper::JobTarget,
    /// Heading to settle on when a taxi ends parked or holding short off the runway.
    #[serde(skip)]
    park_heading: Option<f32>,
    /// Tanker flight-level slot index (assigned dynamically when multiple providers are active).
    #[serde(skip)]
    tanker_orbit_idx: usize,
    /// Holding-orbit centre (derived on entry, not serialised).
    #[serde(skip)]
    orbit_center: Option<[f32; 2]>,
    /// Patrol orbit angle for sector-bound aircraft.
    #[serde(default)]
    patrol_angle: f32,
    /// Ordered ground-movement waypoints (taxiway routing), not serialised.
    #[serde(skip)]
    taxi_waypoints: Vec<[f32; 2]>,
    #[serde(skip)]
    taxi_curve: Option<ground_route::GroundRoute>,
    /// Where taxiing leads next (Ready at the runway, Parked at the gate).
    #[serde(skip)]
    taxi_next: Option<State>,
    /// True only after `release()` clears this aircraft to roll onto the runway
    /// and take off; taxiway-queued planes sit in `State::Ready` with this false.
    #[serde(skip)]
    cleared_for_takeoff: bool,
    /// Ground timer: auto-taxi after landing, hold-short before takeoff.
    #[serde(skip)]
    ground_timer: f32,
    /// Set once the aircraft has genuinely landed (or been placed on the deck
    /// via `land()`) and must clear the runway to a gate or arrival slot.
    /// Aircraft seeded on the runway holding for a takeoff clearance stay
    /// `Landed` without this flag, so the tower never auto-clears them off.
    #[serde(skip)]
    must_clear_runway: bool,
    /// Current taxi speed (m/s) for bounded acceleration and braking.
    #[serde(skip)]
    ground_speed: f32,
    /// Set by `World::tick` for arrivals on final while another aircraft uses the runway.
    #[serde(skip)]
    runway_occupied: bool,
    /// Latched TCAS lateral side (±1) so avoidance never flip-flops mid-conflict.
    #[serde(skip)]
    avoid_side: f32,
    /// Last tick's altitude, for vertical-rate estimation in CPA prediction.
    #[serde(skip)]
    prev_alt: f32,
    /// Vertical rate (m/s) from the last tick.
    #[serde(skip)]
    vr: f32,
    /// True once marked done/archived: taxi to Runway 36, take off, and fly off forever.
    #[serde(skip)]
    departing_forever: bool,
    /// Target outbound heading once clear of the runway on a final departure.
    #[serde(skip)]
    departure_heading: Option<f32>,
    /// Smooth TCAS collision-avoidance offset `[dx, dz, dalt]` applied over nominal
    /// flight paths and decayed gently once clear of traffic.
    #[serde(skip)]
    sep_offset: [f32; 3],
    /// Seconds left before `sep_offset` may relax. Held after any conflict so the
    /// offset doesn't decay, re-conflict and bob up and down.
    #[serde(skip)]
    sep_hold: f32,
    /// Low-pass filtered `sep_offset[2]`; the altitude targets use this so TCAS
    /// pushes from several conflicts never make an aircraft bob.
    #[serde(skip)]
    sep_alt: f32,
    /// True after climb-out: fly a smooth transit to the sector's patrol path
    /// instead of snapping onto it.
    #[serde(skip)]
    joining: bool,
    /// Landing: false while inbound to the final approach fix, true on final.
    #[serde(skip)]
    on_final: bool,
    /// Arrival circuit: east entry, base leg, then aligned glide capture.
    #[serde(skip)]
    approach_leg: u8,
    /// Taxiing off the far-end runway exit back to the apron/queue.
    #[serde(skip)]
    exiting_runway: bool,
    /// Arrival hand-off: after rollout, vacate at the far-end exit and park here
    /// `(position, heading)` in `State::Ready` instead of taxiing to a gate.
    #[serde(skip)]
    arrival_slot: Option<([f32; 2], f32)>,
    /// Set by `World::tick` while another aircraft is using the runway.
    #[serde(skip)]
    runway_hold: bool,
    /// Owning airport id (`"local"` for home, peer `environment_id` for satellites).
    #[serde(skip)]
    airport_id: String,
    /// Cached runway & apron layout for the owning airport.
    #[serde(skip)]
    airport_layout: RunwayLayout,
    /// Ground speed in m/s (serde default keeps old snapshots deserialisable).
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_speed() -> f32 {
    AGENT_SPEED
}

/// Sim area is 820x820; positions wrap around at the edges.
const AREA_SIZE: f32 = 820.0;
const TANKER_BASE_ALTITUDE: f32 = 235.0; // high-altitude refuelling tracks above all zones
const TANKER_ALT_STEP: f32 = 24.0;       // vertical separation between provider tankers
const TANKER_SPEED: f32 = 11.5;          // m/s, stately steady refuelling orbit speed
const TANKER_ORBIT_RX: f32 = 255.0;      // wide ellipse sweeping over the zones without wrapping
const TANKER_ORBIT_RZ: f32 = 205.0;
const FRONTIER_ALTITUDE: f32 = 148.0; // high-altitude jet flight levels (148..198)
const LOCAL_ALTITUDE: f32 = 88.0;     // mid-altitude private prop levels (88..128)
const DRONE_ALTITUDE: f32 = 56.0;     // low-mid surveillance drone levels (56..72)
const AGENT_SPEED: f32 = 10.5;        // m/s, calm scenic prop cruise
const FRONTIER_SPEED: f32 = 14.0;     // m/s, majestic high-altitude jet cruise
const DRONE_SPEED: f32 = 5.5;         // m/s, slow Reaper loiter
const FIGHTER_SPEED: f32 = 36.0;      // m/s, wingman catch-up speed
const HOLD_TURN_RADIUS: f32 = 78.0;   // m, wide calm holding orbit
const HOLD_DRIFT_SPEED: f32 = 8.0;    // m/s along the holding orbit

/// Airborne TCAS separation envelope (horizontal & vertical).
const TCAS_HORIZ_BUFFER: f32 = 36.0; // begin smooth sidestep/climb below 36u horizontal
const TCAS_VERT_BUFFER: f32 = 18.0;  // ...and within 18u vertical
const TCAS_HARD_HORIZ: f32 = 28.0;   // hard minimum horizontal if within hard vertical
const TCAS_HARD_VERT: f32 = 12.0;    // hard minimum vertical when crossing horizontally
const CPA_LOOKAHEAD: f32 = 10.0;      // seconds of own velocity predicted for CPA
const CPA_HORIZ_BUFFER: f32 = 26.0;   // predicted horizontal clearance that triggers avoidance
const CPA_VERT_BUFFER: f32 = 14.0;    // ...and vertical
const VERT_HARD_RATE: f32 = 40.0;     // u/s max rate of the hard vertical floor
const TCAS_SOFT_VERT_RATE: f32 = 14.0; // u/s soft vertical separation rate (scaled by urgency)
const SEP_ALT_SMOOTHING: f32 = 1.0;    // per-second low-pass on the vertical TCAS offset
const SEP_HOLD_TIME: f32 = 10.0;       // seconds a separation offset is held after the last conflict
const SEP_DECAY_RATE: f32 = 0.2;       // per-second relaxation of the offset once released
const ALT_EASE_RATE: f32 = 10.0;       // u/s bounded easing rate toward a target altitude

/// Home airport geometry (matches the client's island rendering).
pub const RUNWAY_MID: [f32; 2] = [56.0, -10.0];
pub const RUNWAY_THRESHOLD: [f32; 2] = [56.0, 46.0];
pub const RUNWAY_HEADING: f32 = -std::f32::consts::FRAC_PI_2; // rolling north (-Z)
/// Dedicated parallel Taxiway Alpha between Runway 36 (x=56) and the gate concourse.
pub const TAXIWAY_X: f32 = 84.0;
/// Gate concourse apron lane between Taxiway Alpha (x=84) and the terminal gates (x=120).
pub const APRON_X: f32 = 103.0;
/// Runway Z coordinate where departing aircraft rotate and lift off (runway ends at z=-76).
const RUNWAY_LIFTOFF_Z: f32 = -22.0;
/// Distance from origin at which a departing aircraft has vanished into the alpine horizon.
pub const DEPARTURE_DESPAWN_RADIUS: f32 = 2050.0;
/// Six departure-queue slots along Taxiway Alpha & the south holding bay:
/// `(position [x, z], heading)`. Slot 0 holds short at the runway entrance;
/// slots 1-5 queue nose-to-tail heading south along Taxiway Alpha.
pub const TAXIWAY_QUEUE: [([f32; 2], f32); 6] = [
    ([78.0, 46.0], std::f32::consts::PI),           // holding bay, facing runway
    ([84.0, 22.0], std::f32::consts::FRAC_PI_2),    // Taxiway Alpha #2
    ([84.0, 0.0], std::f32::consts::FRAC_PI_2),     // Taxiway Alpha #3
    ([84.0, -22.0], std::f32::consts::FRAC_PI_2),   // Taxiway Alpha #4
    ([84.0, -44.0], std::f32::consts::FRAC_PI_2),   // Taxiway Alpha #5
    ([84.0, -66.0], std::f32::consts::FRAC_PI_2),   // Taxiway Alpha #6
];
/// Ten terminal gates along the extended eastern concourse pier (for >24h idle threads).
pub const HOME_GATES: [[f32; 2]; 10] = [
    [120.0, -85.5],
    [120.0, -66.5],
    [120.0, -47.5],
    [120.0, -28.5],
    [120.0, -9.5],
    [120.0, 9.5],
    [120.0, 28.5],
    [120.0, 47.5],
    [120.0, 66.5],
    [120.0, 85.5],
];
/// Heading when nose-in at the home terminal (facing east, +X).
pub const GATE_HEADING: f32 = 0.0;

const TAXI_SPEED: f32 = 6.0; // m/s on the ground
const DEPARTURE_TAXI_SPEED: f32 = 14.0; // brisk taxi out for farewell departures
const GROUND_ACCEL: f32 = 4.0; // m/s^2 taxi acceleration
const GROUND_BRAKE: f32 = 6.0; // m/s^2 taxi braking
#[cfg(test)]
const GROUND_SEP: f32 = 14.0; // spacing the departure hold slots keep between queued aircraft
const TAXI_DELAY_AFTER_LANDING: f32 = 1.5; // s paused at the far-end turn-off, then taxi
const HOLD_SHORT_TIME: f32 = 4.0; // s lined up before rolling
/// Altitude at which a climbing aircraft turns toward its sector.
const CLIMB_OUT_ALT: f32 = 45.0;
/// Final approach fix on the extended Runway 36 centerline (south of the island).
const FINAL_APPROACH_FIX: [f32; 2] = [RUNWAY_THRESHOLD[0], RUNWAY_THRESHOLD[1] + 230.0];
/// Altitude crossing the final approach fix.
const APPROACH_ALT: f32 = GLIDE_SLOPE * (FINAL_APPROACH_FIX[1] - TOUCHDOWN_Z - FLARE_DISTANCE / 2.0);
/// Main-gear touchdown point just past the threshold.
const TOUCHDOWN_Z: f32 = RUNWAY_THRESHOLD[1] - 12.0;
/// Far end of Runway 36 in the landing direction; rollout must stop short of it.
const RUNWAY_END_Z: f32 = -76.0;
/// Painted far-end connector, ten units before the end of Runway 36.
const RUNWAY_EXIT_Z: f32 = RUNWAY_END_Z + 10.0;
/// Home arrival return loop: east off the far-end exit to this lane, north
/// past Alpha's last holding slot to this latitude, then east to the apron.
const HOME_EXIT_LANE_X: f32 = 70.0;
const HOME_EXIT_RETURN_Z: f32 = -88.0;
/// Three-degree final, tangent to a parabolic flare over the last 50 units.
const GLIDE_SLOPE: f32 = 0.05240778;
const FLARE_DISTANCE: f32 = 50.0;
const GLIDE_DESCENT_RATE: f32 = 2.0;
const GLIDE_CLIMB_RATE: f32 = 2.0;

fn glide_height(distance: f32) -> f32 {
    let d = distance.max(0.0);
    if d < FLARE_DISTANCE {
        GLIDE_SLOPE * d * d / (2.0 * FLARE_DISTANCE)
    } else {
        GLIDE_SLOPE * (d - FLARE_DISTANCE / 2.0)
    }
}

/// A runway occupancy this close (u) to the touchdown point forces a go-around.
const GOAROUND_BLOCKED_DIST: f32 = 150.0;
/// Satellite final (touchdown to final approach fix) and go-around distance.
const SATELLITE_FINAL_LEN: f32 = 170.0;
const SATELLITE_GOAROUND_DIST: f32 = 100.0;
/// Rollout braking (m/s^2): base wheel braking plus a speed-proportional fade
/// so the stop eases out; the emergency cap guarantees stopping short of the end.
const ROLLOUT_BRAKE_BASE: f32 = 3.0;
const ROLLOUT_BRAKE_FADE: f32 = 0.10;
const ROLLOUT_BRAKE_MAX: f32 = 8.0;

/// Satellite device airport coordinates placed in open water clear of sectors.
pub const SATELLITE_SLOTS: [[f32; 2]; 5] = [
    [-280.0, -250.0], // NW
    [280.0, -245.0],  // NE
    [0.0, 285.0],     // S
    [-285.0, 265.0],  // SW
    [285.0, 265.0],   // SE
];

/// Wrap a coordinate into [-AREA_SIZE/2, AREA_SIZE/2) so the world is
/// centred on the origin (where the home island sits).
pub(crate) fn wrap_centered(v: f32) -> f32 {
    (v + AREA_SIZE / 2.0).rem_euclid(AREA_SIZE) - AREA_SIZE / 2.0
}

/// Base domain sectors always present on the map: (name, centre [x, z], radius).
pub const BASE_SECTORS: &[(&str, [f32; 2], f32)] = &[
    ("Build", [-275.0, 20.0], 86.0),
    ("Homelab", [0.0, -275.0], 86.0),
    ("AI & Models", [285.0, 30.0], 86.0),
    ("Research", [230.0, 245.0], 86.0),
];

/// Open-water slots for dynamic named T3 project zones (radius 74).
pub const PROJECT_SECTOR_SLOTS: [[f32; 2]; 4] = [
    [-225.0, 245.0],  // SW open pocket
    [-145.0, -165.0], // inner NW pocket
    [155.0, -165.0],  // inner NE pocket
    [-145.0, 165.0],  // inner SW pocket
];
pub const PROJECT_SECTOR_RADIUS: f32 = 74.0;

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Canonical provider metadata derived from a model id (and optional T3 provider
/// fallback), matching the livery rules in `web/livery.js`.
/// Returns `(provider_key, display_name, livery_model_hint, callsign)`.
pub fn provider_info(
    model: Option<&str>,
    fallback_provider: Option<&str>,
) -> (&'static str, &'static str, &'static str, &'static str) {
    let raw = model.unwrap_or("").to_lowercase();
    let has_any = |s: &str, needles: &[&str]| needles.iter().any(|n| s.contains(n));

    let key = if has_any(&raw, &["claude", "anthropic", "opus", "sonnet", "haiku", "fable"]) {
        "anthropic"
    } else if has_any(&raw, &["gemini", "gemma", "google", "palm", "bard"]) {
        "google"
    } else if has_any(&raw, &["gpt", "codex", "chatgpt", "openai", "davinci"])
        || is_openai_o_series(&raw)
    {
        "openai"
    } else if raw.contains("deepseek") {
        "deepseek"
    } else if has_any(&raw, &["glm", "zhipu", "z-ai", "zai"]) {
        "zhipu"
    } else if has_any(&raw, &["qwen", "qwq", "alibaba"]) {
        "qwen"
    } else if has_any(&raw, &["mistral", "mixtral", "codestral", "devstral", "magistral"]) {
        "mistral"
    } else if has_any(&raw, &["grok", "xai"]) {
        "xai"
    } else if has_any(&raw, &["llama", "meta"]) {
        "meta"
    } else if has_any(&raw, &["kimi", "moonshot"]) {
        "moonshot"
    } else {
        match fallback_provider.unwrap_or("").to_lowercase().as_str() {
            "claudeagent" | "claude" | "anthropic" => "anthropic",
            "codex" | "openai" => "openai",
            "opencode" => "opencode",
            "antigravity" | "google" | "gemini" => "google",
            "deepseek" => "deepseek",
            "zhipu" | "glm" => "zhipu",
            "qwen" => "qwen",
            "mistral" => "mistral",
            "xai" | "grok" => "xai",
            "meta" | "llama" => "meta",
            "moonshot" | "kimi" => "moonshot",
            _ => "generic",
        }
    };

    provider_descriptor(key)
}

fn is_openai_o_series(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'o'
            && i + 1 < bytes.len()
            && (b'1'..=b'9').contains(&bytes[i + 1])
            && (i == 0 || !bytes[i - 1].is_ascii_alphabetic())
        {
            return true;
        }
    }
    false
}

/// Look up `(provider_key, display_name, livery_model_hint, tanker_callsign)`
/// from a canonical provider key.
pub fn provider_descriptor(key: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match key {
        "anthropic" => ("anthropic", "Claude", "claude", "KC-CLAUDE"),
        "openai" => ("openai", "OpenAI", "openai", "KC-OPENAI"),
        "google" => ("google", "Gemini", "gemini", "KC-GEMINI"),
        "opencode" => ("opencode", "OpenCode", "opencode", "KC-OPENCODE"),
        "deepseek" => ("deepseek", "DeepSeek", "deepseek", "KC-DEEPSEEK"),
        "zhipu" => ("zhipu", "Z.ai", "glm", "KC-ZAI"),
        "qwen" => ("qwen", "Qwen", "qwen", "KC-QWEN"),
        "mistral" => ("mistral", "Mistral", "mistral", "KC-MISTRAL"),
        "xai" => ("xai", "Grok", "grok", "KC-GROK"),
        "meta" => ("meta", "Llama", "llama", "KC-LLAMA"),
        "moonshot" => ("moonshot", "Kimi", "kimi", "KC-KIMI"),
        _ => ("generic", "Token", "generic", "KC-TOKEN"),
    }
}

/// Deterministic orbit rotation and initial phase for a provider tanker so
/// multiple active provider tankers trace staggered tracks above the zones.
fn tanker_orbit_params(provider_key: &str) -> (f32, f32) {
    let mut hash: u32 = 2166136261;
    for b in provider_key.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    let rot = ((hash % 360) as f32).to_radians();
    let phase = (((hash >> 8) % 360) as f32).to_radians();
    (rot, phase)
}

/// Evaluate `[x, z, heading]` along a tanker's sweeping elliptical orbit above the zones.
fn tanker_orbit_point(t: f32, rot: f32) -> ([f32; 2], f32) {
    let (cr, sr) = (rot.cos(), rot.sin());
    let u = TANKER_ORBIT_RX * t.cos();
    let v = TANKER_ORBIT_RZ * t.sin();
    let du = -TANKER_ORBIT_RX * t.sin();
    let dv = TANKER_ORBIT_RZ * t.cos();
    let x = u * cr - v * sr;
    let z = u * sr + v * cr;
    let vx = du * cr - dv * sr;
    let vz = du * sr + dv * cr;
    ([x, z], vz.atan2(vx))
}

impl Aircraft {
    /// New aircraft with a random-ish start position, cruising.
    #[cfg(test)]
    pub fn new(callsign: &str, kind: Kind) -> Self {
        Self::with_model(callsign, kind, ModelClass::Local)
    }

    /// Target cruise flight level based on airframe class (frontier jets fly higher,
    /// with 6 distinct RVSM flight-level slots per class so aircraft naturally stack).
    pub fn cruise_altitude(&self) -> f32 {
        match (self.kind, self.model_class) {
            (Kind::Tanker, _) => {
                TANKER_BASE_ALTITUDE + (self.tanker_orbit_idx as f32) * TANKER_ALT_STEP
            }
            (Kind::Drone, _) => DRONE_ALTITUDE + (self.id % 3) as f32 * 8.0,
            (Kind::Agent, ModelClass::Frontier) => {
                FRONTIER_ALTITUDE + (self.id % 6) as f32 * 10.0
            }
            (Kind::Agent, ModelClass::Local) => LOCAL_ALTITUDE + (self.id % 6) as f32 * 8.0,
            (Kind::Fighter, _) => FRONTIER_ALTITUDE,
        }
    }

    /// New aircraft with an explicit model class.
    pub fn with_model(callsign: &str, kind: Kind, model_class: ModelClass) -> Self {
        let mut rng = rand::thread_rng();
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let speed = match (kind, model_class) {
            (Kind::Tanker, _) => TANKER_SPEED,
            (Kind::Fighter, _) => FIGHTER_SPEED,
            (Kind::Agent, ModelClass::Frontier) => FRONTIER_SPEED,
            (Kind::Agent, ModelClass::Local) => AGENT_SPEED,
            (Kind::Drone, _) => DRONE_SPEED,
        };
        let alt = match (kind, model_class) {
            (Kind::Tanker, _) => TANKER_BASE_ALTITUDE,
            (Kind::Drone, _) => DRONE_ALTITUDE + (id % 3) as f32 * 8.0,
            (Kind::Agent, ModelClass::Frontier) => FRONTIER_ALTITUDE + (id % 6) as f32 * 10.0,
            (Kind::Agent, ModelClass::Local) => LOCAL_ALTITUDE + (id % 6) as f32 * 8.0,
            (Kind::Fighter, _) => FRONTIER_ALTITUDE,
        };
        Aircraft {
            id,
            callsign: callsign.to_string(),
            kind,
            model_class,
            model: None,
            state: State::Cruising,
            position: [
                rng.gen_range(-AREA_SIZE * 0.35..AREA_SIZE * 0.35),
                rng.gen_range(-AREA_SIZE * 0.35..AREA_SIZE * 0.35),
                alt,
            ],
            heading: rng.gen_range(0.0..std::f32::consts::TAU),
            altitude: alt,
            hold_reason: None,
            topic: None,
            title: None,
            device: None,
            environment_id: None,
            thread_id: None,
            chat_links: None,
            data_stale: false,
            parent_id: None,
            wing_slot: None,
            provider: None,
            tanker_id: None,
            refuel_connected: false,
            refuel_scope: None,
            refuel_slot: None,
            refuel_reset_at: None,
            scheduled_job: None,
            ramp_slot: None,
            job_target: reaper::JobTarget::Park,
            park_heading: None,
            tanker_orbit_idx: 0,
            orbit_center: None,
            patrol_angle: rng.gen_range(0.0..std::f32::consts::TAU),
            taxi_waypoints: Vec::new(),
            taxi_curve: None,
            taxi_next: None,
            cleared_for_takeoff: false,
            ground_timer: 0.0,
            must_clear_runway: false,
            ground_speed: 0.0,
            runway_occupied: false,
            avoid_side: 0.0,
            prev_alt: alt,
            vr: 0.0,
            departing_forever: false,
            departure_heading: None,
            sep_offset: [0.0, 0.0, 0.0],
            sep_hold: 0.0,
            sep_alt: 0.0,
            joining: false,
            on_final: false,
            approach_leg: 0,
            exiting_runway: false,
            landing_pitch: None,
            arrival_slot: None,
            runway_hold: false,
            airport_id: "local".to_string(),
            airport_layout: RunwayLayout::home(),
            speed,
        }
    }

    /// Bind this aircraft's ground and runway operations to `airport`.
    pub fn set_airport(&mut self, airport: &Airport) {
        self.airport_id = airport.id.clone();
        self.airport_layout = airport.runway_layout();
    }

    pub fn airport_id(&self) -> &str {
        if self.airport_id.is_empty() {
            "local"
        } else {
            &self.airport_id
        }
    }

    /// Spawn a provider aerial refuelling tanker orbiting high above the airspace zones.
    fn tanker(provider_key: &str, track_key: &str, orbit_idx: usize) -> Self {
        let (key, display, livery_hint, callsign) = provider_descriptor(provider_key);
        let (rot, phase) = tanker_orbit_params(track_key);
        let ([x, z], heading) = tanker_orbit_point(phase, rot);
        let alt = TANKER_BASE_ALTITUDE + (orbit_idx as f32) * TANKER_ALT_STEP;
        let mut a = Self::with_model(callsign, Kind::Tanker, ModelClass::Frontier);
        a.model = Some(livery_hint.to_string());
        a.provider = Some(key.to_string());
        a.title = Some(format!("{display} Tanker"));
        a.state = State::Refueling;
        a.position = [x, z, alt];
        a.altitude = alt;
        a.heading = heading;
        a.patrol_angle = phase;
        a.tanker_orbit_idx = orbit_idx;
        a.refuel_scope = Some(track_key.to_owned());
        a.speed = TANKER_SPEED;
        a
    }

    /// Nominal cruise speed for this airframe class.
    pub fn cruise_speed(&self) -> f32 {
        match (self.kind, self.model_class) {
            (Kind::Tanker, _) => TANKER_SPEED,
            (Kind::Fighter, _) => FIGHTER_SPEED,
            (Kind::Agent, ModelClass::Frontier) => FRONTIER_SPEED,
            (Kind::Agent, ModelClass::Local) => AGENT_SPEED,
            (Kind::Drone, _) => DRONE_SPEED,
        }
    }

    fn liftoff_speed(&self) -> f32 {
        match (self.kind, self.model_class) {
            (Kind::Agent, ModelClass::Frontier) => 34.0,
            _ => 25.0,
        }
    }

    fn climb_rate(&self) -> f32 {
        match (self.kind, self.model_class) {
            (Kind::Agent, ModelClass::Frontier) => 16.5,
            _ => 13.0,
        }
    }

    fn approach_speed(&self) -> f32 {
        match (self.kind, self.model_class) {
            (Kind::Agent, ModelClass::Frontier) => 26.0,
            _ => 21.0,
        }
    }

    /// True while mid takeoff or landing sequence, so pollers should not
    /// overwrite the state with a steady-state one.
    pub fn in_runway_sequence(&self) -> bool {
        match self.state {
            State::Takeoff | State::Landing | State::Landed => true,
            State::Taxi | State::Ready => self.cleared_for_takeoff || self.arrival_slot.is_some(),
            _ => false,
        }
    }

    /// Fly an approach to Runway 36, touch down, roll out and taxi via the north exit to
    /// `slot` (if any). Returns false if not airborne.
    pub fn begin_approach(&mut self, slot: Option<([f32; 2], f32)>) -> bool {
        if !matches!(
            self.state,
            State::Cruising | State::Holding | State::Refueling | State::Takeoff | State::Landing
        ) || (self.state == State::Takeoff && self.altitude <= 0.0)
        {
            return false;
        }
        if self.state != State::Landing {
            self.on_final = false;
            self.approach_leg = 0;
            self.landing_pitch = None;
        }
        self.state = State::Landing;
        self.hold_reason = None;
        self.orbit_center = None;
        self.tanker_id = None;
        self.refuel_connected = false;
        self.refuel_slot = None;
        self.refuel_reset_at = None;
        self.joining = false;
        self.cleared_for_takeoff = false;
        self.arrival_slot = slot;
        true
    }

    /// Abort an arrival and climb back out to the sector (thread started again).
    pub fn go_around(&mut self) {
        self.arrival_slot = None;
        self.landing_pitch = None;
        if self.state == State::Landing {
            self.state = State::Cruising;
            self.joining = true;
        } else if matches!(self.state, State::Landed | State::Taxi | State::Ready) {
            self.release();
        }
    }

    /// True while rolling, on short final, or backtracking on this aircraft's runway.
    fn occupies_runway(&self) -> bool {
        let layout = self.airport_layout;
        match self.state {
            State::Takeoff => self.altitude < 20.0,
            State::Landing => {
                if !self.on_final {
                    return false;
                }
                let dz = wrap_centered(self.position[1] - layout.touchdown_z);
                dz > -20.0 && dz < 120.0
            }
            State::Landed => true,
            State::Departing => self.altitude <= 0.0,
            // Inbound departure taxis must not circularly hold a Ready departure.
            // Arrivals backtracking to a gate or arrival slot still protect the runway.
            State::Taxi => {
                !(self.taxi_next == Some(State::Ready)
                    && self.cleared_for_takeoff
                    && !self.must_clear_runway
                    && self.arrival_slot.is_none())
                    && (self.position[0] - layout.runway_x).abs() < 8.0
                    && self.position[1] <= layout.threshold[1] + 6.0
                    && self.position[1] >= layout.end_z
            }
            _ => false,
        }
    }

    /// The arrival slot this aircraft is heading for, if any.
    pub fn arrival_slot(&self) -> Option<([f32; 2], f32)> {
        self.arrival_slot
    }

    pub fn set_arrival_slot(&mut self, slot: Option<([f32; 2], f32)>) {
        self.arrival_slot = slot;
    }

    /// Steer heading toward `desired` by at most `max_turn` radians.
    fn turn_toward(&mut self, desired: f32, max_turn: f32) {
        let mut dh = desired - self.heading;
        while dh > std::f32::consts::PI {
            dh -= std::f32::consts::TAU;
        }
        while dh < -std::f32::consts::PI {
            dh += std::f32::consts::TAU;
        }
        self.heading += dh.clamp(-max_turn, max_turn);
    }

    /// Fly straight along the current heading at the current speed.
    fn advance(&mut self, dt: f32) {
        self.position[0] = wrap_centered(self.position[0] + self.heading.cos() * self.speed * dt);
        self.position[1] = wrap_centered(self.position[1] + self.heading.sin() * self.speed * dt);
    }

    /// Fly the runway approach for this aircraft's airport: inbound to the final
    /// approach fix, then pursue the extended centerline down a glidepath to
    /// touchdown. A badly lined-up approach goes around and tries again.
    fn fly_approach(&mut self, dt: f32) {
        let layout = self.airport_layout;
        let target_speed = self.approach_speed();
        self.speed += (target_speed - self.speed).clamp(-4.0 * dt, 4.0 * dt);
        // All approach geometry is flown in a chart centred on this airport's
        // touchdown point (torus-safe offsets), so satellites near the wrap
        // boundary fly exactly the same circuit, glide and flare as home.
        let lx = wrap_centered(self.position[0] - layout.runway_x);
        let lz = wrap_centered(self.position[1] - layout.touchdown_z);
        let final_len = layout.final_len();
        let goaround_dist = layout.goaround_dist();

        let old_alt = self.altitude;
        if !self.on_final {
            // Fly a circuit rather than diving directly at the runway from an
            // arbitrary heading/height. Repeat it if still above glide capture.
            let target = match self.approach_leg {
                0 => [140.0, final_len + 70.0],
                1 => [0.0, final_len + 70.0],
                _ => [0.0, lz - 70.0],
            };
            let dx = target[0] - lx;
            let dz = target[1] - lz;
            let dist = dx.hypot(dz);
            self.turn_toward(dz.atan2(dx), 0.65 * dt);
            let target_alt = if self.approach_leg == 2 {
                glide_height(lz + self.heading.sin() * self.speed * dt)
            } else {
                layout.approach_alt
            };
            let vertical_limit = self.speed * GLIDE_SLOPE;
            self.altitude += (target_alt - self.altitude)
                .clamp(-vertical_limit * dt, vertical_limit * dt);
            if self.approach_leg < 2 && dist < 35.0 {
                self.approach_leg += 1;
            }
            if self.approach_leg == 2 {
                let aligned = (self.heading - RUNWAY_HEADING).sin().abs() < 0.12
                    && self.heading.sin() < -0.98
                    && lx.abs() < 5.0;
                if aligned && (self.altitude - target_alt).abs() < 0.001 && lz <= final_len {
                    self.on_final = true;
                } else if lz < final_len - 45.0 {
                    self.approach_leg = 0;
                }
            }
        } else {
            // Pure pursuit of a point on the centerline ahead lines us up smoothly.
            let look_ahead = if layout.is_home { 70.0 } else { 42.0 };
            let aim_dz = (lz - look_ahead).max(-60.0) - lz;
            let align_rate = if layout.is_home { 0.9 } else { 1.6 };
            self.turn_toward(aim_dz.atan2(-lx), align_rate * dt);
            let frac = (lz / final_len).clamp(0.0, 1.0);
            let mut dh = self.heading - RUNWAY_HEADING;
            while dh > std::f32::consts::PI {
                dh -= std::f32::consts::TAU;
            }
            while dh < -std::f32::consts::PI {
                dh += std::f32::consts::TAU;
            }
            // Blocked runway: go around well before the threshold. The inbound
            // phase re-captures the fix; the window is finite, so a still-blocked
            // runway just starts the cycle again instead of stalling.
            if self.runway_occupied && lz < goaround_dist {
                self.on_final = false;
            } else if frac < 0.5
                && (lx.abs() > 18.0 + 12.0 * frac
                    || dh.abs() > 0.3 + 0.6 * frac)
            {
                // Committed to the approach and no longer recoverable: go
                // around continuously. Tolerances stay well outside what the
                // pure-pursuit alignment can still correct (it has the rest of
                // final plus the threshold gate to finish the job), so a
                // normal capture never flaps into a go-around cycle.
                self.on_final = false;
            }
            if self.on_final {
                // Three-degree glide, tangent to a parabolic flare, so
                // the descent rate eases to zero at the touchdown point. The
                // correction is bounded and works both ways — a too-low
                // approach climbs back to the path instead of clipping short.
                let desired = glide_height(lz + self.heading.sin() * self.speed * dt);
                self.altitude += (desired - self.altitude)
                    .clamp(-GLIDE_DESCENT_RATE * dt, GLIDE_CLIMB_RATE * dt);
                if lz <= 1.0 {
                    // The pursuit oscillation damps out slowly, so the last
                    // gate is deliberately forgiving — a 6u / 0.25 rad
                    // touchdown is still well inside the runway.
                    let lined_up = lx.abs() < 6.0 && dh.abs() < 0.25;
                    if lined_up && self.altitude < 0.02 && !self.runway_occupied {
                        // Touchdown: roll out along the centerline. The flare
                        // already has us on the deck — no position snap; the
                        // Landed state eases the heading onto the centerline.
                        self.landing_pitch = Some(4.0_f32.to_radians());
                        self.state = State::Landed;
                        self.must_clear_runway = true;
                        self.on_final = false;
                        self.altitude = 0.0;
                        self.position[0] = wrap_centered(self.position[0]);
                        self.position[1] = wrap_centered(self.position[1]);
                        self.position[2] = 0.0;
                        self.ground_timer = 0.0;
                        return;
                    }
                    // Not lined up: go around and fly the approach again.
                    self.on_final = false;
                }
            }
        }
        if !self.on_final && self.approach_leg == 2 && lz < goaround_dist {
            self.approach_leg = 0;
        }
        let flight_angle = ((self.altitude - old_alt) / (self.speed * dt).max(0.001)).atan();
        let flare = if self.on_final {
            (1.0 - lz / FLARE_DISTANCE).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.landing_pitch = Some(flight_angle * (1.0 - flare) + 4.0_f32.to_radians() * flare);
        self.advance_in_airport_chart(dt);
        self.position[2] = self.altitude;
    }

    /// Fly straight in the approach chart of this aircraft's airport. The
    /// circuit is physical space, so an aircraft on it never teleports across
    /// the patrol-map edge (it may fly past the edge over open water). An
    /// aircraft whose shortest way to the airport crosses the map edge still
    /// wraps once, at the edge, onto the airport's side.
    fn advance_in_airport_chart(&mut self, dt: f32) {
        let layout = self.airport_layout;
        let origin = [layout.runway_x, layout.touchdown_z];
        let step = [self.heading.cos() * self.speed * dt, self.heading.sin() * self.speed * dt];
        for axis in 0..2 {
            let p = self.position[axis] + step[axis];
            let rel = p - origin[axis];
            self.position[axis] = if p.abs() > AREA_SIZE / 2.0 && rel.abs() > AREA_SIZE / 2.0 {
                origin[axis] + wrap_centered(rel)
            } else {
                p
            };
        }
    }

    /// Fly a smooth transit onto a moving patrol-path point after climb-out.
    /// Returns true while still joining (position already updated).
    fn join_path(&mut self, target: [f32; 2], target_alt: f32, path_heading: f32, dt: f32) -> bool {
        let dx = wrap_centered(target[0] - self.position[0]);
        let dz = wrap_centered(target[1] - self.position[1]);
        let dist = dx.hypot(dz);
        // Follow the moving route with bounded physical motion, never copy its coordinates.
        let cruise = self.cruise_speed();
        let transit = cruise * (1.0 + (dist / 180.0).clamp(0.0, 0.65));
        self.speed += (transit - self.speed).clamp(-5.0 * dt, 5.0 * dt);
        self.altitude += (target_alt - self.altitude).clamp(-ALT_EASE_RATE * dt, ALT_EASE_RATE * dt);
        let heading = if dist > 4.0 { dz.atan2(dx) } else { path_heading };
        self.turn_toward(heading, 0.65 * dt);
        self.advance(dt);
        self.joining = dist > 20.0 || (target_alt - self.altitude).abs() > 2.0;
        self.position[2] = self.altitude;
        true
    }

    /// Spawn a fighter wingman escorting `parent_id` in formation slot `slot`.
    pub fn wingman(callsign: &str, parent_id: u64, slot: usize, start_pos: [f32; 3], heading: f32) -> Self {
        let mut a = Self::with_model(callsign, Kind::Fighter, ModelClass::Frontier);
        a.parent_id = Some(parent_id);
        a.wing_slot = Some(slot);
        a.position = start_pos;
        a.altitude = start_pos[2];
        a.heading = heading;
        a
    }

    /// Assign this aircraft to a topic sector.
    pub fn assign(&mut self, topic: &str) {
        self.topic = Some(topic.to_string());
    }

    /// Enter a holding pattern with a reason (e.g. "awaiting approval").
    pub fn hold(&mut self, reason: &str) {
        if self.state != State::Holding {
            self.orbit_center = None; // re-derive the orbit from current pos
        }
        self.state = State::Holding;
        self.hold_reason = Some(reason.to_string());
        self.tanker_id = None;
        self.refuel_connected = false;
        self.refuel_slot = None;
        self.refuel_reset_at = None;
    }

    /// Connect to the provider's aerial refuelling tanker while waiting for
    /// tokens / quota to replenish.
    pub fn refuel(&mut self, reason: &str, reset_at: Option<String>) {
        self.state = State::Refueling;
        self.hold_reason = Some(reason.to_string());
        self.refuel_reset_at = reset_at;
        self.orbit_center = None;
        self.joining = false;
        self.on_final = false;
        self.arrival_slot = None;
        self.cleared_for_takeoff = false;
        self.taxi_waypoints.clear();
        self.taxi_curve = None;
        self.taxi_next = None;
        if self.provider.is_none() {
            let (key, _, _, _) = provider_info(self.model.as_deref(), None);
            self.provider = Some(key.to_string());
        }
    }

    /// Start taxiing to `dest` via the airport's taxiway/apron centerline so
    /// aircraft never cut diagonally across parked planes at neighboring gates.
    pub fn taxi_to(&mut self, dest: [f32; 2], next_state: State) {
        self.exiting_runway = false;
        self.state = State::Taxi;
        self.taxi_next = Some(next_state);
        self.ground_timer = 0.0;
        self.taxi_waypoints.clear();
        self.taxi_curve = None;

        let cur = [self.position[0], self.position[1]];
        let lane_x = self.airport_layout.taxiway_x;
        // Route via the airport's taxiway/apron lane unless already on the same lane
        // (except gate-to-gate moves on a satellite apron, which pull out to apron_x).
        let same_lane = (cur[0] - dest[0]).abs() < 3.0
            && (self.airport_layout.is_home || (cur[0] - lane_x).abs() < 3.0);
        if !same_lane {
            if (cur[0] - lane_x).abs() > 2.0 {
                self.taxi_waypoints.push([lane_x, cur[1]]);
            }
            if (cur[1] - dest[1]).abs() > 2.0 {
                self.taxi_waypoints.push([lane_x, dest[1]]);
            }
        }
        self.taxi_waypoints.push(dest);
    }

    /// Arrivals use the far-end bridge and apron lane, bypassing queued departures.
    fn taxi_from_runway(&mut self, dest: [f32; 2], next: State) {
        self.taxi_to(dest, next);
        self.ground_speed = self.speed.min(TAXI_SPEED);
        self.exiting_runway = true;
        self.taxi_waypoints = self.airport_layout.far_exit_route(dest);
    }

    /// Follow the same tangent route for arrivals, departures and ramp traffic.
    fn tick_ground_route(&mut self, dt: f32) {
        self.altitude = 0.0;
        self.position[2] = 0.0;
        if self.taxi_curve.is_none() {
            let mut curve = ground_route::GroundRoute::new(
                [self.position[0], self.position[1]],
                &self.taxi_waypoints,
                self.airport_layout.is_home.then_some(APRON_X),
            );
            let next = self.taxi_next.unwrap_or(State::Parked);
            let queue_heading = self.taxi_waypoints.last().and_then(|dest| {
                TAXIWAY_QUEUE
                    .iter()
                    .find(|(p, _)| p == dest)
                    .map(|(_, h)| *h)
            });
            let end_heading = self
                .park_heading
                .filter(|_| {
                    next == State::Parked || (next == State::Ready && !self.cleared_for_takeoff)
                })
                .or(self.arrival_slot.map(|(_, h)| h))
                .or(queue_heading)
                .unwrap_or(if matches!(next, State::Ready | State::Departing) {
                    RUNWAY_HEADING
                } else {
                    GATE_HEADING
                });
            curve.align(self.heading, end_heading);
            self.taxi_curve = Some(curve);
        }
        let curve = self.taxi_curve.as_mut().unwrap();
        let max = if self.departing_forever {
            DEPARTURE_TAXI_SPEED
        } else {
            TAXI_SPEED
        };
        let remaining = curve.remaining().max(0.0);
        let want = curve.speed_limit(max, GROUND_BRAKE).min(
            (2.0 * GROUND_BRAKE * remaining + (GROUND_BRAKE * dt).powi(2)).sqrt()
                - GROUND_BRAKE * dt,
        );
        self.ground_speed +=
            (want - self.ground_speed).clamp(-GROUND_BRAKE * dt, GROUND_ACCEL * dt);
        let (p, h, done) = curve.advance((self.ground_speed * dt).min(remaining));
        self.position[0] = p[0];
        self.position[1] = p[1];
        self.heading = h;
        self.speed = self.ground_speed;
        if done || remaining < 0.002 {
            if let Some(dest) = self.taxi_waypoints.last() {
                self.position[0] = dest[0];
                self.position[1] = dest[1];
            }
            self.taxi_waypoints.clear();
            self.taxi_curve = None;
            self.state = self.taxi_next.take().unwrap_or(State::Parked);
            self.exiting_runway = false;
            self.ground_timer = 0.0;
            self.ground_speed = 0.0;
            self.speed = if self.state == State::Departing {
                6.0
            } else {
                0.0
            };
        }
    }

    /// The final destination of any in-progress taxi, if taxiing.
    pub fn taxi_destination(&self) -> Option<[f32; 2]> {
        self.taxi_waypoints.last().copied()
    }

    /// Cleared for takeoff: taxi out via the taxiway, hold short, then roll.
    pub fn release(&mut self) {
        self.orbit_center = None;
        self.hold_reason = None;
        self.tanker_id = None;
        self.refuel_connected = false;
        self.refuel_slot = None;
        self.refuel_reset_at = None;
        self.cleared_for_takeoff = true;
        self.arrival_slot = None;
        self.exiting_runway = false;
        match self.state {
            // On the ground: taxi to the runway threshold, hold short, then roll.
            State::Parked | State::Landed | State::Taxi | State::Ready => {
                self.must_clear_runway = false;
                let thresh = self.airport_layout.threshold;
                let d = (self.position[0] - thresh[0]).hypot(self.position[1] - thresh[1]);
                self.speed = 0.0;
                if d < 6.0 {
                    self.state = State::Ready;
                    self.heading = RUNWAY_HEADING;
                    self.ground_timer = 0.0;
                } else {
                    self.route_to_runway(State::Ready);
                }
            }
            // Refuelling high above the zones: disconnect and smoothly join patrol.
            State::Refueling => {
                self.cleared_for_takeoff = false;
                self.state = State::Cruising;
                self.joining = true;
            }
            // Airborne hold: climb straight out, then join continuously.
            _ => self.state = State::Takeoff,
        }
    }

    /// Taxi to the airport's runway threshold along the apron lane / taxiway.
    fn route_to_runway(&mut self, next: State) {
        self.exiting_runway = false;
        self.state = State::Taxi;
        self.taxi_next = Some(next);
        self.ground_timer = 0.0;
        self.taxi_waypoints.clear();
        self.taxi_curve = None;
        let layout = self.airport_layout;
        let thresh = layout.threshold;
        // Off the Reaper ramp first, onto the lane that leads to the runway.
        let (exit, cur) = layout.ramp_exit([self.position[0], self.position[1]]);
        self.taxi_waypoints.extend(exit);
        if layout.is_home {
            if cur[0] > 96.0 {
                // Gate concourse: pull back onto apron centerline, south to connector, west to runway.
                if (cur[0] - layout.apron_x).abs() > 2.0 {
                    self.taxi_waypoints.push([layout.apron_x, cur[1]]);
                }
                if (cur[1] - thresh[1]).abs() > 2.0 {
                    self.taxi_waypoints.push([layout.apron_x, thresh[1]]);
                }
                self.taxi_waypoints.push([layout.taxiway_x, thresh[1]]);
            } else if cur[0] > 64.0 {
                // Taxiway Alpha or south holding bay.
                if (cur[1] - thresh[1]).abs() > 2.0 {
                    if (cur[0] - layout.taxiway_x).abs() > 2.0 {
                        self.taxi_waypoints.push([layout.taxiway_x, cur[1]]);
                    }
                    self.taxi_waypoints.push([layout.taxiway_x, thresh[1]]);
                }
            }
        } else if cur[0] > layout.runway_x + 6.0 {
            // Satellite apron: pull out onto apron lane, south to threshold, west onto runway.
            if (cur[0] - layout.apron_x).abs() > 2.0 {
                self.taxi_waypoints.push([layout.apron_x, cur[1]]);
            }
            if (cur[1] - thresh[1]).abs() > 2.0 {
                self.taxi_waypoints.push([layout.apron_x, thresh[1]]);
            }
        }
        // From the runway itself this simply backtracks down the centerline.
        self.taxi_waypoints.push(thresh);
    }

    /// Common on-deck state for an aircraft in the Landed state.
    fn start_landed(&mut self) {
        self.exiting_runway = false;
        self.state = State::Landed;
        self.speed = 0.0;
        self.on_final = false;
        self.joining = false;
        self.arrival_slot = None;
        self.altitude = 0.0;
        self.position[2] = 0.0;
        self.hold_reason = None;
        self.orbit_center = None;
        self.tanker_id = None;
        self.refuel_connected = false;
        self.refuel_slot = None;
        self.refuel_reset_at = None;
        self.cleared_for_takeoff = false;
        self.departing_forever = false;
        self.departure_heading = None;
        self.taxi_waypoints.clear();
        self.taxi_curve = None;
        self.taxi_next = None;
        self.ground_timer = 0.0;
    }

    /// Seeded on the runway holding for a takeoff clearance: hold position in
    /// the Landed state until `release()` clears it. World::tick must not
    /// assign such an aircraft a gate or arrival slot.
    pub fn hold_on_runway(&mut self) {
        self.start_landed();
        self.must_clear_runway = false;
        self.heading = RUNWAY_HEADING;
    }

    /// True if this aircraft is taxiing out for or flying a final farewell departure.
    pub fn is_departing(&self) -> bool {
        self.departing_forever || self.state == State::Departing
    }

    /// Thread marked done / archived: taxi out to Runway 36 (if on the ground),
    /// take off, climb over the alpine peaks, and fly into the horizon forever.
    pub fn depart(&mut self) {
        if self.is_departing() {
            return;
        }
        self.departing_forever = true;
        self.hold_reason = None;
        self.orbit_center = None;
        self.tanker_id = None;
        self.refuel_connected = false;
        self.refuel_slot = None;
        self.refuel_reset_at = None;
        self.cleared_for_takeoff = false;
        self.topic = None;
        self.taxi_waypoints.clear();
        self.taxi_curve = None;
        self.taxi_next = None;
        self.arrival_slot = None;
        self.joining = false;

        match self.state {
            State::Takeoff if self.altitude <= 0.0 => {
                // Mid ground roll: just keep rolling straight out.
                let spread = ((self.id % 7) as f32 - 2.5) * 0.14;
                self.departure_heading = Some(RUNWAY_HEADING + spread);
                self.state = State::Departing;
                self.ground_timer = 0.0;
            }
            State::Cruising | State::Holding | State::Refueling | State::Takeoff | State::Landing => {
                // Already airborne: bank outward away from the island and climb into the horizon.
                let radial = if self.position[0].hypot(self.position[1]) > 20.0 {
                    self.position[1].atan2(self.position[0])
                } else {
                    self.heading
                };
                self.departure_heading = Some(radial);
                self.state = State::Departing;
                self.altitude = self.altitude.max(15.0);
                self.position[2] = self.altitude;
                self.ground_timer = 10.0; // skip ground roll
                self.speed = self.speed.max(26.0);
            }
            State::Parked | State::Ready | State::Landed | State::Taxi | State::Departing => {
                // Scenic outbound spread off Runway 36 (north-northeast / north-northwest).
                let spread = ((self.id % 7) as f32 - 2.5) * 0.14;
                self.departure_heading = Some(RUNWAY_HEADING + spread);

                let cur = [self.position[0], self.position[1]];
                let layout = self.airport_layout;
                let dist_to_thresh =
                    (cur[0] - layout.threshold[0]).hypot(cur[1] - layout.threshold[1]);
                let on_runway = (cur[0] - layout.runway_x).abs() < 8.0
                    && cur[1] <= layout.threshold[1] + 6.0
                    && cur[1] >= layout.end_z - 6.0;

                if dist_to_thresh < 6.0 || on_runway {
                    // Already lined up or on the runway: roll straight north for takeoff.
                    self.state = State::Departing;
                    self.heading = RUNWAY_HEADING;
                    self.altitude = 0.0;
                    self.position[2] = 0.0;
                    self.ground_timer = 0.0;
                    self.speed = 6.0;
                    return;
                }

                // Route cleanly along the airport's apron lane / taxiway to its runway threshold.
                self.route_to_runway(State::Departing);
            }
        }
    }

    /// Advance the simulation by `dt` seconds (for non-wingman aircraft).
    #[cfg(test)]
    pub fn tick(&mut self, dt: f32) {
        self.tick_with_sectors(dt, &default_sectors());
    }

    /// Advance the simulation by `dt` seconds using the current world sectors.
    pub fn tick_with_sectors(&mut self, dt: f32, sectors: &[Sector]) {
        if self.kind == Kind::Tanker && self.state == State::Cruising {
            // Provider aerial refuelling tanker: smooth, stately elliptical track
            // high above all airspace zones, never wrapping across map edges.
            let key = self.refuel_scope.as_deref().unwrap_or("generic");
            let (rot, _) = tanker_orbit_params(key);
            let t = self.patrol_angle;
            let du = -TANKER_ORBIT_RX * t.sin();
            let dv = TANKER_ORBIT_RZ * t.cos();
            let ds_dt = du.hypot(dv).max(1.0);
            self.patrol_angle =
                (self.patrol_angle + (TANKER_SPEED / ds_dt) * dt) % std::f32::consts::TAU;
            let ([x, z], heading) = tanker_orbit_point(self.patrol_angle, rot);
            let target_alt = self.cruise_altitude() + self.sep_alt;
            self.altitude +=
                (target_alt - self.altitude).clamp(-ALT_EASE_RATE * dt, ALT_EASE_RATE * dt);
            self.position = [wrap_centered(x + self.sep_offset[0]), wrap_centered(z + self.sep_offset[1]), self.altitude];
            self.heading = heading;
            self.speed = TANKER_SPEED;
            return;
        }

        match self.state {
            State::Refueling => {
                // Driven in formation by `World::tick` once linked to a provider tanker;
                // if ticked standalone (e.g. in unit tests), hold a calm high orbit.
                let target_alt = TANKER_BASE_ALTITUDE - 10.0;
                self.altitude +=
                    (target_alt - self.altitude).clamp(-ALT_EASE_RATE * dt, ALT_EASE_RATE * dt);
                self.position[2] = self.altitude;
                self.advance(dt);
            }
            State::Holding => {
                if self.orbit_center.is_none() {
                    let r = HOLD_TURN_RADIUS;
                    let cx = self.position[0] - self.heading.sin() * r;
                    let cy = self.position[1] + self.heading.cos() * r;
                    self.orbit_center = Some([cx, cy]);
                }
                let [cx, cy] = self.orbit_center.unwrap();
                let angle = (self.position[1] - cy).atan2(self.position[0] - cx);
                let omega = HOLD_DRIFT_SPEED / HOLD_TURN_RADIUS;
                let angle = angle + omega * dt;
                let nx = cx + angle.cos() * HOLD_TURN_RADIUS;
                let ny = cy + angle.sin() * HOLD_TURN_RADIUS;
                self.heading = angle + std::f32::consts::FRAC_PI_2;
                self.position[0] = wrap_centered(nx);
                self.position[1] = wrap_centered(ny);
                let hold_alt = self.cruise_altitude();
                self.altitude += (hold_alt-self.altitude).clamp(-10.0*dt,10.0*dt);
                self.position[2] = self.altitude;
            }
            State::Cruising => {
                let base_alt = self.cruise_altitude();

                if let Some(topic) = &self.topic {
                    if let Some(sec) = sectors.iter().find(|s| s.name == *topic) {
                        // Sweeping, calm parametric flight paths instead of tight circles:
                        // Frontier jets carve wide high-altitude figure-8s / arcs; props &
                        // drones fly scenic ovals and lazy S-curves at constant true airspeed.
                        let span = match (self.kind, self.model_class) {
                            (Kind::Agent, ModelClass::Frontier) => sec.radius * 1.18,
                            (Kind::Drone, _) => sec.radius * 0.78,
                            _ => sec.radius * 0.88,
                        };
                        let rot = (self.id as f32 * 1.17) % std::f32::consts::TAU;
                        let (cr, sr) = (rot.cos(), rot.sin());
                        let pattern = if self.kind == Kind::Drone {
                            1
                        } else {
                            self.id % 3
                        };

                        let t = self.patrol_angle;
                        let (du, dv) = match pattern {
                            // Pattern 0: wide figure-8 (Lissajous 1:2)
                            0 => {
                                let a = span * 1.12;
                                let b = span * 0.56;
                                (a * t.cos(), 2.0 * b * (2.0 * t).cos())
                            }
                            // Pattern 1: broad tilted scenic oval
                            1 => {
                                let a = span * 1.15;
                                let b = span * 0.68;
                                (-a * t.sin(), b * t.cos())
                            }
                            // Pattern 2: gentle three-lobe scenic S-loop
                            _ => {
                                let a = span * 0.88;
                                (
                                    a * (-t.sin() - 0.66 * (3.0 * t).sin()),
                                    a * (t.cos() - 0.66 * (3.0 * t).cos()),
                                )
                            }
                        };
                        let ds_dt = du.hypot(dv).max(1.0);
                        self.patrol_angle = (self.patrol_angle + (self.cruise_speed() / ds_dt) * dt)
                            % std::f32::consts::TAU;

                        let t = self.patrol_angle;
                        let (u, v, du, dv) = match pattern {
                            0 => {
                                let a = span * 1.12;
                                let b = span * 0.56;
                                (
                                    a * t.sin(),
                                    b * (2.0 * t).sin(),
                                    a * t.cos(),
                                    2.0 * b * (2.0 * t).cos(),
                                )
                            }
                            1 => {
                                let a = span * 1.15;
                                let b = span * 0.68;
                                (a * t.cos(), b * t.sin(), -a * t.sin(), b * t.cos())
                            }
                            _ => {
                                let a = span * 0.88;
                                (
                                    a * (t.cos() + 0.22 * (3.0 * t).cos()),
                                    a * (t.sin() - 0.22 * (3.0 * t).sin()),
                                    a * (-t.sin() - 0.66 * (3.0 * t).sin()),
                                    a * (t.cos() - 0.66 * (3.0 * t).cos()),
                                )
                            }
                        };

                        let ox = u * cr - v * sr;
                        let oz = u * sr + v * cr;
                        let vx = du * cr - dv * sr;
                        let vz = du * sr + dv * cr;

                        let path_heading = vz.atan2(vx);
                        let target = [
                            sec.center[0] + ox + self.sep_offset[0],
                            sec.center[1] + oz + self.sep_offset[1],
                        ];
                        // Gentle vertical breath while cruising + TCAS altitude offset.
                        let wave = 2.5 * (2.0 * t).sin();
                        let target_alt = (base_alt + wave + self.sep_alt).max(30.0);
                        self.join_path(target, target_alt, path_heading, dt);
                        return;
                    }
                }
                // Ease to the (possibly TCAS-offset) cruise altitude; never snap.
                self.altitude += (base_alt + self.sep_alt - self.altitude)
                    .clamp(-ALT_EASE_RATE * dt, ALT_EASE_RATE * dt);
                if self.joining {
                    // Unassigned: still accelerating to cruise speed on the current heading.
                    self.speed += (self.cruise_speed() - self.speed).clamp(-5.0 * dt, 5.0 * dt);
                    self.joining = (base_alt - self.altitude).abs() > 0.5;
                }
                self.position[2] = self.altitude;
                let dx = self.heading.cos() * self.speed * dt;
                let dy = self.heading.sin() * self.speed * dt;
                self.position[0] = wrap_centered(self.position[0] + dx);
                self.position[1] = wrap_centered(self.position[1] + dy);
            }
            State::Landed => {
                self.landing_pitch = None;
                // Decelerate down the runway; once stopped, World::tick assigns a
                // free gate (or the arrival slot) after the landing pause.
                self.altitude = 0.0;
                self.position[2] = 0.0;
                // Ease onto the centerline heading (touchdown no longer snaps it).
                let mut dh = RUNWAY_HEADING - self.heading;
                while dh > std::f32::consts::PI {
                    dh -= std::f32::consts::TAU;
                }
                while dh < -std::f32::consts::PI {
                    dh += std::f32::consts::TAU;
                }
                self.heading += dh.clamp(-1.5 * dt, 1.5 * dt);
                if !self.must_clear_runway {
                    self.speed = 0.0;
                    return;
                }
                let remaining = wrap_centered(self.position[1] - self.airport_layout.exit_z).max(0.0);
                if remaining > 0.02 {
                    // Speed-dependent rollout to the far-end turn-off: wheel
                    // braking (fading with speed) slows to taxi speed, then the
                    // aircraft rolls on along the remaining runway and eases to
                    // a stop at the exit. Braking stiffens, up to the cap,
                    // whenever the runway left to the exit demands it.
                    let v = self.speed;
                    let cruise = if v > TAXI_SPEED {
                        (v - (ROLLOUT_BRAKE_BASE + ROLLOUT_BRAKE_FADE * v) * dt).max(TAXI_SPEED)
                    } else {
                        (v + GROUND_ACCEL * dt).min(TAXI_SPEED)
                    };
                    // Reach the exit-curve handoff at a rolling corner speed.
                    let envelope = (2.0 * GROUND_BRAKE * (remaining - 10.0).max(0.0) + 9.0).sqrt();
                    self.speed = cruise.min(envelope).max(v - ROLLOUT_BRAKE_MAX * dt).max(0.0);
                    let distance = ((v + self.speed) * 0.5 * dt).min(remaining);
                    self.position[0] += self.heading.cos() * distance;
                    self.position[1] -= distance;
                } else if self.speed > 0.0 {
                    // At the turn-off: shed the last creep with bounded braking.
                    self.speed = (self.speed - ROLLOUT_BRAKE_MAX * dt).max(0.0);
                } else {
                    self.ground_timer += dt;
                }
            }
            State::Taxi => self.tick_ground_route(dt),
            State::Ready => {
                self.altitude = 0.0;
                self.position[2] = 0.0;
                // Only roll onto takeoff when explicitly cleared via `release()`;
                // planes queued on the taxiway wait here until awakened.
                if self.cleared_for_takeoff && !self.runway_hold {
                    self.ground_timer += dt;
                    if self.ground_timer >= HOLD_SHORT_TIME {
                        self.cleared_for_takeoff = false;
                        self.state = State::Takeoff;
                        self.heading = RUNWAY_HEADING;
                        self.speed = 0.0;
                    }
                }
            }
            State::Takeoff => {
                if self.altitude <= 0.0 {
                    // Ground roll down Runway 36, rotating at liftoff speed.
                    self.heading = RUNWAY_HEADING;
                    self.speed = (self.speed + 8.0 * dt).min(self.liftoff_speed());
                    self.advance(dt);
                    if self.speed >= self.liftoff_speed()
                        || self.position[1] <= self.airport_layout.liftoff_z
                    {
                        self.altitude = 0.5;
                    }
                } else {
                    // Climb straight out, then hand over to a smooth transit to the sector.
                    self.speed = (self.speed + 6.0 * dt).min(self.liftoff_speed() * 1.15);
                    self.altitude += self.climb_rate() * dt;
                    self.advance(dt);
                    if self.altitude >= CLIMB_OUT_ALT {
                        self.state = State::Cruising;
                        self.joining = true;
                    }
                }
                self.position[2] = self.altitude;
            }
            State::Landing => self.fly_approach(dt),
            State::Parked => {
                self.altitude = 0.0;
                self.position[2] = 0.0;
            }
            State::Departing => {
                let max_speed = match (self.kind, self.model_class) {
                    (Kind::Fighter, _) => 66.0,
                    (Kind::Agent, ModelClass::Frontier) => 62.0,
                    _ => 48.0,
                };
                let liftoff_speed = self.liftoff_speed();
                let climb_rate = self.climb_rate();

                self.ground_timer += dt;

                // Phase 1: Takeoff ground roll along the airport's runway before rotation.
                let liftoff_z = self.airport_layout.liftoff_z;
                let early_rotate_z = self.airport_layout.threshold[1] - 38.0;
                if self.altitude <= 0.0 && self.position[1] > liftoff_z && self.ground_timer < 8.0 {
                    self.heading = RUNWAY_HEADING;
                    if self.ground_timer < 0.5 {
                        self.speed = (self.speed + 8.0 * dt).min(10.0);
                    } else {
                        self.speed = (self.speed + 15.0 * dt).min(liftoff_speed);
                    }
                    self.position[0] += self.heading.cos() * self.speed * dt;
                    self.position[1] += self.heading.sin() * self.speed * dt;
                    self.altitude = 0.0;
                    self.position[2] = 0.0;
                    if self.position[1] <= liftoff_z
                        || (self.speed >= liftoff_speed * 0.92 && self.position[1] <= early_rotate_z)
                    {
                        self.altitude = 0.5;
                        self.position[2] = 0.5;
                    }
                } else {
                    // Phase 2: Rotate, climb over the Swiss Alps, and fly unwrapped into the horizon.
                    self.speed = (self.speed + 10.0 * dt).min(max_speed);
                    if self.altitude > 35.0 {
                        if let Some(target_h) = self.departure_heading {
                            let mut dh = target_h - self.heading;
                            while dh > std::f32::consts::PI {
                                dh -= std::f32::consts::TAU;
                            }
                            while dh < -std::f32::consts::PI {
                                dh += std::f32::consts::TAU;
                            }
                            let max_turn = 0.45 * dt;
                            self.heading += dh.clamp(-max_turn, max_turn);
                        }
                    }
                    let rate = if self.altitude < 480.0 {
                        climb_rate
                    } else {
                        climb_rate * 0.28
                    };
                    self.altitude = (self.altitude + rate * dt).min(640.0);
                    self.position[0] += self.heading.cos() * self.speed * dt;
                    self.position[1] += self.heading.sin() * self.speed * dt;
                    self.position[2] = self.altitude;
                }
            }
        }
    }
}

/// Compute the unwrapped target `[x, z, alt]` for a wingman fighter in formation.
fn formation_position_raw(
    parent_pos: [f32; 3],
    parent_heading: f32,
    parent_on_ground: bool,
    slot: usize,
) -> [f32; 3] {
    let side = if slot % 2 == 0 { -1.0_f32 } else { 1.0_f32 };
    let tier = (slot / 2) as f32 + 1.0;
    let lateral = side * (16.0 + tier * 9.0);
    let aft = -(8.0 + tier * 7.0);
    let alt_offset = if parent_on_ground { 0.0 } else { 2.5 * tier };

    let fwd_x = parent_heading.cos();
    let fwd_z = parent_heading.sin();
    // Starboard (+right) unit vector in top-down XZ plane.
    let right_x = -parent_heading.sin();
    let right_z = parent_heading.cos();

    let x = parent_pos[0] + fwd_x * aft + right_x * lateral;
    let z = parent_pos[1] + fwd_z * aft + right_z * lateral;
    let alt = (parent_pos[2] + alt_offset).max(0.0);
    [x, z, alt]
}

/// Compute the target `[x, z, alt]` for a wingman fighter in formation slot `slot`
/// relative to a parent aircraft at `parent_pos` with `parent_heading`.
pub fn formation_position(
    parent_pos: [f32; 3],
    parent_heading: f32,
    parent_on_ground: bool,
    slot: usize,
) -> [f32; 3] {
    let raw = formation_position_raw(parent_pos, parent_heading, parent_on_ground, slot);
    [wrap_centered(raw[0]), wrap_centered(raw[1]), raw[2]]
}

/// Compute the target `[x, z, alt]` for a refuelling receiver aircraft trailing
/// behind its provider tanker in formation slot `slot`:
/// - `0`: center tail boom directly aft & slightly below
/// - `1`: port (left) underwing drogue pod
/// - `2`: starboard (right) underwing drogue pod
/// - `3+`: wider/deeper V-echelon behind the outer pods
pub fn refuel_formation_position(
    tanker_pos: [f32; 3],
    tanker_heading: f32,
    slot: usize,
) -> [f32; 3] {
    let (aft, lateral, alt_offset) = match slot {
        0 => (-54.0_f32, 0.0_f32, -10.0_f32),
        1 => (-52.0, -44.0, -9.0),
        2 => (-52.0, 44.0, -9.0),
        _ => {
            let side = if slot % 2 == 1 { -1.0_f32 } else { 1.0_f32 };
            let tier = ((slot - 1) / 2) as f32;
            (
                -52.0 - tier * 34.0,
                side * (44.0 + tier * 18.0),
                -9.0 - tier * 5.0,
            )
        }
    };

    let fwd_x = tanker_heading.cos();
    let fwd_z = tanker_heading.sin();
    // Starboard (+right) unit vector in top-down XZ plane.
    let right_x = -tanker_heading.sin();
    let right_z = tanker_heading.cos();

    let x = tanker_pos[0] + fwd_x * aft + right_x * lateral;
    let z = tanker_pos[1] + fwd_z * aft + right_z * lateral;
    let alt = (tanker_pos[2] + alt_offset).max(60.0);
    [wrap_centered(x), wrap_centered(z), alt]
}

/// Snapshot of the whole world, ready to serialise over the wire.
#[derive(Clone, Debug, Serialize)]
pub struct WorldSnapshot {
    pub aircraft: Vec<Aircraft>,
    pub sectors: Vec<Sector>,
    pub airports: Vec<Airport>,
    pub omitted_threads: usize,
    /// Scheduled jobs beyond the Reaper ramps' capacity (shown as a count).
    pub omitted_jobs: usize,
}

/// A topic or project airspace sector, serialised for the client to render.
#[derive(Clone, Debug, Serialize)]
pub struct Sector {
    pub name: String,
    pub center: [f32; 2],
    pub radius: f32,
    /// `"domain"` for the 4 fixed life/work zones, `"project"` for dynamic T3 projects.
    pub kind: String,
}

/// Build the 4 default domain sectors.
pub fn default_sectors() -> Vec<Sector> {
    BASE_SECTORS
        .iter()
        .map(|(name, centre, radius)| Sector {
            name: (*name).to_string(),
            center: *centre,
            radius: *radius,
            kind: "domain".to_string(),
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct World {
    pub aircraft: Vec<Aircraft>,
    pub airports: Vec<Airport>,
    pub omitted_threads: usize,
    pub omitted_jobs: usize,
    pub sectors: Vec<Sector>,
    /// Per-airport aircraft currently granted the runway (rolling or on final).
    runway_active: std::collections::HashMap<String, u64>,
    /// Per-airport cleared ground departures, ordered by queue entry.
    departure_queue: std::collections::HashMap<String, std::collections::VecDeque<u64>>,
}

impl Default for World {
    fn default() -> Self {
        Self {
            aircraft: Vec::new(),
            airports: vec![Airport::home("local", "Tower")],
            omitted_threads: 0,
            omitted_jobs: 0,
            sectors: default_sectors(),
            runway_active: std::collections::HashMap::new(),
            departure_queue: std::collections::HashMap::new(),
        }
    }
}

impl World {
    pub fn new() -> Self {
        World::default()
    }

    pub fn add(&mut self, aircraft: Aircraft) {
        self.aircraft.push(aircraft);
    }

    pub fn get_mut(&mut self, callsign: &str) -> Option<&mut Aircraft> {
        self.aircraft.iter_mut().find(|a| a.callsign == callsign)
    }

    /// Hand an aircraft over to a new callsign (e.g. taxiway TX3 -> airborne
    /// SKY7) so the same airframe carries on instead of popping in and out.
    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        if from == to || self.aircraft.iter().any(|a| a.callsign == to) {
            return false;
        }
        match self.get_mut(from) {
            Some(a) => {
                a.callsign = to.to_string();
                true
            }
            None => false,
        }
    }

    /// Remove an aircraft by callsign (adapter reconciliation).
    pub fn remove_by_callsign(&mut self, callsign: &str) {
        self.aircraft.retain(|a| a.callsign != callsign);
    }

    /// Clear an aircraft for final departure into the horizon (thread marked done / archived).
    pub fn depart_by_callsign(&mut self, callsign: &str) -> bool {
        if let Some(a) = self.get_mut(callsign) {
            a.depart();
            true
        } else {
            false
        }
    }

    /// True if `callsign` is a wingman whose parent aircraft is departing forever.
    pub fn is_parent_departing(&self, callsign: &str) -> bool {
        let Some(pid) = self
            .aircraft
            .iter()
            .find(|a| a.callsign == callsign)
            .and_then(|a| a.parent_id)
        else {
            return false;
        };
        self.aircraft
            .iter()
            .find(|a| a.id == pid)
            .map(|p| p.is_departing())
            .unwrap_or(false)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Aircraft> {
        self.aircraft.iter()
    }

    /// Return true if `gate` is currently occupied or reserved by any aircraft
    /// other than `exclude_id`.
    pub fn is_gate_occupied(&self, gate: [f32; 2], exclude_id: Option<u64>) -> bool {
        const CLEARANCE: f32 = 12.0;
        self.aircraft.iter().any(|a| {
            if Some(a.id) == exclude_id {
                return false;
            }
            let at_gate = matches!(a.state, State::Parked | State::Landed | State::Taxi | State::Ready)
                && (a.position[0] - gate[0]).hypot(a.position[1] - gate[1]) < CLEARANCE;
            let reserved = a
                .taxi_destination()
                .map(|d| (d[0] - gate[0]).hypot(d[1] - gate[1]) < CLEARANCE)
                .unwrap_or(false);
            at_gate || reserved
        })
    }

    /// Pick an unoccupied gate at `airport_id`, falling back to `HOME_GATES` for `"local"`.
    pub fn find_free_gate_at(
        &self,
        airport_id: &str,
        exclude_id: Option<u64>,
        prefer_idx: usize,
    ) -> Option<[f32; 2]> {
        let gates: &[[f32; 2]] = self
            .airports
            .iter()
            .find(|ap| ap.id == airport_id)
            .map(|ap| ap.gates.as_slice())
            .filter(|g| !g.is_empty())
            .unwrap_or(&HOME_GATES);
        for offset in 0..gates.len() {
            let gate = gates[(prefer_idx + offset) % gates.len()];
            if !self.is_gate_occupied(gate, exclude_id) {
                return Some(gate);
            }
        }
        None
    }

    /// Advance every aircraft by `dt` seconds. Non-wingman aircraft move first,
    /// landed aircraft claim an unoccupied gate before taxiing, refuelling
    /// receivers track their provider tanker, and fighter wingmen smoothly
    /// track their parent's wing formation.
    pub fn tick(&mut self, dt: f32) {
        self.sync_tankers();
        let sectors = self.sectors.clone();

        // Runway occupancy per airport: who is rolling, on short final, or backtracking now.
        let mut runway_users_by_ap: std::collections::HashMap<String, Vec<u64>> =
            std::collections::HashMap::new();
        let mut arrival_on_final_by_ap: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for a in &self.aircraft {
            if a.parent_id.is_some() {
                continue;
            }
            let ap = a.airport_id().to_string();
            if a.occupies_runway() {
                runway_users_by_ap.entry(ap.clone()).or_default().push(a.id);
            }
            if a.state == State::Landing && a.on_final {
                arrival_on_final_by_ap.insert(ap);
            }
        }

        // Enqueue departures per airport while still taxiing, so each gets a reachable
        // holding slot rather than piling up behind a single threshold target.
        for (ap_id, queue) in &mut self.departure_queue {
            queue.retain(|id| {
                self.aircraft.iter().any(|a| {
                    a.id == *id
                        && a.parent_id.is_none()
                        && a.airport_id() == ap_id.as_str()
                        && a.cleared_for_takeoff
                        && matches!(a.state, State::Taxi | State::Ready)
                })
            });
        }
        self.departure_queue.retain(|_, q| !q.is_empty());

        let mut joining_by_ap: std::collections::HashMap<String, Vec<(u64, f32)>> =
            std::collections::HashMap::new();
        for a in &self.aircraft {
            if a.parent_id.is_some()
                || !a.cleared_for_takeoff
                || !matches!(a.state, State::Taxi | State::Ready)
            {
                continue;
            }
            let ap_id = a.airport_id().to_string();
            if self
                .departure_queue
                .get(&ap_id)
                .is_some_and(|q| q.contains(&a.id))
            {
                continue;
            }
            let mut previous = [a.position[0], a.position[1]];
            let mut remaining = 0.0;
            for &wp in &a.taxi_waypoints {
                remaining += (wp[0] - previous[0]).hypot(wp[1] - previous[1]);
                previous = wp;
            }
            joining_by_ap.entry(ap_id).or_default().push((a.id, remaining));
        }
        for (ap_id, mut joining) in joining_by_ap {
            joining.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            self.departure_queue
                .entry(ap_id)
                .or_default()
                .extend(joining.into_iter().map(|(id, _)| id));
        }

        let queues_snapshot: Vec<(String, Vec<u64>)> = self
            .departure_queue
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().copied().collect()))
            .collect();
        for (ap_id, queue) in queues_snapshot {
            let is_home = ap_id == "local";
            for (rank, id) in queue.into_iter().enumerate() {
                let a = self.aircraft.iter_mut().find(|a| a.id == id).unwrap();
                if !is_home {
                    let thresh = a.airport_layout.threshold;
                    if rank == 0 {
                        let at_thresh =
                            (a.position[0] - thresh[0]).hypot(a.position[1] - thresh[1]) < 0.1;
                        if a.taxi_destination() != Some(thresh)
                            && !(a.state == State::Ready && at_thresh)
                        {
                            a.route_to_runway(State::Ready);
                        }
                    } else if a.taxi_destination() == Some(thresh) {
                        // Hold at current gate/apron until the plane ahead clears the satellite runway.
                        a.taxi_waypoints.clear();
                        a.taxi_curve = None;
                        a.state = State::Ready;
                        a.ground_speed = 0.0;
                    }
                    continue;
                }
                let dest = if rank == 0 {
                    RUNWAY_THRESHOLD
                } else if rank <= TAXIWAY_QUEUE.len() {
                    TAXIWAY_QUEUE[rank - 1].0
                } else {
                    [
                        TAXIWAY_X,
                        TAXIWAY_QUEUE.last().unwrap().0[1]
                            - (rank - TAXIWAY_QUEUE.len()) as f32 * 22.0,
                    ]
                };
                let at_slot = (a.position[0] - dest[0]).hypot(a.position[1] - dest[1]) < 0.1;
                if a.taxi_destination() == Some(dest) || (a.state == State::Ready && at_slot) {
                    continue;
                }
                if rank == 0 {
                    a.route_to_runway(State::Ready);
                } else {
                    // Gate traffic uses the apron to the slot's latitude; planes
                    // already on Alpha move up the queue along the taxiway.
                    a.state = State::Taxi;
                    a.taxi_next = Some(State::Ready);
                    a.ground_timer = 0.0;
                    a.taxi_waypoints.clear();
                    a.taxi_curve = None;
                    let (exit, cur) = a.airport_layout.ramp_exit([a.position[0], a.position[1]]);
                    a.taxi_waypoints.extend(exit);
                    if cur[0] > 96.0 {
                        a.taxi_waypoints.push([APRON_X, cur[1]]);
                        a.taxi_waypoints.push([APRON_X, dest[1]]);
                    } else if (cur[0] - TAXIWAY_X).abs() > 2.0 {
                        a.taxi_waypoints.push([TAXIWAY_X, cur[1]]);
                    }
                    a.taxi_waypoints.push([TAXIWAY_X, dest[1]]);
                    a.taxi_waypoints.push(dest);
                }
            }
        }

        // One runway operation at a time per airport: drop a stale grant, keep
        // the departure queue in arrival order, and grant the runway to the next
        // eligible plane.
        let mut active_airports: std::collections::HashSet<String> =
            self.runway_active.keys().cloned().collect();
        active_airports.extend(self.departure_queue.keys().cloned());
        active_airports.extend(arrival_on_final_by_ap.iter().cloned());
        for ap_id in active_airports {
            let users = runway_users_by_ap.get(&ap_id);
            let arrival_on_final = arrival_on_final_by_ap.contains(&ap_id);
            if let Some(&id) = self.runway_active.get(&ap_id) {
                let still_granted = self
                    .aircraft
                    .iter()
                    .find(|a| a.id == id)
                    .map(|a| {
                        (a.state == State::Ready && a.cleared_for_takeoff)
                            || users.is_some_and(|u| u.contains(&id))
                    })
                    .unwrap_or(false);
                if !still_granted {
                    self.runway_active.remove(&ap_id);
                }
            }
            let has_users = users.is_some_and(|u| !u.is_empty());
            if !self.runway_active.contains_key(&ap_id) && !arrival_on_final && !has_users {
                if let Some(id) = self
                    .departure_queue
                    .get(&ap_id)
                    .and_then(|q| q.front().copied())
                {
                    let a = self.aircraft.iter().find(|a| a.id == id).unwrap();
                    if a.state == State::Ready && a.cleared_for_takeoff {
                        self.runway_active.insert(ap_id.clone(), id);
                    }
                }
            } else if arrival_on_final && self.runway_active.contains_key(&ap_id) {
                self.runway_active.remove(&ap_id);
            }
        }
        // A departure lined up on the threshold sits in the landing path:
        // arrivals at that airport treat the runway as occupied (go around).
        let lined_up_by_ap: std::collections::HashSet<String> = self
            .aircraft
            .iter()
            .filter(|a| {
                let layout = a.airport_layout;
                a.parent_id.is_none()
                    && a.state == State::Ready
                    && a.cleared_for_takeoff
                    && wrap_centered(a.position[0] - layout.runway_x).abs() < 8.0
                    && wrap_centered(a.position[1] - layout.threshold[1]).abs() < 10.0
            })
            .map(|a| a.airport_id().to_string())
            .collect();
        for a in &mut self.aircraft {
            let ap_id = a.airport_id().to_string();
            let users = runway_users_by_ap.get(&ap_id);
            let arrival_on_final = arrival_on_final_by_ap.contains(&ap_id);
            let active_id = self.runway_active.get(&ap_id).copied();
            a.runway_hold = match a.state {
                State::Ready => {
                    active_id != Some(a.id)
                        || arrival_on_final
                        || users.is_some_and(|u| u.iter().any(|&x| x != a.id))
                }
                _ => false,
            };
            a.runway_occupied = a.state == State::Landing
                && (users.is_some_and(|u| u.iter().any(|&x| x != a.id))
                    || lined_up_by_ap.contains(&ap_id));
        }

        // Advance tankers and all non-wingman, non-refueling-receiver aircraft first.
        for a in &mut self.aircraft {
            let is_refuel_receiver =
                a.kind != Kind::Tanker && a.state == State::Refueling && a.tanker_id.is_some();
            if a.parent_id.is_none() && !is_refuel_receiver {
                a.tick_with_sectors(dt, &sectors);
            }
        }

        self.tick_refuel_receivers(dt);

        // Assign unoccupied gates to landed aircraft ready to clear the runway.
        let ready_to_taxi: Vec<(u64, String)> = self
            .aircraft
            .iter()
            .filter(|a| {
                a.parent_id.is_none()
                    && a.state == State::Landed
                    && a.must_clear_runway
                    && (a.ground_timer >= TAXI_DELAY_AFTER_LANDING
                        || (wrap_centered(a.position[1] - a.airport_layout.exit_z) <= 10.0
                            && a.speed <= TAXI_SPEED + 0.01))
            })
            .map(|a| (a.id, a.airport_id().to_string()))
            .collect();

        for (id, ap_id) in ready_to_taxi {
            // Arrivals vacate at the far-end exit and taxi back along painted
            // taxiways to their (poll-reserved) arrival slot or a free gate.
            let slot = self.aircraft.iter().find(|a| a.id == id).and_then(|a| a.arrival_slot);
            if let Some((slot, _)) = slot {
                if let Some(a) = self.aircraft.iter_mut().find(|a| a.id == id) {
                    // Reapers shut down on their ramp slot; threads queue Ready.
                    let next = if a.scheduled_job.is_some() { State::Parked } else { State::Ready };
                    a.taxi_from_runway(slot, next);
                }
                continue;
            }
            if let Some(gate) = self.find_free_gate_at(&ap_id, Some(id), id as usize) {
                if let Some(a) = self.aircraft.iter_mut().find(|a| a.id == id) {
                    a.taxi_from_runway(gate, State::Parked);
                }
            }
        }

        // Snapshot parent telemetry so wingmen can read it without borrow conflict.
        let parents: std::collections::HashMap<u64, ([f32; 3], f32, State, bool)> = self
            .aircraft
            .iter()
            .filter(|a| a.parent_id.is_none())
            .map(|a| (a.id, (a.position, a.heading, a.state, a.is_departing())))
            .collect();

        let blend = (6.0 * dt).min(1.0);
        for a in &mut self.aircraft {
            let Some(pid) = a.parent_id else { continue };
            let Some(&(p_pos, p_heading, p_state, p_departing)) = parents.get(&pid) else {
                // Parent vanished: cruise independently until reconciled.
                a.tick_with_sectors(dt, &sectors);
                continue;
            };
            let on_ground = matches!(
                p_state,
                State::Parked | State::Landed | State::Taxi | State::Ready
            ) || (matches!(p_state, State::Departing | State::Takeoff) && p_pos[2] <= 0.5);

            if p_departing || p_state == State::Departing {
                // Follow departing parent unwrapped out over the mountains into the horizon.
                let target = formation_position_raw(p_pos, p_heading, on_ground, a.wing_slot.unwrap_or(0));
                let dx = target[0] - a.position[0];
                let dz = target[1] - a.position[1];
                a.position[0] += dx * blend;
                a.position[1] += dz * blend;
                a.altitude += (target[2] - a.altitude) * blend;
                a.position[2] = a.altitude;
            } else {
                let target = formation_position(p_pos, p_heading, on_ground, a.wing_slot.unwrap_or(0));

                // Shortest-path delta in wrapped world coordinates.
                let mut dx = target[0] - a.position[0];
                let mut dz = target[1] - a.position[1];
                if dx > AREA_SIZE * 0.5 {
                    dx -= AREA_SIZE;
                } else if dx < -AREA_SIZE * 0.5 {
                    dx += AREA_SIZE;
                }
                if dz > AREA_SIZE * 0.5 {
                    dz -= AREA_SIZE;
                } else if dz < -AREA_SIZE * 0.5 {
                    dz += AREA_SIZE;
                }

                a.position[0] = wrap_centered(a.position[0] + dx * blend);
                a.position[1] = wrap_centered(a.position[1] + dz * blend);
                a.altitude += (target[2] - a.altitude) * blend;
                a.position[2] = a.altitude;
            }

            // Match parent heading smoothly.
            let mut dh = p_heading - a.heading;
            while dh > std::f32::consts::PI {
                dh -= std::f32::consts::TAU;
            }
            while dh < -std::f32::consts::PI {
                dh += std::f32::consts::TAU;
            }
            a.heading += dh * blend;

            a.state = match p_state {
                State::Holding => State::Holding,
                State::Parked | State::Landed => State::Parked,
                State::Departing => State::Departing,
                _ => State::Cruising,
            };
        }

        // Despawn departing aircraft (and their escorting wingmen) once they vanish past the horizon.
        let vanished_ids: std::collections::HashSet<u64> = self
            .aircraft
            .iter()
            .filter(|a| {
                a.parent_id.is_none()
                    && a.state == State::Departing
                    && a.position[0].hypot(a.position[1]) >= DEPARTURE_DESPAWN_RADIUS
            })
            .map(|a| a.id)
            .collect();
        if !vanished_ids.is_empty() {
            self.aircraft.retain(|a| {
                !vanished_ids.contains(&a.id)
                    && !a.parent_id.map(|pid| vanished_ids.contains(&pid)).unwrap_or(false)
            });
        }

        // Vertical rates for CPA prediction (smoothed over the tick).
        if dt > 0.0 {
            for a in &mut self.aircraft {
                a.vr = ((a.altitude - a.prev_alt) / dt).clamp(-30.0, 30.0);
                a.prev_alt = a.altitude;
            }
        }

        // TCAS airborne separation: smoothly sidestep & climb/descend away from
        // conflicting traffic, then enforce hard 3D clearance so no two airborne
        // planes can ever overlap.
        self.enforce_air_separation(dt);
        self.refresh_reaper_phases();
    }

    fn enforce_air_separation(&mut self, dt: f32) {
        // Takeoffs and approaches are sequenced by the tower, not TCAS (its
        // 30u altitude floor would yank them off the glidepath).
        let is_airborne =
            |s: State| matches!(s, State::Cruising | State::Holding | State::Refueling);
        let is_formation_pair = |a: &Aircraft, b: &Aircraft| {
            a.parent_id == Some(b.id)
                || b.parent_id == Some(a.id)
                || (a.parent_id.is_some() && a.parent_id == b.parent_id)
                || (a.refuel_connected && a.tanker_id == Some(b.id))
                || (b.refuel_connected && b.tanker_id == Some(a.id))
                || (a.refuel_connected && b.refuel_connected && a.tanker_id == b.tanker_id)
        };

        // Gently relax previous TCAS offsets back toward zero when clear.
        // The latched evasion side releases once the offset has decayed away.
        let decay = (-SEP_DECAY_RATE * dt).exp();
        for a in &mut self.aircraft {
            if is_airborne(a.state) {
                a.sep_alt += (a.sep_offset[2] - a.sep_alt) * (1.0 - (-SEP_ALT_SMOOTHING * dt).exp());
                a.sep_hold = (a.sep_hold - dt).max(0.0);
                if a.sep_hold <= 0.0 {
                    a.sep_offset[0] *= decay;
                    a.sep_offset[1] *= decay;
                    a.sep_offset[2] *= decay;
                }
                if a.sep_offset[0].abs() + a.sep_offset[1].abs() < 4.0 {
                    a.avoid_side = 0.0;
                }
            } else {
                a.sep_offset = [0.0, 0.0, 0.0];
                a.sep_alt = 0.0;
                a.avoid_side = 0.0;
            }
        }

        let n = self.aircraft.len();
        // Velocity snapshot for CPA prediction: heading * speed + measured vertical rate.
        let vel: Vec<[f32; 3]> = (0..n)
            .map(|k| {
                let a = &self.aircraft[k];
                [
                    a.heading.cos() * a.speed,
                    a.heading.sin() * a.speed,
                    a.vr,
                ]
            })
            .collect();
        let mut push: Vec<[f32; 3]> = vec![[0.0, 0.0, 0.0]; n];
        let mut hard_alt: Vec<f32> = vec![0.0; n];

        for i in 0..n {
            if !is_airborne(self.aircraft[i].state) {
                continue;
            }
            for j in (i + 1)..n {
                if !is_airborne(self.aircraft[j].state) {
                    continue;
                }
                if is_formation_pair(&self.aircraft[i], &self.aircraft[j]) {
                    continue;
                }

                let mut dx = self.aircraft[j].position[0] - self.aircraft[i].position[0];
                let mut dz = self.aircraft[j].position[1] - self.aircraft[i].position[1];
                if dx > AREA_SIZE * 0.5 {
                    dx -= AREA_SIZE;
                } else if dx < -AREA_SIZE * 0.5 {
                    dx += AREA_SIZE;
                }
                if dz > AREA_SIZE * 0.5 {
                    dz -= AREA_SIZE;
                } else if dz < -AREA_SIZE * 0.5 {
                    dz += AREA_SIZE;
                }
                let hdist = dx.hypot(dz);
                let dalt = self.aircraft[j].position[2] - self.aircraft[i].position[2];
                let vdist = dalt.abs();

                // Canonical frame (lower-id -> higher-id) so every decision is
                // independent of vec order: c is the lower id, d the higher.
                let (c, d) = if self.aircraft[i].id <= self.aircraft[j].id {
                    (i, j)
                } else {
                    (j, i)
                };
                let mut cdx = self.aircraft[d].position[0] - self.aircraft[c].position[0];
                let mut cdz = self.aircraft[d].position[1] - self.aircraft[c].position[1];
                if cdx > AREA_SIZE * 0.5 {
                    cdx -= AREA_SIZE;
                } else if cdx < -AREA_SIZE * 0.5 {
                    cdx += AREA_SIZE;
                }
                if cdz > AREA_SIZE * 0.5 {
                    cdz -= AREA_SIZE;
                } else if cdz < -AREA_SIZE * 0.5 {
                    cdz += AREA_SIZE;
                }
                let cdalt = self.aircraft[d].position[2] - self.aircraft[c].position[2];

                // Predicted closest point of approach on the current velocity vectors.
                let rvx = vel[d][0] - vel[c][0];
                let rvz = vel[d][1] - vel[c][1];
                let rvy = vel[d][2] - vel[c][2];
                let v2 = rvx * rvx + rvz * rvz + rvy * rvy;
                let t_cpa = if v2 > 0.01 {
                    (-(cdx * rvx + cdz * rvz + cdalt * rvy) / v2).clamp(0.0, CPA_LOOKAHEAD)
                } else {
                    0.0
                };
                let px = cdx + rvx * t_cpa;
                let pz = cdz + rvz * t_cpa;
                let pv = cdalt + rvy * t_cpa;
                let h_pred = px.hypot(pz);
                let conflicting = (hdist < TCAS_HORIZ_BUFFER && vdist < TCAS_VERT_BUFFER)
                    || (h_pred < CPA_HORIZ_BUFFER && pv.abs() < CPA_VERT_BUFFER);
                if !conflicting {
                    continue;
                }

                // Canonical axis unit vector (deterministic fallback if co-located).
                let (nx, nz) = if hdist > 0.1 {
                    (cdx / hdist, cdz / hdist)
                } else {
                    let ang = (self.aircraft[c].id as f32) * 2.1;
                    (ang.cos(), ang.sin())
                };
                // The higher one climbs (lower descends); on a level tie the
                // higher-id aircraft climbs. Always expressed in the canonical frame.
                let higher_climbs = cdalt >= -0.01;
                let sgn = if higher_climbs { 1.0 } else { -1.0 };

                let urgency = (1.0 - hdist / TCAS_HORIZ_BUFFER)
                    .max(1.0 - h_pred / CPA_HORIZ_BUFFER)
                    .max(1.0 - vdist / TCAS_VERT_BUFFER)
                    .clamp(0.15, 1.0);
                let vert_step = TCAS_SOFT_VERT_RATE * urgency * dt;

                // Vertical resolution: both aircraft separate, at a bounded rate.
                push[c][2] -= sgn * vert_step;
                push[d][2] += sgn * vert_step;

                // Lateral resolution: only ONE aircraft evades (the lower id —
                // deterministic, and asymmetric so the pair cannot oscillate
                // left-right against each other); the other holds its course.
                // The evading side is latched until the offset decays.
                let evader = c;
                let current = self.aircraft[evader].avoid_side;
                let side = if current != 0.0 {
                    current
                } else {
                    // Deadband: near-collinear geometry has no meaningful side,
                    // so keep a fixed one instead of reading noise.
                    let cross = cdx * rvz - cdz * rvx;
                    if cross > 1.0 {
                        1.0
                    } else if cross < -1.0 {
                        -1.0
                    } else {
                        1.0
                    }
                };
                self.aircraft[evader].avoid_side = side;
                let lat_step = 18.0 * urgency * dt * side;
                // Perp to the canonical axis; a fixed physical side of the line.
                push[evader][0] += -nz * lat_step;
                push[evader][1] += nx * lat_step;

                // Hard vertical safety floor whenever two planes cross tightly,
                // applied at a bounded rate so it never snaps an aircraft.
                if hdist < TCAS_HARD_HORIZ && vdist < TCAS_HARD_VERT {
                    let need = (TCAS_HARD_VERT - vdist) * 0.5 + 0.5;
                    let step = need.min(VERT_HARD_RATE * dt);
                    hard_alt[c] -= sgn * step;
                    hard_alt[d] += sgn * step;
                }
            }
        }

        for i in 0..n {
            if !is_airborne(self.aircraft[i].state) {
                continue;
            }
            let a = &mut self.aircraft[i];
            a.sep_offset[0] = (a.sep_offset[0] + push[i][0]).clamp(-32.0, 32.0);
            a.sep_offset[1] = (a.sep_offset[1] + push[i][1]).clamp(-32.0, 32.0);
            a.sep_offset[2] = (a.sep_offset[2] + push[i][2] + hard_alt[i]).clamp(-26.0, 36.0);

            if push[i][0] != 0.0 || push[i][1] != 0.0 || push[i][2] != 0.0 || hard_alt[i] != 0.0 {
                a.sep_hold = SEP_HOLD_TIME;
                a.position[0] = wrap_centered(a.position[0] + push[i][0]);
                a.position[1] = wrap_centered(a.position[1] + push[i][1]);
                // Soft vertical resolution flows through the smoothed `sep_alt` target. Only
                // the rare hard safety floor moves altitude directly (bounded rate).
                a.altitude = (a.altitude + hard_alt[i]).max(30.0);
                a.position[2] = a.altitude;
            }
        }
    }

    pub fn snapshot(&self) -> WorldSnapshot {
        WorldSnapshot {
            aircraft: self.aircraft.clone(),
            sectors: self.sectors.clone(),
            airports: self.airports.clone(),
            omitted_threads: self.omitted_threads,
            omitted_jobs: self.omitted_jobs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ground_departure(callsign: &str, position: [f32; 2]) -> Aircraft {
        let mut a = Aircraft::with_model(callsign, Kind::Agent, ModelClass::Frontier);
        a.state = State::Parked;
        a.position = [position[0], position[1], 0.0];
        a.altitude = 0.0;
        a.heading = TAXIWAY_QUEUE.iter().find(|(p,_)| *p == position).map(|(_,h)|*h).unwrap_or(GATE_HEADING);
        a.release();
        a
    }

    #[test]
    fn gate_departures_reach_spaced_holds_then_advance_after_head_removal() {
        let mut world = World::new();
        for (i, &position) in HOME_GATES[..6].iter().enumerate() {
            world.add(ground_departure(&format!("GATE{i}"), position));
        }
        // Keep an arrival on final without advancing its approach: all cleared
        // departures should settle at distinct holds while the runway is busy.
        let mut arrival = Aircraft::with_model("FINAL", Kind::Agent, ModelClass::Frontier);
        arrival.state = State::Landing;
        arrival.altitude = 100.0;
        arrival.position = [56.0, 200.0, 100.0];
        arrival.on_final = true;
        let mut holding = false;
        for _ in 0..4000 {
            world.aircraft.push(arrival.clone());
            world.tick(0.05);
            world.aircraft.retain(|a| a.id != arrival.id);
            if world.aircraft.iter().all(|a| a.state == State::Ready) {
                holding = true;
                break;
            }
        }
        assert!(
            holding,
            "gate departures could not reach their holds: {:?}",
            world
                .aircraft
                .iter()
                .map(|a| (a.state, a.position, a.taxi_destination()))
                .collect::<Vec<_>>()
        );
        for (i, a) in world.aircraft.iter().enumerate() {
            for b in &world.aircraft[i + 1..] {
                assert!(
                    (a.position[0] - b.position[0]).hypot(a.position[1] - b.position[1])
                        >= GROUND_SEP
                );
            }
        }
        let head = *world.departure_queue["local"].front().unwrap();
        world.aircraft.retain(|a| a.id != head);
        let mut departed = std::collections::HashSet::new();
        for _ in 0..6000 {
            world.tick(0.05);
            for a in &world.aircraft {
                if a.state == State::Takeoff {
                    departed.insert(a.id);
                }
            }
            if departed.len() == 5 {
                break;
            }
        }
        assert_eq!(
            departed.len(),
            5,
            "queue did not advance after removing the head"
        );
    }

    #[test]
    fn threshold_departure_is_not_held_by_inbound_departure_taxis() {
        let mut world = World::new();
        world.add(ground_departure("HEAD", RUNWAY_THRESHOLD));
        for (i, position) in [[55.9, 39.3], [78.0, 46.0], [84.0, 22.0]]
            .into_iter()
            .enumerate()
        {
            world.add(ground_departure(&format!("BEHIND{i}"), position));
        }
        assert!(
            !world.aircraft[1].occupies_runway(),
            "inbound departure blocks the runway"
        );
        // Arrival backtracking must continue protecting the runway.
        let mut arrival = ground_departure("ARRIVAL", [56.0, 0.0]);
        arrival.cleared_for_takeoff = false;
        arrival.must_clear_runway = true;
        arrival.arrival_slot = Some(TAXIWAY_QUEUE[0]);
        assert!(arrival.occupies_runway());
        let mut departed = Vec::new();
        for _ in 0..6000 {
            world.tick(0.05);
            for a in &world.aircraft {
                if a.state == State::Takeoff && !departed.contains(&a.id) {
                    departed.push(a.id);
                }
            }
            if departed.len() == 4 {
                break;
            }
        }
        assert_eq!(
            departed,
            world.aircraft.iter().map(|a| a.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn coincident_taxi_departures_both_progress() {
        let mut world = World::new();
        world.add(ground_departure("OVERLAP1", [78.0, 46.0]));
        world.add(ground_departure("OVERLAP2", [78.0, 46.0]));
        let mut moved = [false; 2];
        for _ in 0..2400 {
            world.tick(0.05);
            for (i, a) in world.aircraft.iter().enumerate() {
                moved[i] |= (a.position[0] - 78.0).hypot(a.position[1] - 46.0) > 5.0;
            }
        }
        assert!(
            moved.into_iter().all(|m| m),
            "coincident departures stalled"
        );
        assert!(world.aircraft.iter().all(|a| a.altitude > 0.0));
    }

    #[test]
    fn simultaneous_departures_queue_and_take_off_in_sequence() {
        let mut world = World::new();
        for (i, &(position, _)) in TAXIWAY_QUEUE.iter().enumerate() {
            world.add(ground_departure(&format!("QUEUE{i}"), position));
        }
        let ids: Vec<_> = world.aircraft.iter().map(|a| a.id).collect();
        let mut ready = std::collections::HashSet::new();
        let mut departed = Vec::new();
        for _ in 0..6000 {
            world.tick(0.05);
            for a in &world.aircraft {
                if a.state == State::Ready {
                    ready.insert(a.id);
                }
                if a.state == State::Takeoff && !departed.contains(&a.id) {
                    assert!(ready.contains(&a.id), "departure skipped Ready");
                    assert!(
                        (a.position[0] - RUNWAY_THRESHOLD[0]).abs() < 0.1,
                        "departure rolled from a queue slot"
                    );
                    departed.push(a.id);
                }
            }
            if departed.len() == ids.len() {
                break;
            }
        }
        assert_eq!(
            departed, ids,
            "departure queue permanently stalled or reordered"
        );
    }

    #[test]
    fn sector_changes_and_hold_release_preserve_bounded_motion() {
        let mut a = Aircraft::with_model("CONTINUOUS", Kind::Agent, ModelClass::Frontier);
        a.position = [0.0, 0.0, 130.0]; a.altitude=130.0; a.heading=0.0;
        let sectors=default_sectors(); a.assign(&sectors[0].name);
        for i in 0..300 {
            if i==100 { a.assign(&sectors[1].name); }
            if i==150 { a.hold("test"); }
            if i==200 { a.release(); }
            let old=a.position; let heading=a.heading;
            a.tick_with_sectors(0.05,&sectors);
            let distance=wrap_centered(a.position[0]-old[0]).hypot(wrap_centered(a.position[1]-old[1]));
            assert!(distance < 6.0, "position jumped {distance} at {i}");
            let delta=(a.heading-heading+std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)-std::f32::consts::PI;
            assert!(delta.abs()<0.15,"heading jumped {delta} at {i}");
            assert!((a.altitude-old[2]).abs()<1.5,"altitude jumped");
        }
    }

    #[test]
    fn takeoff_climbs_continuously_into_assigned_sector_without_snap() {
        // Full lifecycle: taxi out, hold short, roll, rotate, climb out, then join
        // the assigned sector continuously — no position/heading/altitude snap anywhere.
        let sectors = default_sectors();
        let sector = &sectors[0];
        let mut world = World::new();
        let mut a = Aircraft::with_model("CONT-TAKEOFF", Kind::Agent, ModelClass::Frontier);
        a.state = State::Parked;
        a.position = [HOME_GATES[1][0], HOME_GATES[1][1], 0.0];
        a.altitude = 0.0;
        a.heading = GATE_HEADING;
        a.assign(&sector.name);
        world.add(a);
        world.get_mut("CONT-TAKEOFF").unwrap().release();

        let mut saw_airborne = false;
        let mut saw_join = false;
        for i in 0..4000 {
            let (old, old_alt, old_hdg) = world
                .iter()
                .find(|p| p.callsign == "CONT-TAKEOFF")
                .map(|p| (p.position, p.altitude, p.heading))
                .unwrap();
            world.tick(0.1);
            let p = world.iter().find(|p| p.callsign == "CONT-TAKEOFF").unwrap();
            let dist = wrap_centered(p.position[0] - old[0]).hypot(wrap_centered(p.position[1] - old[1]));
            let ground = p.altitude <= 0.0;
            // Physical speed caps: taxi 6 m/s, ground roll to liftoff 34 m/s,
            // air up to liftoff+15% climb speed (39 m/s). dt is 0.1s.
            let max_pos = if ground && !matches!(p.state, State::Takeoff | State::Departing) {
                0.7
            } else {
                5.0
            };
            assert!(dist < max_pos, "position jumped {dist} at tick {i} ({:?})", p.state);
            if ground {
                continue;
            }
            saw_airborne = true;
            let dh = (p.heading - old_hdg + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            assert!(dh.abs() < 0.2, "heading jumped {dh} at tick {i} ({:?})", p.state);
            assert!((p.altitude - old_alt).abs() < 2.0, "altitude jumped at tick {i}");
            if p.state == State::Cruising && p.joining {
                saw_join = true;
            }
            if saw_join && p.state == State::Cruising && !p.joining {
                let d = wrap_centered(p.position[0] - sector.center[0])
                    .hypot(wrap_centered(p.position[1] - sector.center[1]));
                assert!(d < sector.radius * 1.6, "drifted out of assigned sector: {d}");
                break;
            }
        }
        assert!(saw_airborne, "aircraft never left the ground");
        assert!(saw_join, "climb-out never handed over to the continuous sector join");
    }

    /// Run a head-on conflict and return (min 3D distance, evader side flips,
    /// final per-vec-slot (position, sep_offset)). FA always starts east, FB west;
    /// `fa_first` only changes the vec order (ids are creation-order stable).
    fn run_headon(fa_first: bool, dt: f32, ticks: u32) -> (f32, u32, Vec<([f32; 3], [f32; 3])>) {
        let mk = |cs: &str, east: bool| {
            let mut a = Aircraft::new(cs, Kind::Fighter);
            a.state = State::Cruising;
            a.position = [if east { 60.0 } else { -60.0 }, 0.0, 148.0];
            a.altitude = 148.0;
            a.heading = if east { std::f32::consts::PI } else { 0.0 };
            a.speed = 30.0;
            a.joining = false;
            a
        };
        let a = mk("FA", true);
        let b = mk("FB", false);
        let mut world = World::new();
        world.aircraft = if fa_first { vec![a, b] } else { vec![b, a] };

        let mut min_dist = f32::MAX;
        let mut prev_side = 0.0;
        let mut flips = 0u32;
        let first = world.aircraft[0].position;
        let mut last = [first[0], 0.0, first[2]];
        for _ in 0..ticks {
            world.tick(dt);
            let pair: Vec<&Aircraft> = world.aircraft.iter().collect();
            let (pa, pb) = (pair[0], pair[1]);
            let dx = wrap_centered(pb.position[0] - pa.position[0]);
            let dz = wrap_centered(pb.position[1] - pa.position[1]);
            let d = dx.hypot(dz).hypot((pb.position[2] - pa.position[2]).abs());
            min_dist = min_dist.min(d);
            // Bounded motion: speed + TCAS push (wrapped for map-edge crossings).
            let moved = wrap_centered(pa.position[0] - last[0]).abs()
                .max(wrap_centered(pa.position[2] - last[2]).abs());
            assert!(moved < 5.0, "unbounded per-tick motion: {moved}");
            last = [pa.position[0], 0.0, pa.position[2]];
            // Track the evader's latched side (lowest id) — sign of the lateral offset.
            let ev = if pa.id < pb.id { pa } else { pb };
            let side = ev.sep_offset[1].signum();
            if ev.sep_offset[1].abs() > 3.0 && prev_side != 0.0 && side != prev_side {
                flips += 1;
            }
            if side != 0.0 {
                prev_side = side;
            }
        }
        let pos: Vec<([f32; 3], [f32; 3])> = vec![
            (world.aircraft[0].position, world.aircraft[0].sep_offset),
            (world.aircraft[1].position, world.aircraft[1].sep_offset),
        ];
        (min_dist, flips, pos)
    }

    #[test]
    fn headon_conflict_yields_deterministically_without_oscillation() {
        let (min_dist, flips, _) = run_headon(true, 0.05, 400);
        assert!(min_dist > 8.0, "aircraft came within {min_dist} of each other");
        assert_eq!(flips, 0, "evader flip-flopped left/right {flips} times");
    }

    #[test]
    fn headon_result_is_stable_across_order_and_tick_size() {
        let (_, _, fwd) = run_headon(true, 0.05, 400);
        let (_, _, rev) = run_headon(false, 0.05, 400);
        // Same ids and initial states, reversed vec order: same outcome.
        let (fa_f, fb_f) = (fwd[0].0, fwd[1].0);
        let (fb_r, fa_r) = (rev[0].0, rev[1].0); // reversed vec holds [FB, FA]
        for k in 0..3 {
            assert!((fa_f[k] - fa_r[k]).abs() < 1e-3, "order changed FA pos: {fa_f:?} vs {fa_r:?}");
            assert!((fb_f[k] - fb_r[k]).abs() < 1e-3, "order changed FB pos: {fb_f:?} vs {fb_r:?}");
        }
        // Finer ticks must keep the same safety invariants.
        let (min_fine, flips_fine, _) = run_headon(true, 0.025, 800);
        assert!(min_fine > 8.0, "fine-tick run came within {min_fine}");
        assert_eq!(flips_fine, 0);
    }

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

    #[test]
    fn fighter_wingmen_flank_left_and_right_of_parent() {
        let parent_pos = [0.0, 0.0, 100.0];
        // Parent heading north (-PI/2): forward is (0, -1), starboard/right is (+1, 0).
        let left = formation_position(parent_pos, RUNWAY_HEADING, false, 0);
        let right = formation_position(parent_pos, RUNWAY_HEADING, false, 1);
        assert!(left[0] < 0.0, "slot 0 should be on port (west) wing");
        assert!(right[0] > 0.0, "slot 1 should be on starboard (east) wing");
        assert!(left[1] > 0.0 && right[1] > 0.0, "wingmen should fly slightly aft");
        assert!((left[0] + right[0]).abs() < 1e-3, "symmetric lateral spacing");
    }

    #[test]
    fn world_tick_moves_wingman_with_parent() {
        let mut world = World::new();
        let mut parent = Aircraft::with_model("SKY1", Kind::Agent, ModelClass::Frontier);
        parent.position = [10.0, 20.0, 100.0];
        parent.altitude = 100.0;
        parent.heading = 0.0;
        let pid = parent.id;
        world.add(parent);

        let wing = Aircraft::wingman("SKY1-A", pid, 0, [0.0, 0.0, 100.0], 0.0);
        world.add(wing);

        for _ in 0..40 {
            world.tick(0.05);
        }
        let p = world.iter().find(|a| a.callsign == "SKY1").unwrap();
        let w = world.iter().find(|a| a.callsign == "SKY1-A").unwrap();
        let expected = formation_position(p.position, p.heading, false, 0);
        let dist = (w.position[0] - expected[0]).hypot(w.position[1] - expected[1]);
        assert!(dist < 5.0, "wingman should track formation closely, dist={dist}");
    }

    #[test]
    fn landed_aircraft_never_parks_on_occupied_gate() {
        let mut world = World::new();
        // Fill all gates except the last one with parked aircraft.
        let last = HOME_GATES.len() - 1;
        for i in 0..last {
            let mut p = Aircraft::new(&format!("GT{}", i + 1), Kind::Agent);
            p.state = State::Parked;
            p.position = [HOME_GATES[i][0], HOME_GATES[i][1], 0.0];
            p.altitude = 0.0;
            world.add(p);
        }

        // Land a plane — it must claim the only free gate (HOME_GATES[last]).
        // Simulate a real landing: on the deck, and it must clear the runway.
        let mut arriving = Aircraft::new("SKY9", Kind::Agent);
        arriving.position = [RUNWAY_MID[0], RUNWAY_MID[1], 0.0];
        arriving.hold_on_runway();
        arriving.must_clear_runway = true;
        world.add(arriving);

        for _ in 0..1200 {
            world.tick(0.1);
        }
        let sky9 = world.iter().find(|a| a.callsign == "SKY9").unwrap();
        assert_eq!(sky9.state, State::Parked);
        let d = (sky9.position[0] - HOME_GATES[last][0])
            .hypot(sky9.position[1] - HOME_GATES[last][1]);
        assert!(d < 1.0, "SKY9 should park at the only free gate, got {:?}", sky9.position);

        // Land another plane when ALL gates are full — must stay Landed, never overlap!
        let mut overflow = Aircraft::new("SKY10", Kind::Agent);
        overflow.position = [RUNWAY_MID[0], RUNWAY_MID[1] + 20.0, 0.0];
        overflow.hold_on_runway();
        overflow.must_clear_runway = true;
        world.add(overflow);
        for _ in 0..200 {
            world.tick(0.1);
        }
        let sky10 = world.iter().find(|a| a.callsign == "SKY10").unwrap();
        assert_eq!(sky10.state, State::Landed, "must not park over an occupied gate");
    }

    #[test]
    fn frontier_jets_cruise_higher_than_local_props_and_drones() {
        let jet = Aircraft::with_model("JET1", Kind::Agent, ModelClass::Frontier);
        let prop = Aircraft::with_model("PROP1", Kind::Agent, ModelClass::Local);
        let drone = Aircraft::with_model("REAP1", Kind::Drone, ModelClass::Local);
        assert!(jet.cruise_altitude() > prop.cruise_altitude() + 20.0);
        assert!(prop.cruise_altitude() > drone.cruise_altitude() + 15.0);
    }

    #[test]
    fn airborne_planes_never_collide_on_crossing_paths() {
        let mut world = World::new();
        let mut a = Aircraft::with_model("SKY1", Kind::Agent, ModelClass::Frontier);
        let mut b = Aircraft::with_model("SKY2", Kind::Agent, ModelClass::Frontier);
        // Force both onto the exact same starting position, heading, and altitude.
        a.position = [0.0, 0.0, 150.0];
        a.altitude = 150.0;
        a.heading = 0.0;
        b.position = [0.0, 0.0, 150.0];
        b.altitude = 150.0;
        b.heading = 0.0;
        world.add(a);
        world.add(b);

        let dt = 0.05;
        // Worst-case per-tick relative growth: both planes get the full soft
        // vertical step plus the rate-limited hard floor, on top of the
        // bounded easing each does toward its own (TCAS-offset) target
        // altitude; only the evader moves laterally (18 u/s). Derived from
        // the rate constants so the caps can't drift out of sync.
        let max_dh = 18.0 * dt + 0.5;
        let max_dv = 2.0 * (TCAS_SOFT_VERT_RATE + VERT_HARD_RATE + ALT_EASE_RATE) * dt + 0.25;
        let (mut prev_h, mut prev_v) = (0.0f32, 0.0f32);
        for step in 0..200 {
            world.tick(dt);
            let p1 = world.aircraft[0].position;
            let p2 = world.aircraft[1].position;
            let hdist = (p1[0] - p2[0]).hypot(p1[1] - p2[1]);
            let vdist = (p1[2] - p2[2]).abs();
            // Separation is built at a bounded rate (no snap), then the hard
            // envelope holds for the rest of the flight.
            assert!(
                hdist - prev_h <= max_dh && vdist - prev_v <= max_dv,
                "separation jumped at step {step}: dh={:.2} dv={:.2}",
                hdist - prev_h,
                vdist - prev_v
            );
            if step >= 4 {
                assert!(
                    hdist >= TCAS_HARD_HORIZ || vdist >= TCAS_HARD_VERT - 0.1,
                    "collision at step {step}: hdist={hdist:.2}, vdist={vdist:.2}"
                );
            }
            (prev_h, prev_v) = (hdist, vdist);
        }
    }

    #[test]
    fn parked_plane_taxis_takes_off_and_flies_into_horizon_forever() {
        let mut world = World::new();
        let mut a = Aircraft::with_model("GT1", Kind::Agent, ModelClass::Frontier);
        a.state = State::Parked;
        a.position = [HOME_GATES[2][0], HOME_GATES[2][1], 0.0];
        a.altitude = 0.0;
        a.heading = GATE_HEADING;
        world.add(a);

        assert!(world.depart_by_callsign("GT1"));
        assert_eq!(world.iter().find(|p| p.callsign == "GT1").unwrap().state, State::Taxi);

        let mut saw_departing_on_runway = false;
        let mut saw_airborne_beyond_world = false;
        for _ in 0..1200 {
            world.tick(0.1);
            let Some(p) = world.iter().find(|p| p.callsign == "GT1") else {
                break;
            };
            if p.state == State::Departing && p.altitude == 0.0 {
                saw_departing_on_runway = true;
            }
            if p.state == State::Departing
                && p.altitude > 100.0
                && p.position[0].hypot(p.position[1]) > AREA_SIZE * 0.6
            {
                saw_airborne_beyond_world = true;
            }
        }
        assert!(saw_departing_on_runway, "should roll down Runway 36 before liftoff");
        assert!(saw_airborne_beyond_world, "should climb past world boundary without wrapping");
        assert!(
            world.iter().find(|p| p.callsign == "GT1").is_none(),
            "departed aircraft should despawn once beyond horizon"
        );
    }

    #[test]
    fn airborne_plane_and_wingman_depart_into_horizon_and_despawn_together() {
        let mut world = World::new();
        let mut parent = Aircraft::with_model("SKY1", Kind::Agent, ModelClass::Frontier);
        parent.position = [120.0, -80.0, 150.0];
        parent.altitude = 150.0;
        parent.heading = -0.5;
        let pid = parent.id;
        world.add(parent);

        let wing = Aircraft::wingman("SKY1-A", pid, 0, [110.0, -70.0, 152.5], -0.5);
        world.add(wing);

        assert!(world.depart_by_callsign("SKY1"));
        assert!(world.is_parent_departing("SKY1-A"));

        for _ in 0..800 {
            world.tick(0.1);
            if world.aircraft.is_empty() {
                break;
            }
        }
        assert!(
            world.aircraft.is_empty(),
            "parent and wingman should both vanish past horizon, left: {:?}",
            world.aircraft.iter().map(|a| &a.callsign).collect::<Vec<_>>()
        );
    }

    /// Home, an ordinary satellite, and the southern satellite whose final
    /// crosses the map wrap boundary.
    fn test_airports() -> [Airport; 3] {
        [
            Airport::home("local", "Tower"),
            Airport::satellite("ne", "NE", SATELLITE_SLOTS[1], true),
            Airport::satellite("south", "S", SATELLITE_SLOTS[2], true),
        ]
    }

    fn world_with(airport: &Airport) -> World {
        let mut world = World::new();
        world.airports = vec![Airport::home("local", "Tower")];
        if !airport.is_home {
            world.airports.push(airport.clone());
        }
        world
    }

    /// Departures join at the threshold end; arrivals roll out with
    /// speed-dependent braking along the runway, vacate at the opposite (far)
    /// end, and taxi back along the painted route without teleporting,
    /// re-entering the runway, or exceeding taxi speed, braking and yaw rates.
    #[test]
    fn every_airframe_lands_rolls_out_and_vacates_at_the_far_end() {
        const DT: f32 = 0.05;
        for airport in test_airports() {
            let layout = airport.runway_layout();
            for (n, (kind, model)) in [
                (Kind::Agent, ModelClass::Local),
                (Kind::Agent, ModelClass::Frontier),
                (Kind::Drone, ModelClass::Local),
                (Kind::Fighter, ModelClass::Frontier),
                (Kind::Tanker, ModelClass::Frontier),
            ]
            .into_iter()
            .enumerate()
            {
                let ctx = format!("{kind:?}/{model:?} at {}", airport.id);
                // Alternate between a poll-reserved arrival slot and a free gate.
                let slot = (n % 2 == 0).then(|| {
                    if airport.is_home {
                        TAXIWAY_QUEUE[n % TAXIWAY_QUEUE.len()]
                    } else {
                        (airport.gates[n % airport.gates.len()], GATE_HEADING)
                    }
                });
                let mut a = Aircraft::with_model("ARR", kind, model);
                a.set_airport(&airport);
                a.state = State::Cruising;
                a.position = [layout.runway_x + 30.0, wrap_centered(layout.touchdown_z + 260.0), 40.0];
                a.altitude = 40.0;
                a.heading = RUNWAY_HEADING;
                a.speed = a.approach_speed();
                assert!(a.begin_approach(slot));
                // The World despawns tankers that have no receivers, so the
                // tanker drives the same rollout and exit route standalone.
                let standalone = kind == Kind::Tanker;
                let mut world = world_with(&airport);
                if !standalone {
                    world.add(a.clone());
                }
                let id = a.id;
                let mut touchdown_z = None;
                let mut vacated_at = None;
                let mut left_runway = false;
                let mut prev: Option<Aircraft> = None;
                for _ in 0..40000 {
                    let cur = if standalone {
                        a.tick(DT);
                        if a.state == State::Landed && (a.ground_timer >= TAXI_DELAY_AFTER_LANDING
                        || (wrap_centered(a.position[1] - a.airport_layout.exit_z) <= 10.0
                            && a.speed <= TAXI_SPEED + 0.01)) {
                            let dest = slot.map(|s| s.0).unwrap_or(airport.gates[0]);
                            a.taxi_from_runway(dest, if slot.is_some() { State::Ready } else { State::Parked });
                        }
                        a.clone()
                    } else {
                        world.tick(DT);
                        world.aircraft.iter().find(|x| x.id == id).expect("arrival vanished").clone()
                    };
                    let lx = wrap_centered(cur.position[0] - layout.runway_x);
                    let lz = wrap_centered(cur.position[1] - layout.touchdown_z);
                    if let Some(p) = &prev {
                        let step = wrap_centered(cur.position[0] - p.position[0])
                            .hypot(wrap_centered(cur.position[1] - p.position[1]));
                        match (p.state, cur.state) {
                            (State::Landing, State::Landed) => {
                                assert!(lz.abs() < 3.0 && lx.abs() < 6.0, "{ctx}: touchdown {:?}", cur.position);
                                touchdown_z = Some(cur.position[1]);
                            }
                            (State::Landed, State::Landed) => {
                                // Rolling north along the runway, braking boundedly.
                                assert!(cur.position[1] <= p.position[1] + 1e-4, "{ctx}: rolled backwards");
                                assert!(step <= p.speed.max(cur.speed) * DT + 1e-3, "{ctx}: rollout jump {step}");
                                assert!(p.speed - cur.speed <= ROLLOUT_BRAKE_MAX * DT + 1e-3, "{ctx}: braking spike {} -> {} at {:?}", p.speed, cur.speed, cur.position);
                                assert!(cur.speed - p.speed <= GROUND_ACCEL * DT + 1e-3, "{ctx}: rollout surge");
                                assert!(cur.position[1] >= layout.end_z, "{ctx}: overran the runway");
                            }
                            (State::Landed, State::Taxi) => {
                                // Vacates only at the far-end exit, never short.
                                let ez = wrap_centered(cur.position[1] - layout.exit_z);
                                assert!(ez.abs() <= 10.5 && lx.abs() < 6.0, "{ctx}: vacated at {:?}", cur.position);
                                vacated_at = Some(cur.position);
                            }
                            (State::Taxi, State::Taxi) => {
                                assert!(step <= TAXI_SPEED * DT + 1e-3, "{ctx}: taxi jump {step}");
                                let yaw = (cur.heading - p.heading).sin().atan2((cur.heading - p.heading).cos());
                                assert!(yaw.abs() <= 1.2 * DT + 1e-3, "{ctx}: taxi yaw {yaw}, pos {:?}, speed {} -> {}", cur.position, p.ground_speed,cur.ground_speed);
                                assert!((cur.ground_speed - p.ground_speed).abs() <= GROUND_BRAKE * DT + 1e-3);
                                // Painted route stays on this airport's deck.
                                let (ox, oz) = (cur.position[0] - airport.center[0], cur.position[1] - airport.center[1]);
                                if airport.is_home {
                                    assert!((50.0..=125.0).contains(&ox) && (-90.0..=90.0).contains(&oz), "{ctx}: off deck {:?}", cur.position);
                                } else {
                                    assert!((2.0..=26.0).contains(&ox) && (-32.0..=32.0).contains(&oz), "{ctx}: off deck {:?}", cur.position);
                                }
                                if lx.abs() > 8.0 {
                                    left_runway = true;
                                }
                                assert!(!(left_runway && cur.occupies_runway()), "{ctx}: re-entered the runway {:?}", cur.position);
                            }
                            _ => {}
                        }
                    }
                    let done = matches!(cur.state, State::Ready | State::Parked);
                    prev = Some(cur);
                    if done {
                        break;
                    }
                }
                let last = prev.unwrap();
                assert!(matches!(last.state, State::Ready | State::Parked), "{ctx}: ended {:?}", last.state);
                assert!(touchdown_z.is_some() && vacated_at.is_some() && left_runway, "{ctx}: incomplete");
                assert!(!last.exiting_runway, "{ctx}: exit flag left set");
                if let Some((dest, heading)) = slot {
                    assert!((last.position[0] - dest[0]).hypot(last.position[1] - dest[1]) < 0.1, "{ctx}: not at slot");
                    assert!((last.heading - heading).sin().abs() < 0.02, "{ctx}: slot heading");
                }
            }
        }
    }

    /// The far-end exit route is continuous and starts on the runway at the
    /// opposite end from the departure entrance (the threshold).
    #[test]
    fn far_exit_route_starts_at_the_far_end_and_stays_off_the_runway() {
        for airport in test_airports() {
            let layout = airport.runway_layout();
            let dests: Vec<[f32; 2]> = if airport.is_home {
                TAXIWAY_QUEUE.iter().map(|s| s.0).chain(HOME_GATES).collect()
            } else {
                airport.gates.clone()
            };
            for dest in dests {
                let route = layout.far_exit_route(dest);
                assert_eq!(route[0], [layout.runway_x, layout.exit_z]);
                assert_eq!(*route.last().unwrap(), dest);
                // Far end: beyond the takeoff liftoff point, away from the threshold.
                assert!(layout.exit_z < layout.liftoff_z && layout.exit_z > layout.end_z);
                for w in route.windows(2) {
                    // Axis-aligned painted legs only, and nothing after the
                    // first leg is on the runway strip.
                    assert!((w[0][0] - w[1][0]).abs() < 1e-3 || (w[0][1] - w[1][1]).abs() < 1e-3);
                    assert!((w[1][0] - layout.runway_x).abs() > 8.0, "{} route {route:?}", airport.id);
                }
            }
        }
    }

    /// A departure never starts its roll while an arrival is on the runway
    /// (rollout or exit taxi), and an arrival never lands over a departure
    /// lined up on the threshold: it goes around and lands after the takeoff.
    #[test]
    fn runway_contention_between_arrivals_and_departures_is_safe() {
        for airport in test_airports() {
            let layout = airport.runway_layout();
            // 1. Arrival rolling out; departure released from the threshold.
            let mut world = world_with(&airport);
            let mut arrival = Aircraft::with_model("ARR", Kind::Agent, ModelClass::Frontier);
            arrival.set_airport(&airport);
            arrival.state = State::Landed;
            arrival.must_clear_runway = true;
            arrival.position = [layout.runway_x, layout.touchdown_z, 0.0];
            arrival.heading = RUNWAY_HEADING;
            arrival.speed = arrival.approach_speed();
            world.add(arrival);
            let mut dep = Aircraft::with_model("DEP", Kind::Agent, ModelClass::Frontier);
            dep.set_airport(&airport);
            dep.state = State::Parked;
            dep.position = [layout.threshold[0], layout.threshold[1], 0.0];
            dep.release();
            world.add(dep);
            let mut rolled = false;
            for _ in 0..4000 {
                let occupied = world.aircraft[0].occupies_runway();
                world.tick(0.05);
                if matches!(world.aircraft[1].state, State::Departing | State::Takeoff) {
                    assert!(!occupied, "{}: departure rolled with arrival on runway", airport.id);
                    assert!(world.aircraft[0].state != State::Landed, "{}: arrival still rolling", airport.id);
                    rolled = true;
                    break;
                }
            }
            assert!(rolled, "{}: departure never rolled", airport.id);

            // 2. Arrival on final; departure lined up on the threshold.
            let mut world = world_with(&airport);
            let mut dep = Aircraft::with_model("DEP", Kind::Agent, ModelClass::Local);
            dep.set_airport(&airport);
            dep.state = State::Parked;
            dep.position = [layout.threshold[0], layout.threshold[1], 0.0];
            dep.release();
            assert_eq!(dep.state, State::Ready);
            world.add(dep);
            let mut arr = Aircraft::with_model("ARR", Kind::Agent, ModelClass::Local);
            arr.set_airport(&airport);
            arr.state = State::Cruising;
            arr.position = [layout.runway_x, wrap_centered(layout.touchdown_z + 120.0), glide_height(120.0)];
            arr.altitude = glide_height(120.0);
            arr.heading = RUNWAY_HEADING;
            arr.speed = arr.approach_speed();
            arr.begin_approach(None);
            arr.on_final = true;
            world.add(arr);
            let mut dep_gone = false;
            let mut landed = false;
            for _ in 0..40000 {
                world.tick(0.05);
                let dep = world.iter().find(|a| a.callsign == "DEP");
                let dep_on_ground = dep.is_some_and(|d| d.altitude <= 0.0 && !d.is_departing());
                dep_gone |= dep.is_none_or(|d| d.altitude > 0.0);
                let arr = world.iter().find(|a| a.callsign == "ARR").unwrap();
                if arr.state == State::Landed {
                    assert!(dep_gone && !dep_on_ground, "{}: landed over a lined-up departure", airport.id);
                    landed = true;
                    break;
                }
            }
            assert!(dep_gone && landed, "{}: contention never resolved", airport.id);
        }
    }

    #[test]
    fn departure_waits_for_arrival_to_vacate_far_exit() {
        let mut world = World::new();
        let mut arrival = Aircraft::new("ARR", Kind::Agent);
        arrival.position = [56.0, TOUCHDOWN_Z, 0.0];
        arrival.altitude = 0.0;
        arrival.heading = RUNWAY_HEADING;
        arrival.state = State::Landed;
        arrival.must_clear_runway = true;
        arrival.speed = arrival.approach_speed();
        world.add(arrival);
        world.add(ground_departure("DEP", RUNWAY_THRESHOLD));
        let mut departed = false;
        for _ in 0..4000 {
            let occupied = world.aircraft[0].occupies_runway();
            world.tick(0.05);
            if world.aircraft[1].state == State::Takeoff {
                assert!(!occupied, "departure rolled while arrival occupied runway");
                departed = true;
                break;
            }
        }
        assert!(departed);
    }

    #[test]
    fn arrivals_roll_to_far_exit_and_return_via_apron() {
        for (kind, model) in [
            (Kind::Agent, ModelClass::Frontier),
            (Kind::Agent, ModelClass::Local),
            (Kind::Drone, ModelClass::Local),
            (Kind::Fighter, ModelClass::Frontier),
        ] {
            let mut world = World::new();
            let mut a = Aircraft::with_model("ARR", kind, model);
            a.position = [56.0, 300.0, 60.0];
            a.altitude = 60.0;
            a.heading = RUNWAY_HEADING;
            a.begin_approach(Some(TAXIWAY_QUEUE[0]));
            world.add(a);
            for (i, &(position, heading)) in TAXIWAY_QUEUE.iter().enumerate().skip(1) {
                let mut queued = Aircraft::new(&format!("Q{i}"), Kind::Agent);
                queued.state = State::Ready;
                queued.position = [position[0], position[1], 0.0];
                queued.altitude = 0.0;
                queued.heading = heading;
                world.add(queued);
            }
            let mut saw_exit = false;
            let mut saw_apron = false;
            for _ in 0..6000 {
                let prev = world.aircraft[0].position;
                let prev_heading = world.aircraft[0].heading;
                let prev_speed = world.aircraft[0].ground_speed;
                let was_taxi = world.aircraft[0].state == State::Taxi;
                world.tick(0.05);
                let a = &world.aircraft[0];
                if was_taxi && a.state == State::Taxi {
                    let yaw = (a.heading - prev_heading)
                        .sin()
                        .atan2((a.heading - prev_heading).cos())
                        .abs();
                    assert!(yaw <= 1.2 * 0.05 + 0.001);
                    assert!((a.ground_speed - prev_speed).abs() <= GROUND_BRAKE * 0.05 + 0.001);
                }
                if a.state == State::Taxi {
                    assert!(a.position[1] >= -88.1);
                    for queued in world.aircraft.iter().skip(1) {
                        assert!(
                            (a.position[0] - queued.position[0])
                                .hypot(a.position[1] - queued.position[1])
                                >= 12.0
                        );
                    }
                    if !saw_exit {
                        assert!(
                            (a.position[1] + 66.0).abs() <= 10.5,
                            "arrival vacated before far exit: {:?}",
                            a.position
                        );
                        saw_exit = true;
                    }
                    assert!(
                        (a.position[0] - prev[0]).hypot(a.position[1] - prev[1])
                            <= TAXI_SPEED * 0.05 + 0.01
                    );
                    if a.position[0] > 100.0 {
                        saw_apron = true;
                    }
                    if a.position[1] > -60.0 && a.position[0] > 64.0 {
                        assert!(
                            a.position[0] >= APRON_X - 1.5 || a.position[1] >= 40.9,
                            "arrival crossed departure queue: {:?}",
                            a.position
                        );
                    }
                }
                if a.state == State::Ready {
                    assert!(saw_exit && saw_apron);
                    break;
                }
            }
            assert_eq!(world.aircraft[0].state, State::Ready);
        }
    }

    #[test]
    fn approaches_from_anywhere_touch_down_on_the_runway() {
        for i in 0..48 {
            let ang = i as f32 * 0.7;
            let r = 120.0 + (i % 4) as f32 * 80.0;
            let class = if i % 2 == 0 { ModelClass::Frontier } else { ModelClass::Local };
            let mut a = Aircraft::with_model("ARR", Kind::Agent, class);
            a.position = [ang.cos() * r, ang.sin() * r, 150.0];
            a.altitude = 150.0;
            a.heading = i as f32 * 1.3;
            assert!(a.begin_approach(None));
            let mut landed_at = None;
            for _ in 0..(240.0 / 0.05) as usize {
                a.tick(0.05);
                if a.state == State::Landed {
                    landed_at = Some([a.position[0], a.position[1]]);
                    break;
                }
            }
            let p = landed_at.unwrap_or_else(|| panic!("approach {i} never landed"));
            // No touchdown snap: within the lined-up tolerance of the centerline.
            assert!((p[0] - RUNWAY_THRESHOLD[0]).abs() < 7.0, "touchdown x {p:?}");
            assert!((p[1] - TOUCHDOWN_Z).abs() < 3.0, "touched down at {p:?}");
            // Rollout stops on the runway (which ends at z = -77).
            for _ in 0..400 {
                a.tick(0.05);
            }
            assert_eq!(a.speed, 0.0);
            assert!(a.position[1] > -70.0, "overran the runway: {:?}", a.position);
        }
    }

    /// Every airframe flies the circuit, captures a ~3 degree final, flares
    /// and touches down nose-up at home, at a satellite, and at a southern
    /// satellite whose final crosses the wrap boundary of the patrol map.
    #[test]
    fn every_airframe_captures_a_shallow_final() {
        let airports = [
            Airport::home("local", "Tower"),
            Airport::satellite("ne", "NE", SATELLITE_SLOTS[1], true),
            Airport::satellite("south", "S", SATELLITE_SLOTS[2], true),
        ];
        for airport in &airports {
            let layout = airport.runway_layout();
            let [cx, cz] = airport.center;
            // Near the airport, far away and high, and (for the southern
            // satellite) across the north map edge so the shortest way wraps.
            let starts = [
                [cx + 0.0, wrap_centered(cz + 260.0), 60.0],
                [wrap_centered(cx - 240.0), wrap_centered(cz - 220.0), 180.0],
                [cx + 90.0, wrap_centered(cz + 240.0), 30.0],
                [cx, -395.0, 50.0],
            ];
            for (kind, model) in [
                (Kind::Agent, ModelClass::Local),
                (Kind::Agent, ModelClass::Frontier),
                (Kind::Drone, ModelClass::Local),
                (Kind::Fighter, ModelClass::Frontier),
                (Kind::Tanker, ModelClass::Frontier),
            ] {
                for start in starts {
                    let mut a = Aircraft::with_model("GLIDE", kind, model);
                    a.set_airport(airport);
                    a.state = State::Cruising;
                    a.position = start;
                    a.altitude = start[2];
                    a.heading = 1.2;
                    a.speed = a.approach_speed();
                    a.begin_approach(None);
                    let mut final_samples = 0;
                    let mut map_wraps = 0;
                    let mut landed = false;
                    let ctx = format!("{kind:?} at {} from {start:?}", airport.id);
                    for _ in 0..30000 {
                        let old = a.position;
                        let was_final = a.on_final;
                        let old_pitch = a.landing_pitch;
                        if was_final && old[2] < 0.15 {
                            assert!(old_pitch.unwrap() > 0.025, "{ctx}: no pre-touchdown flare");
                        }
                        a.tick(0.05);
                        let raw = (a.position[0] - old[0]).hypot(a.position[1] - old[1]);
                        let horizontal = wrap_centered(a.position[0] - old[0])
                            .hypot(wrap_centered(a.position[1] - old[1]));
                        assert!(horizontal < 1.5, "{ctx}: arrival position discontinuity");
                        if raw > 1.5 {
                            // Only the one crossing of the map edge on the way
                            // in; never on the circuit or on final.
                            map_wraps += 1;
                            assert!(!was_final && !a.on_final, "{ctx}: wrapped on final");
                            assert!(a.approach_leg == 0, "{ctx}: wrapped on the circuit");
                        }
                        assert!((a.altitude - old[2]).abs() < 0.11, "{ctx}: altitude discontinuity");
                        let angle = ((a.altitude - old[2]) / horizontal.max(0.001)).atan().to_degrees();
                        if was_final && a.on_final && a.altitude > 3.0 {
                            assert!(angle > -4.0 && angle < -2.0, "{ctx}: glide {angle}");
                            let pitch = a.landing_pitch.unwrap().to_degrees();
                            assert!((pitch - angle).abs() < 0.2, "{ctx}: pitch {pitch} vs path {angle}");
                            final_samples += 1;
                        }
                        if a.state == State::Landed {
                            assert!((old[2] - a.altitude) / 0.05 < 0.5, "{ctx}: hard touchdown");
                            assert!(a.landing_pitch.unwrap().to_degrees() > 3.5);
                            assert!(a.altitude >= 0.0);
                            let lz = wrap_centered(a.position[1] - layout.touchdown_z);
                            let lx = wrap_centered(a.position[0] - layout.runway_x);
                            assert!(lz.abs() < 3.0 && lx.abs() < 6.0, "{ctx}: touched down at {:?}", a.position);
                            assert!(a.position[1].abs() <= AREA_SIZE / 2.0, "{ctx}: off-map touchdown");
                            landed = true;
                            break;
                        }
                    }
                    assert!(map_wraps <= 1, "{ctx}: wrapped {map_wraps} times");
                    assert!(landed && final_samples > 30, "{ctx}: failed arrival");
                }
            }
        }
    }

    /// Arrivals from several spawn points and both approach speeds must flare
    /// gently onto the centerline, touch down on the deck (no clip, no
    /// float), and stop well short of the far end of the runway.
    #[test]
    fn arrival_flares_and_stops_within_runway() {
        for (cs, model, start) in [
            ("FL1", ModelClass::Frontier, [56.0, 300.0, 60.0]),
            ("FL2", ModelClass::Frontier, [90.0, 240.0, 30.0]),
            ("FL3", ModelClass::Local, [24.0, 320.0, 90.0]),
        ] {
            let mut world = World::new();
            let mut a = Aircraft::with_model(cs, Kind::Agent, model);
            a.state = State::Cruising;
            a.position = start;
            a.altitude = start[2];
            a.heading = RUNWAY_HEADING;
            a.speed = a.approach_speed();
            assert!(a.begin_approach(None));
            world.add(a);

            let mut prev_alt = start[2];
            let mut landed_at: Option<([f32; 2], usize)> = None;
            let mut max_climb = 0.0f32;
            let mut max_descent = 0.0f32;
            let mut early_rates = Vec::new();
            let mut late_rates = Vec::new();
            let mut clipped = false;
            let mut min_z = f32::MAX;
            for step in 0..2400 {
                world.tick(0.05);
                let a = &world.aircraft[0];
                let rate = (a.altitude - prev_alt) / 0.05;
                prev_alt = a.altitude;
                if a.state == State::Landing && a.on_final {
                    if rate < 0.0 {
                        let r = -rate;
                        if a.position[1] < TOUCHDOWN_Z + 50.0 {
                            late_rates.push(r);
                        } else if a.position[1] > TOUCHDOWN_Z + 180.0 {
                            early_rates.push(r);
                        }
                        max_descent = max_descent.max(r);
                    } else {
                        max_climb = max_climb.max(rate);
                    }
                }
                if a.state == State::Landing && a.altitude < 0.0 {
                    clipped = true;
                }
                if a.state == State::Landed {
                    landed_at.get_or_insert(([a.position[0], a.position[1]], step));
                    min_z = min_z.min(a.position[1]);
                    if a.speed == 0.0 && step - landed_at.unwrap().1 > 20 {
                        break;
                    }
                }
            }
            let (td, _) = landed_at.unwrap_or_else(|| panic!("{cs} never landed"));
            assert!((td[0] - RUNWAY_THRESHOLD[0]).abs() < 7.0, "{cs} touchdown x: {td:?}");
            assert!((td[1] - TOUCHDOWN_Z).abs() < 4.0, "{cs} touchdown z: {td:?}");
            assert!(!clipped, "{cs} clipped the deck");
            assert!(
                max_descent <= GLIDE_DESCENT_RATE + 0.5 && max_climb <= GLIDE_CLIMB_RATE + 0.5,
                "{cs} unbounded vertical correction: down {max_descent} up {max_climb}"
            );
            // Flare: descent near the touchdown point is far gentler than near
            // the fix — the path flattens out instead of cutting in.
            let avg = |v: &Vec<f32>| v.iter().sum::<f32>() / v.len().max(1) as f32;
            let early = avg(&early_rates);
            let late = avg(&late_rates);
            assert!(early > 1.0, "{cs} never meaningfully descended: {early}");
            assert!(
                late < 0.6 * early,
                "{cs} no flare: late {late} u/s vs early {early} u/s"
            );
            assert!(
                min_z > RUNWAY_END_Z + 6.0,
                "{cs} overran the runway: stopped at z {min_z}"
            );
        }
    }

    /// An arrival whose runway gets occupied on final goes around continuously
    /// (climbing, no snap), then completes a cleared approach once it clears.
    #[test]
    fn occupied_runway_forces_continuous_goaround_then_cleared_landing() {
        let mut world = World::new();
        // Blocker: a previous arrival still rolling out on the runway — it
        // must eventually clear, so it carries the real-landing flag.
        let mut b = Aircraft::with_model("BLK", Kind::Agent, ModelClass::Frontier);
        b.state = State::Landed;
        b.must_clear_runway = true;
        b.position = [56.0, 10.0, 0.0];
        b.altitude = 0.0;
        b.heading = RUNWAY_HEADING;
        b.speed = 12.0;
        world.add(b);
        // Arrival on final, 216u short of the touchdown point with a small
        // lateral offset — the blocker must still be clearing the runway
        // when it enters the go-around window, so it goes around, not down.
        let mut a = Aircraft::with_model("ARR", Kind::Agent, ModelClass::Frontier);
        a.state = State::Landing;
        a.on_final = true;
        a.position = [60.0, 250.0, 36.0];
        a.altitude = 36.0;
        a.heading = RUNWAY_HEADING;
        a.speed = a.approach_speed();
        world.add(a);

        let mut was_on_final = false;
        let mut went_around = false;
        let mut unsafe_gap = false;
        let mut landed = false;
        let mut prev_alt = 36.0f32;
        for step in 0..3000 {
            world.tick(0.05);
            let a = &world.aircraft[1];
            let dalt = (a.altitude - prev_alt) / 0.05;
            prev_alt = a.altitude;
            assert!(
                dalt >= -GLIDE_DESCENT_RATE - 0.5 && dalt <= GLIDE_CLIMB_RATE + 0.5,
                "vertical snap at {step}: {dalt} u/s"
            );
            if was_on_final && !a.on_final && a.altitude > 15.0 {
                went_around = true;
            }
            was_on_final |= a.state == State::Landing && a.on_final;
            let b = &world.aircraft[0];
            // Only a blocker actually on the runway matters: a cleared
            // backtaxi that has left the runway band (e.g. apron taxi to its
            // gate) does not threaten the final approach.
            if a.state == State::Landing && b.occupies_runway() {
                let gap = (a.position[0] - b.position[0]).hypot(a.position[1] - b.position[1]);
                unsafe_gap = unsafe_gap || gap < 40.0;
            }
            if a.state == State::Landed {
                landed = true;
                break;
            }
        }
        assert!(went_around, "arrival never went around the occupied runway");
        assert!(!unsafe_gap, "arrival closed to the blocker on the runway");
        assert!(landed, "arrival never completed a cleared approach");
    }

    /// An aircraft seeded on the runway holding for a takeoff clearance must
    /// not be auto-cleared to a gate, and must depart once released.
    #[test]
    fn held_aircraft_stays_on_runway_until_released() {
        let mut world = World::new();
        let mut a = Aircraft::with_model("HLD", Kind::Agent, ModelClass::Frontier);
        a.position = [RUNWAY_MID[0], RUNWAY_MID[1], 0.0];
        a.hold_on_runway();
        let id = a.id;
        world.add(a);

        // 120 s of ticking: the tower must never auto-clear it off the runway.
        for _ in 0..(120.0 / 0.05) as usize {
            world.tick(0.05);
        }
        let a = world.iter().find(|x| x.id == id).unwrap();
        assert_eq!(a.state, State::Landed, "held aircraft must stay Landed");
        let d = (a.position[0] - RUNWAY_MID[0]).hypot(a.position[1] - RUNWAY_MID[1]);
        assert!(d < 1.0, "held aircraft must hold position: {:?}", a.position);

        // Cleared for takeoff: backtaxi, hold short, roll out and depart.
        world.get_mut("HLD").unwrap().release();
        let mut departed = false;
        for _ in 0..(180.0 / 0.05) as usize {
            world.tick(0.05);
            let a = world.iter().find(|x| x.id == id).unwrap();
            if matches!(a.state, State::Takeoff | State::Cruising) {
                departed = true;
                break;
            }
        }
        assert!(departed, "held aircraft must depart once cleared");
    }

    #[test]
    fn tanker_spawns_only_when_provider_has_refueling_planes_and_despawns_when_empty() {
        let mut world = World::new();
        let mut a = Aircraft::with_model("SKY1", Kind::Agent, ModelClass::Frontier);
        a.model = Some("claude-opus-5-5".to_string());
        world.add(a);

        // No refueling planes -> no tanker in the sky.
        world.tick(0.05);
        assert!(world.iter().all(|x| x.kind != Kind::Tanker));

        // Plane enters refueling -> exactly one Claude tanker spawns above the zones.
        world
            .get_mut("SKY1")
            .unwrap()
            .refuel("Claude usage limit reached", None);
        world.tick(0.05);
        let tankers: Vec<_> = world.iter().filter(|x| x.kind == Kind::Tanker).collect();
        assert_eq!(tankers.len(), 1);
        assert_eq!(tankers[0].callsign, "KC-CLAUDE");
        assert_eq!(tankers[0].provider.as_deref(), Some("anthropic"));
        assert!(tankers[0].altitude >= TANKER_BASE_ALTITUDE - 0.5);

        let sky1 = world.iter().find(|x| x.callsign == "SKY1").unwrap();
        assert_eq!(sky1.tanker_id, Some(tankers[0].id));
        assert_eq!(sky1.refuel_slot, Some(0));

        // Second Claude plane joins the same tanker without moving the first receiver.
        let mut b = Aircraft::with_model("SKY2", Kind::Agent, ModelClass::Frontier);
        b.model = Some("claude-sonnet-5-5".to_string());
        b.refuel("Claude usage limit reached", None);
        world.add(b);
        world.tick(0.05);
        assert_eq!(world.iter().filter(|x| x.kind == Kind::Tanker).count(), 1);
        let s1 = world.iter().find(|x| x.callsign == "SKY1").unwrap().refuel_slot;
        let s2 = world.iter().find(|x| x.callsign == "SKY2").unwrap().refuel_slot;
        assert_eq!((s1, s2), (Some(0), Some(1)));

        // OpenAI plane connects -> a second tanker spawns at a separate flight level!
        let mut c = Aircraft::with_model("SKY3", Kind::Agent, ModelClass::Frontier);
        c.model = Some("gpt-6.1-sol".to_string());
        c.refuel("OpenAI usage limit reached", None);
        world.add(c);
        world.tick(0.05);
        let tankers: Vec<_> = world.iter().filter(|x| x.kind == Kind::Tanker).collect();
        assert_eq!(tankers.len(), 2);
        assert!(
            (tankers[0].cruise_altitude() - tankers[1].cruise_altitude()).abs() >= TANKER_ALT_STEP - 0.1
        );

        // Release both Claude planes -> Claude tanker disappears, OpenAI tanker remains.
        world.get_mut("SKY1").unwrap().release();
        world.get_mut("SKY2").unwrap().release();
        world.tick(0.05);
        let tankers: Vec<_> = world.iter().filter(|x| x.kind == Kind::Tanker).collect();
        assert_eq!(tankers.len(), 1);
        assert_eq!(tankers[0].callsign, "KC-OPENAI");

        // Release OpenAI plane -> zero tankers in the sky.
        world.get_mut("SKY3").unwrap().release();
        world.tick(0.05);
        assert_eq!(world.iter().filter(|x| x.kind == Kind::Tanker).count(), 0);
    }

    #[test]
    fn tanker_departure_does_not_move_other_flight_levels_or_occupied_slots() {
        let mut world = World::new();
        for (callsign, model) in [("FIRST", "claude"), ("SECOND", "claude"), ("THIRD", "codex")] {
            let mut a = Aircraft::with_model(callsign, Kind::Agent, ModelClass::Frontier);
            a.model = Some(model.into());
            a.refuel("usage limit", None);
            world.add(a);
        }
        world.sync_tankers();
        let second_slot = world.get_mut("SECOND").unwrap().refuel_slot;
        let third_tanker = world.get_mut("THIRD").unwrap().tanker_id.unwrap();
        let third_level = world.iter().find(|a| a.id == third_tanker).unwrap().cruise_altitude();

        world.remove_by_callsign("FIRST");
        world.sync_tankers();
        assert_eq!(world.get_mut("SECOND").unwrap().refuel_slot, second_slot);
        world.remove_by_callsign("SECOND");
        world.sync_tankers();
        assert_eq!(world.iter().find(|a| a.id == third_tanker).unwrap().cruise_altitude(), third_level);
    }

    #[test]
    fn refueling_planes_converge_and_hold_formation_behind_tanker() {
        let mut world = World::new();
        let mut a = Aircraft::with_model("SKY1", Kind::Agent, ModelClass::Frontier);
        a.model = Some("claude-opus-5-5".to_string());
        a.position = [0.0, 0.0, 150.0];
        a.altitude = 150.0;
        a.refuel("waiting for tokens", None);
        world.add(a);

        // Simulate 45 seconds so SKY1 flies up and locks onto the tanker's center boom.
        for _ in 0..900 {
            world.tick(0.05);
        }
        let tanker = world.iter().find(|x| x.kind == Kind::Tanker).unwrap();
        let sky1 = world.iter().find(|x| x.callsign == "SKY1").unwrap();
        let expected = refuel_formation_position(tanker.position, tanker.heading, 0);
        let hdist = (sky1.position[0] - expected[0]).hypot(sky1.position[1] - expected[1]);
        let vdist = (sky1.altitude - expected[2]).abs();
        assert!(hdist < 6.0 && vdist < 4.0, "SKY1 should hold formation behind tanker: h={hdist:.2}, v={vdist:.2}");
    }

    #[test]
    fn crowded_sector_cruise_altitude_does_not_bob() {
        // Several agents sharing a sector trigger TCAS offsets; altitude must stay
        // smooth (no near-instant jumps or rapid up/down flips).
        let sectors = default_sectors();
        let c = sectors[0].center;
        for count in [3usize, 6] {
            let mut w = World::new();
            for i in 0..count {
                let mut a = Aircraft::with_model(&format!("B{i}"), Kind::Agent, ModelClass::Frontier);
                a.state = State::Cruising;
                let ang = i as f32 * std::f32::consts::TAU / count as f32;
                a.position = [c[0] + 70.0 * ang.cos(), c[1] + 70.0 * ang.sin(), 130.0];
                a.altitude = 130.0;
                a.joining = true;
                a.assign(&sectors[0].name);
                w.add(a);
            }
            let mut prev: Vec<f32> = w.aircraft.iter().map(|a| a.altitude).collect();
            let (mut samples, mut jerky) = (0u32, 0u32);
            for t in 0..4800 {
                w.tick(0.05);
                for (i, a) in w.aircraft.iter().enumerate() {
                    let rate = (a.altitude - prev[i]) / 0.05;
                    if t > 800 {
                        samples += 1;
                        // Only the rare hard safety floor may move faster than the soft easing.
                        if rate.abs() > 12.0 {
                            jerky += 1;
                        }
                        assert!(rate.abs() < 55.0, "{count} planes: climb rate {rate} at tick {t}");
                    }
                    prev[i] = a.altitude;
                }
            }
            assert!(
                jerky * 200 < samples,
                "{count} planes: {jerky}/{samples} ticks climbed faster than 12 u/s (bobbing)"
            );
        }
    }

    #[test]
    fn satellite_aircraft_takeoff_and_departure_use_satellite_runway() {
        let mut w = World::new();
        let mac = Airport::satellite("mac", "Tom's M5 Air", [0.0, 285.0], true);
        let mac_thresh = mac.runway_layout().threshold;
        w.airports.push(mac.clone());

        let mut a = Aircraft::with_model("M1", Kind::Agent, ModelClass::Frontier);
        a.set_airport(&mac);
        a.state = State::Parked;
        a.position = [mac.gates[0][0], mac.gates[0][1], 0.0];
        a.altitude = 0.0;
        a.heading = GATE_HEADING;
        a.release();
        w.add(a);

        let mut lifted_near_mac = false;
        for _ in 0..2000 {
            w.tick(0.05);
            let p = w.get_mut("M1").unwrap();
            if p.altitude <= 0.0 {
                assert!(
                    p.position[0] < 35.0 && p.position[1] > 240.0,
                    "satellite ground taxi/roll left satellite island: {:?}",
                    p.position
                );
            }
            if p.state == State::Takeoff && p.altitude > 0.0 && !lifted_near_mac {
                assert!((p.position[0] - mac_thresh[0]).abs() < 4.0);
                assert!(p.position[1] >= 248.0 && p.position[1] <= mac_thresh[1]);
                lifted_near_mac = true;
            }
            if p.state == State::Cruising && p.altitude > 80.0 {
                break;
            }
        }
        assert!(lifted_near_mac, "aircraft should rotate on the satellite runway");

        // Final departure from a satellite gate also rolls off the satellite runway.
        let mut d = Aircraft::with_model("M2", Kind::Agent, ModelClass::Frontier);
        d.set_airport(&mac);
        d.state = State::Parked;
        d.position = [mac.gates[1][0], mac.gates[1][1], 0.0];
        d.altitude = 0.0;
        d.heading = GATE_HEADING;
        d.depart();
        w.add(d);

        let mut departed_off_mac = false;
        for _ in 0..3000 {
            w.tick(0.05);
            let Some(p) = w.get_mut("M2") else {
                break;
            };
            if p.altitude <= 0.0 {
                assert!(
                    p.position[0] < 35.0 && p.position[1] > 240.0,
                    "satellite departure taxi/roll left satellite island: {:?}",
                    p.position
                );
            } else if !departed_off_mac {
                assert!((p.position[0] - mac_thresh[0]).abs() < 4.0);
                departed_off_mac = true;
            }
        }
        assert!(departed_off_mac, "departing aircraft should lift off satellite runway");
        assert!(w.get_mut("M2").is_none(), "departing aircraft should vanish past horizon");
    }

    #[test]
    fn satellite_aircraft_approach_lands_at_satellite_runway_and_backtracks_to_gate() {
        let mut w = World::new();
        let mac = Airport::satellite("mac", "Tom's M5 Air", [0.0, 285.0], true);
        let mac_layout = mac.runway_layout();
        w.airports.push(mac.clone());

        let mut a = Aircraft::with_model("M3", Kind::Agent, ModelClass::Frontier);
        a.set_airport(&mac);
        a.state = State::Cruising;
        a.position = [120.0, 120.0, 148.0];
        a.altitude = 148.0;
        assert!(a.begin_approach(Some((mac.gates[2], GATE_HEADING))));
        w.add(a);

        let mut touched_on_mac = false;
        for _ in 0..4000 {
            w.tick(0.05);
            let p = w.get_mut("M3").unwrap();
            if p.state == State::Landed && !touched_on_mac {
                assert!(
                    (p.position[0] - mac_layout.runway_x).abs() < 8.0
                        && p.position[1] >= mac_layout.end_z
                        && p.position[1] <= mac_layout.threshold[1] + 5.0,
                    "touchdown should be on satellite runway, got {:?}",
                    p.position
                );
                touched_on_mac = true;
            }
            if touched_on_mac && p.state == State::Ready {
                break;
            }
        }
        assert!(touched_on_mac, "should touch down on the satellite runway");
        let p = w.get_mut("M3").unwrap();
        assert_eq!(p.state, State::Ready);
        let dist = (p.position[0] - mac.gates[2][0]).hypot(p.position[1] - mac.gates[2][1]);
        assert!(dist < 1.5, "should backtrack to assigned satellite gate, dist={dist}");
    }

    #[test]
    fn satellite_taxi_between_gates_stays_on_satellite_apron() {
        let mut w = World::new();
        let mac = Airport::satellite("mac", "Tom's M5 Air", [0.0, 285.0], true);
        w.airports.push(mac.clone());

        let mut a = Aircraft::with_model("M4", Kind::Agent, ModelClass::Frontier);
        a.set_airport(&mac);
        a.state = State::Parked;
        a.position = [mac.gates[0][0], mac.gates[0][1], 0.0];
        a.altitude = 0.0;
        a.heading = GATE_HEADING;
        a.taxi_to(mac.gates[3], State::Parked);
        w.add(a);

        // Include the moving turn out of a nose-in stand.
        for _ in 0..2400 {
            w.tick(0.05);
            let p = w.get_mut("M4").unwrap();
            assert!(
                p.position[0] >= 12.0 && p.position[0] <= 26.0 && p.position[1] >= 255.0 && p.position[1] <= 315.0,
                "satellite gate-to-gate taxi strayed off apron: {:?}",
                p.position
            );
            if p.state == State::Parked {
                break;
            }
        }
        let p = w.get_mut("M4").unwrap();
        assert_eq!(p.state, State::Parked);
        assert!((p.position[0] - mac.gates[3][0]).hypot(p.position[1] - mac.gates[3][1]) < 1.0);
    }

    // ---------------------------------------------------- scheduled-job Reapers

    const T0: i64 = 1_800_000_000;

    fn job(enabled: bool, running: bool, next_in: Option<i64>, now: i64) -> ScheduledJob {
        ScheduledJob {
            schedule: "every 15 min".into(),
            enabled,
            running,
            next_run_at: next_in.map(|s| crate::time::format_rfc3339(now + s)),
            last_run_at: None,
            last_run_status: if running { "running" } else { "never" }.into(),
            run_count: Some(0),
            phase: JobPhase::Parked,
        }
    }

    fn reaper_world(airport: &Airport, n: usize, job_for: impl Fn(usize) -> ScheduledJob) -> World {
        let mut world = world_with(airport);
        for i in 0..n {
            let slot = world.free_ramp_slot(&airport.id).expect("free ramp slot");
            let mut a = Aircraft::reaper(&format!("REAP{}", i + 1), airport, slot, job_for(i));
            a.assign("Homelab");
            world.add(a);
        }
        world
    }

    fn reaper<'a>(w: &'a World, cs: &str) -> &'a Aircraft {
        w.iter().find(|a| a.callsign == cs).unwrap()
    }

    fn set_job(w: &mut World, cs: &str, f: impl FnOnce(&mut ScheduledJob)) {
        f(w.get_mut(cs).unwrap().scheduled_job.as_mut().unwrap());
    }

    /// Tick at 20 Hz, steering every 3 s like the poller, until `done` holds.
    /// Asserts motion stays continuous (no teleports) the whole way.
    fn run_reapers(w: &mut World, now: &mut f32, t0: i64, secs: f32, done: impl Fn(&World) -> bool) -> bool {
        const DT: f32 = 0.05;
        let mut last: std::collections::HashMap<u64, [f32; 3]> =
            w.iter().map(|a| (a.id, a.position)).collect();
        for step in 0..(secs / DT) as usize {
            if step % 60 == 0 {
                w.steer_reapers(t0 + *now as i64);
            }
            w.tick(DT);
            *now += DT;
            for a in w.iter() {
                if let Some(p) = last.get(&a.id) {
                    let d = wrap_centered(a.position[0] - p[0]).hypot(wrap_centered(a.position[1] - p[1]));
                    assert!(d < 4.0, "{} jumped {d} in one tick ({:?})", a.callsign, a.state);
                }
                last.insert(a.id, a.position);
            }
            if done(w) {
                return true;
            }
        }
        false
    }

    #[test]
    fn reaper_ramp_slots_stay_clear_of_gates_queue_and_runway() {
        for airport in test_airports() {
            let layout = airport.runway_layout();
            let slots = layout.ramp_slots();
            assert!(!slots.is_empty());
            for (p, _) in &slots {
                assert!(layout.on_ramp(*p), "{}: slot off ramp", airport.id);
                for g in &airport.gates {
                    assert!((p[0] - g[0]).hypot(p[1] - g[1]) > 18.0, "{}: ramp slot near gate", airport.id);
                }
                for (q, _) in TAXIWAY_QUEUE {
                    assert!((p[0] - q[0]).hypot(p[1] - q[1]) > 30.0);
                }
                assert!((p[0] - layout.runway_x).abs() > 10.0, "{}: ramp on runway", airport.id);
                assert!(p[1] < layout.end_z, "{}: ramp beside the runway", airport.id);
            }
            for (i, (a, _)) in slots.iter().enumerate() {
                for (b, _) in &slots[i + 1..] {
                    assert!((a[0] - b[0]).hypot(a[1] - b[1]) >= 20.0, "{}: ramp slots too close", airport.id);
                }
            }
        }
    }

    #[test]
    fn reaper_full_lifecycle_from_ramp_to_sector_and_back() {
        for airport in test_airports() {
            let layout = airport.runway_layout();
            let mut w = reaper_world(&airport, 1, |_| job(true, false, Some(3600), T0));
            let slot = reaper(&w, "REAP1").ramp_slot().unwrap();
            let mut now = 0.0;

            // Enabled and idle: parked on the ramp, engines off.
            w.steer_reapers(T0);
            let a = reaper(&w, "REAP1");
            assert_eq!(a.state, State::Parked);
            assert_eq!(a.scheduled_job.as_ref().unwrap().phase, JobPhase::Parked);

            // Due within two minutes: taxi out and hold short of the runway.
            set_job(&mut w, "REAP1", |j| j.next_run_at = Some(crate::time::format_rfc3339(T0 + 90)));
            let (hold, _) = layout.reaper_hold();
            assert!(run_reapers(&mut w, &mut now, T0, 120.0, |w| reaper(w, "REAP1").state == State::Ready),
                "{}: never reached the holding point", airport.id);
            let a = reaper(&w, "REAP1");
            assert!((a.position[0] - hold[0]).hypot(a.position[1] - hold[1]) < 0.5);
            assert_eq!(a.scheduled_job.as_ref().unwrap().phase, JobPhase::HoldingShort);
            assert!(!a.in_runway_sequence(), "holding short must not take the runway");

            // Running: take the runway, climb out and loiter over the sector.
            set_job(&mut w, "REAP1", |j| { j.running = true; j.last_run_status = "running".into(); });
            assert!(run_reapers(&mut w, &mut now, T0, 120.0, |w| reaper(w, "REAP1").state == State::Takeoff));
            let a = reaper(&w, "REAP1");
            assert!((a.position[0] - layout.runway_x).abs() < 2.0, "{}: takeoff off the runway", airport.id);
            assert!(run_reapers(&mut w, &mut now, T0, 300.0, |w| {
                let a = reaper(w, "REAP1");
                a.state == State::Cruising && !a.joining
            }), "{}: never joined its loiter", airport.id);
            run_reapers(&mut w, &mut now, T0, 30.0, |_| false);
            let a = reaper(&w, "REAP1");
            assert_eq!(a.scheduled_job.as_ref().unwrap().phase, JobPhase::OnStation);
            let homelab = BASE_SECTORS.iter().find(|s| s.0 == "Homelab").unwrap();
            let d = wrap_centered(a.position[0] - homelab.1[0]).hypot(wrap_centered(a.position[1] - homelab.1[1]));
            assert!(d < homelab.2 * 1.3, "{}: loiter outside its sector ({d})", airport.id);
            assert!((48.0..90.0).contains(&a.altitude), "{}: not at drone altitude", airport.id);

            // Finished: approach, land and taxi back to the same ramp slot.
            set_job(&mut w, "REAP1", |j| {
                j.running = false;
                j.last_run_status = "succeeded".into();
                j.next_run_at = Some(crate::time::format_rfc3339(T0 + 3 * 3600));
            });
            assert!(run_reapers(&mut w, &mut now, T0, 30.0, |w| reaper(w, "REAP1").state == State::Landing));
            assert_eq!(reaper(&w, "REAP1").scheduled_job.as_ref().unwrap().phase, JobPhase::Returning);
            assert!(run_reapers(&mut w, &mut now, T0, 400.0, |w| reaper(w, "REAP1").state == State::Landed),
                "{}: never touched down", airport.id);
            assert!(run_reapers(&mut w, &mut now, T0, 300.0, |w| reaper(w, "REAP1").state == State::Parked),
                "{}: never got back to the ramp", airport.id);
            let a = reaper(&w, "REAP1");
            assert!((a.position[0] - slot.0[0]).hypot(a.position[1] - slot.0[1]) < 0.5, "{}: wrong slot", airport.id);
            assert!((a.heading - slot.1).sin().abs() < 0.05);
            run_reapers(&mut w, &mut now, T0, 3.0, |_| false);
            assert_eq!(reaper(&w, "REAP1").scheduled_job.as_ref().unwrap().phase, JobPhase::Parked);
        }
    }

    #[test]
    fn disabled_reaper_stays_parked_even_when_due() {
        let home = Airport::home("local", "Tower");
        let mut w = reaper_world(&home, 1, |_| job(false, false, Some(30), T0));
        let mut now = 0.0;
        run_reapers(&mut w, &mut now, T0, 20.0, |_| false);
        let a = reaper(&w, "REAP1");
        assert_eq!(a.state, State::Parked);
        assert_eq!(a.scheduled_job.as_ref().unwrap().phase, JobPhase::Disabled);
    }

    #[test]
    fn one_due_reaper_holds_short_per_airport_and_the_rest_wait_on_the_ramp() {
        let home = Airport::home("local", "Tower");
        let mut w = reaper_world(&home, 3, |i| job(true, false, Some(60 + i as i64 * 10), T0));
        let mut now = 0.0;
        run_reapers(&mut w, &mut now, T0, 50.0, |_| false);
        let moving: Vec<_> = w.iter().filter(|a| a.state != State::Parked).map(|a| a.callsign.clone()).collect();
        assert_eq!(moving, vec!["REAP1".to_string()], "only the soonest job taxis out");
    }

    #[test]
    fn simultaneous_reaper_runs_take_turns_on_the_runway() {
        let home = Airport::home("local", "Tower");
        let mut w = reaper_world(&home, 3, |_| job(true, true, Some(600), T0));
        let mut now = 0.0;
        let mut airborne_order = Vec::new();
        for _ in 0..(240.0 / 0.05) as usize {
            run_reapers(&mut w, &mut now, T0, 0.05, |_| false);
            let rolling: Vec<_> = w
                .iter()
                .filter(|a| a.state == State::Takeoff && a.altitude <= 0.0)
                .collect();
            assert!(rolling.len() <= 1, "two reapers rolling on the runway at once");
            for a in w.iter() {
                if a.altitude > 0.0 && !airborne_order.contains(&a.callsign) {
                    airborne_order.push(a.callsign.clone());
                }
            }
        }
        assert_eq!(airborne_order.len(), 3, "every running job got airborne: {airborne_order:?}");
    }

    #[test]
    fn reaper_ramp_capacity_is_bounded() {
        for airport in test_airports() {
            let mut w = world_with(&airport);
            let cap = w.ramp_capacity(&airport.id);
            for i in 0..cap {
                let slot = w.free_ramp_slot(&airport.id).unwrap();
                w.add(Aircraft::reaper(&format!("REAP{i}"), &airport, slot, job(true, false, None, T0)));
            }
            assert!(w.free_ramp_slot(&airport.id).is_none());
        }
    }
    #[test]
    fn rounded_taxi_keeps_moving_and_limits_yaw_through_corner() {
        for arrival in [false, true] {
            let mut a = Aircraft::new("CURVE", Kind::Agent);
            a.position = [103.0, -40.0, 0.0];
            a.heading = std::f32::consts::FRAC_PI_2;
            a.state = State::Taxi;
            a.exiting_runway = arrival;
            a.taxi_next = Some(State::Parked);
            a.taxi_waypoints = vec![[103.0, 0.0], [120.0, 0.0]];
            let mut turning = 0;
            for _ in 0..2000 {
                let h = a.heading;
                a.tick(0.05);
                let yaw = (a.heading - h).sin().atan2((a.heading - h).cos()).abs();
                assert!(yaw <= 0.061, "corner yaw {yaw}, arrival={arrival}");
                if yaw > 0.001 {
                    turning += 1;
                    assert!(a.ground_speed > 0.2, "pivot at painted corner");
                }
                if a.state != State::Taxi {
                    break;
                }
            }
            assert!(turning > 10);
            assert_eq!(a.state, State::Parked);
            assert!((a.position[0] - 120.0).hypot(a.position[1]) < 0.02);
        }
    }

    #[test]
    fn rounded_taxi_aligns_runway_entry_and_exit_while_moving() {
        for (start, heading, route, next, end_heading) in [
            (
                [84.0, 46.0],
                std::f32::consts::PI,
                vec![[56.0, 46.0]],
                State::Ready,
                RUNWAY_HEADING,
            ),
            (
                [56.0, -66.0],
                RUNWAY_HEADING,
                vec![
                    [70.0, -66.0],
                    [70.0, -88.0],
                    [103.0, -88.0],
                    [103.0, 0.0],
                    [120.0, 0.0],
                ],
                State::Parked,
                GATE_HEADING,
            ),
        ] {
            let mut a = Aircraft::new("ALIGN", Kind::Agent);
            a.position = [start[0], start[1], 0.0];
            a.heading = heading;
            a.state = State::Taxi;
            a.taxi_next = Some(next);
            a.taxi_waypoints = route;
            let mut finished = false;
            for _ in 0..10000 {
                let old = a.heading;
                a.tick(0.05);
                let yaw = (a.heading - old).sin().atan2((a.heading - old).cos()).abs();
                assert!(yaw < 0.061, "entry/exit snapped {yaw}");
                if a.state != State::Taxi {
                    finished = true;
                    break;
                }
            }
            assert!(finished);
            assert!(
                (a.heading - end_heading)
                    .sin()
                    .atan2((a.heading - end_heading).cos())
                    .abs()
                    < 0.01
            );
        }
    }

    #[test]
    fn rounded_routes_cover_short_satellite_reaper_and_jumbo_clearance() {
        let mut traces = Vec::new();
        let cases = [
            (
                "747 apron",
                [103.0, -40.0],
                std::f32::consts::FRAC_PI_2,
                vec![[103.0, 0.0], [120.0, 0.0]],
                0.0,
            ),
            (
                "satellite",
                [16.0, 15.0],
                RUNWAY_HEADING,
                vec![[16.0, -15.0], [24.0, -15.0]],
                0.0,
            ),
            (
                "Reaper return",
                [70.0, -66.0],
                std::f32::consts::PI,
                vec![
                    [70.0, -88.0],
                    [84.0, -88.0],
                    [84.0, -126.0],
                    [100.0, -126.0],
                ],
                std::f32::consts::PI,
            ),
            (
                "short",
                [0.0, 0.0],
                0.0,
                vec![[0.0, 0.0], [0.1, 0.0], [0.1, 0.1]],
                std::f32::consts::FRAC_PI_2,
            ),
        ];
        for (name, start, heading, route, end_heading) in cases {
            let mut a = Aircraft::with_model(name, Kind::Agent, ModelClass::Frontier);
            a.position = [start[0], start[1], 0.0];
            a.heading = heading;
            a.state = State::Taxi;
            a.taxi_next = Some(State::Parked);
            a.park_heading = Some(end_heading);
            a.taxi_waypoints = route.clone();
            let mut frames = Vec::new();
            for _ in 0..20000 {
                let old = a.heading;
                let previous = a.position;
                a.tick(0.05);
                let yaw = (a.heading - old).sin().atan2((a.heading - old).cos()).abs();
                assert!(yaw < 0.061, "{name}: yaw {yaw}");
                assert!(a.position.iter().all(|v| v.is_finite()));
                if yaw > 0.005 && a.state == State::Taxi {
                    assert!(
                        (a.position[0] - previous[0]).hypot(a.position[1] - previous[1]) > 0.000001,
                        "{name}: pivot"
                    );
                }
                // Jumbo gear track is 11m scaled by .355: on the broad apron
                // its wheel envelope must stay inside the apron/lead-in pavement.
                if name == "747 apron" {
                    assert!(a.position[0] - 1.953 >= 99.0);
                }
                frames.push(serde_json::json!([
                    a.position[0],
                    a.position[1],
                    a.heading,
                    a.ground_speed
                ]));
                if a.state != State::Taxi {
                    break;
                }
            }
            assert_eq!(a.state, State::Parked, "{name}: never finished");
            let dest = route.last().unwrap();
            assert!((a.position[0] - dest[0]).hypot(a.position[1] - dest[1]) < 0.002);
            traces
                .push(serde_json::json!({"name":name,"start":start,"route":route,"frames":frames}));
        }
        if let Ok(path) = std::env::var("SKYWATCH_TRAJECTORY_EVIDENCE") {
            std::fs::write(path, serde_json::to_string(&traces).unwrap()).unwrap();
        }
    }

    #[test]
    fn degenerate_taxi_preserves_position_and_heading() {
        for route in [vec![], vec![[103.0, 0.0], [103.0, 0.0]]] {
            let mut a = Aircraft::new("EMPTY", Kind::Agent);
            a.position = [103.0, 0.0, 0.0];
            a.heading = 1.3;
            a.state = State::Taxi;
            a.taxi_next = Some(State::Parked);
            a.taxi_waypoints = route;
            a.tick(0.05);
            assert_eq!(a.position, [103.0, 0.0, 0.0]);
            assert!(
                (a.heading - 1.3).abs() < 0.001,
                "stationary heading snapped"
            );
            assert_eq!(a.state, State::Parked);
        }
    }

    #[test]
    fn arrival_rollout_enters_exit_curve_without_stopping() {
        let mut world = World::new();
        let mut a = Aircraft::new("EXIT", Kind::Agent);
        a.position = [56.0, -40.0, 0.0];
        a.heading = RUNWAY_HEADING;
        a.state = State::Landed;
        a.must_clear_runway = true;
        a.speed = TAXI_SPEED;
        world.add(a);
        let mut entered = false;
        for _ in 0..2000 {
            world.tick(0.05);
            let a = &world.aircraft[0];
            if a.state == State::Taxi {
                assert!(
                    a.speed > 1.0,
                    "stopped at exit instead of rolling into curve"
                );
                assert!(a.position[1] > a.airport_layout.exit_z + 1.0);
                entered = true;
                break;
            }
        }
        assert!(entered);
    }

}
