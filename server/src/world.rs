use rand::Rng;
use serde::{Deserialize, Serialize};

/// What kind of entity this is: a piloted agent, an autonomous drone, or a
/// sub-agent fighter jet escorting a parent aircraft.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Agent,
    Drone,
    Fighter,
}

/// Lifecycle state of an aircraft in the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Cruising,
    Holding,
    Landed,
    /// Rolling on the ground between runway and parking gate.
    Taxi,
    /// Parked at a terminal gate, engines off.
    Parked,
    /// Lined up on the runway, holding short for takeoff clearance.
    Ready,
    Takeoff,
    Landing,
    /// Thread done: climbing away off the edge of the map, then removed.
    Outbound,
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
    pub hold_reason: Option<String>,
    /// Topic sector this aircraft works in (None = unassigned).
    #[serde(default)]
    pub topic: Option<String>,
    /// Human-readable subject (thread title or sub-agent task).
    #[serde(default)]
    pub title: Option<String>,
    /// Owning device / airport name (e.g. "omarchy", "Tom's M5 Air").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Parent aircraft id for escorting fighter wingmen (sub-agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<u64>,
    /// Wingman formation slot: 0 = left wing, 1 = right wing, 2 = outer left, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wing_slot: Option<usize>,
    /// Holding-orbit centre (derived on entry, not serialised).
    #[serde(skip)]
    orbit_center: Option<[f32; 2]>,
    /// Patrol orbit angle for sector-bound aircraft.
    #[serde(default)]
    patrol_angle: f32,
    /// Ordered ground-movement waypoints (taxiway routing), not serialised.
    #[serde(skip)]
    taxi_waypoints: Vec<[f32; 2]>,
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
    /// Smooth TCAS collision-avoidance offset `[dx, dz, dalt]` applied over nominal
    /// flight paths and decayed gently once clear of traffic.
    #[serde(skip)]
    sep_offset: [f32; 3],
    /// Active guidance (zone rejoin or approach intercept), not serialised.
    #[serde(skip)]
    leg: Option<Steer>,
    /// Current approach phase while `State::Landing`.
    #[serde(skip)]
    approach: Option<Approach>,
    /// Where to taxi after rolling out (None: wait for a free gate).
    #[serde(skip)]
    after_landing: Option<GroundSpot>,
    /// Heading to settle on when the current taxi route ends.
    #[serde(skip)]
    taxi_heading: Option<f32>,
    /// Current speed on the takeoff roll / landing rollout / outbound climb.
    #[serde(skip)]
    ground_speed: f32,
    /// Leave the airspace after takeoff instead of rejoining the zone.
    #[serde(skip)]
    leaving: bool,
    /// Ground speed in m/s (serde default keeps old snapshots deserialisable).
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_speed() -> f32 {
    AGENT_SPEED
}

/// Sim area is 820x820; positions wrap around at the edges.
const AREA_SIZE: f32 = 820.0;
const TAKEOFF_RATE: f32 = 6.0; // m/s climb
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

/// Home airport geometry (matches the client's island rendering).
pub const RUNWAY_MID: [f32; 2] = [56.0, -10.0];
pub const RUNWAY_THRESHOLD: [f32; 2] = [56.0, 46.0];
pub const RUNWAY_HEADING: f32 = -std::f32::consts::FRAC_PI_2; // rolling north (-Z)
/// Dedicated parallel Taxiway Alpha between Runway 36 (x=56) and the gate concourse.
pub const TAXIWAY_X: f32 = 84.0;
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
const TAKEOFF_ACCEL: f32 = 2.0; // m/s² down the runway before rotating
const DEPARTURE_ALTITUDE: f32 = 24.0; // climb straight out to here, then turn
const ROLLOUT_DECEL: f32 = 1.5; // m/s² braking after touchdown
const FINAL_SPEED_FACTOR: f32 = 0.8; // slow down on final
const TURN_RATE: f32 = 0.35; // rad/s, standard-rate-ish banking turns
const STEER_LOOKAHEAD: f32 = 40.0; // carrot distance along the intercept line
const DESCENT_RATE: f32 = 6.0; // m/s max descent on the inbound leg
const FINAL_DESCENT_MAX: f32 = 10.0; // m/s max descent to regain the glideslope
const INBOUND_GRADIENT: f32 = 0.3; // target height gained per unit from the FAF
const APPROACH_LEAD: f32 = 160.0; // intercept the localizer this far out
const REJOIN_LEAD: f32 = 90.0;
const OUTBOUND_SPEED_FACTOR: f32 = 1.6; // speed up once heading out
const OUTBOUND_CLIMB: f32 = 40.0; // climb above cruise level on the way out
/// Outbound aircraft are removed once this far past the map edge.
const OUTBOUND_EXIT: f32 = AREA_SIZE / 2.0 + 40.0;
/// Off-pattern distance beyond which cruising aircraft fly a rejoin leg.
const REJOIN_THRESHOLD: f32 = 6.0;
/// Final approach fix on the extended Runway 36 centreline, south of the island.
pub const FINAL_APPROACH_FIX: [f32; 2] = [56.0, 166.0];
pub const FAF_ALTITUDE: f32 = 32.0;
/// Touchdown point just past the threshold; rollout continues north.
pub const TOUCHDOWN: [f32; 2] = [56.0, 38.0];
const TAXI_DELAY_AFTER_LANDING: f32 = 5.0; // s rolled out on runway, then taxi
const HOLD_SHORT_TIME: f32 = 4.0; // s lined up before rolling

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
fn wrap_centered(v: f32) -> f32 {
    (v + AREA_SIZE / 2.0).rem_euclid(AREA_SIZE) - AREA_SIZE / 2.0
}

/// Shortest signed difference along one wrapped axis.
fn wrap_delta(d: f32) -> f32 {
    wrap_centered(d)
}

/// Normalise an angle to (-PI, PI].
fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let r = (a + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI;
    if r <= -std::f32::consts::PI { r + tau } else { r }
}

fn ease_toward(v: f32, target: f32, max_step: f32) -> f32 {
    v + (target - v).clamp(-max_step, max_step)
}

/// A parking spot on the ground: where to stop, which way to face, and the
/// state to settle into (Ready on the taxiway queue, Parked at a gate).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundSpot {
    pub pos: [f32; 2],
    pub heading: f32,
    pub state: State,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Approach {
    /// Curving from the current track onto the final approach fix.
    Inbound,
    /// Straight-in glideslope down the extended runway centreline.
    Final,
    /// Braking along the runway after touchdown.
    Rollout,
}

/// Turn-rate-limited guidance onto a line through `target` along `heading`,
/// the way an aircraft intercepts a localizer: fly to a gate well behind the
/// target, then chase a carrot along the line. Heading is always continuous,
/// so there are no cusps, reversals or jumps whatever the starting pose.
#[derive(Clone, Debug)]
struct Steer {
    target: [f32; 2],
    heading: f32,
    /// How far behind the target the intercept begins.
    lead: f32,
    /// Max cross-track error accepted when crossing the target.
    capture: f32,
    /// True once established far enough behind the target to intercept.
    intercept: bool,
}

impl Steer {
    fn new(target: [f32; 2], heading: f32, lead: f32, capture: f32) -> Self {
        Steer { target, heading, lead, capture, intercept: false }
    }
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
            (Kind::Fighter, _) => FIGHTER_SPEED,
            (Kind::Agent, ModelClass::Frontier) => FRONTIER_SPEED,
            (Kind::Agent, ModelClass::Local) => AGENT_SPEED,
            (Kind::Drone, _) => DRONE_SPEED,
        };
        let alt = match (kind, model_class) {
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
            parent_id: None,
            wing_slot: None,
            orbit_center: None,
            patrol_angle: rng.gen_range(0.0..std::f32::consts::TAU),
            taxi_waypoints: Vec::new(),
            taxi_next: None,
            cleared_for_takeoff: false,
            ground_timer: 0.0,
            sep_offset: [0.0, 0.0, 0.0],
            leg: None,
            approach: None,
            after_landing: None,
            taxi_heading: None,
            ground_speed: 0.0,
            leaving: false,
            speed,
        }
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

    fn on_ground(&self) -> bool {
        matches!(
            self.state,
            State::Parked | State::Landed | State::Taxi | State::Ready
        )
    }

    /// Enter a holding pattern with a reason (e.g. "awaiting approval").
    pub fn hold(&mut self, reason: &str) {
        if self.state != State::Holding {
            self.orbit_center = None; // re-derive the orbit from current pos
        }
        self.leg = None;
        self.approach = None;
        self.after_landing = None;
        self.state = State::Holding;
        self.hold_reason = Some(reason.to_string());
    }

    /// Start taxiing to `dest` via the yellow taxiway centerline so aircraft
    /// never cut diagonally across parked planes at neighboring gates.
    pub fn taxi_to(&mut self, dest: [f32; 2], next_state: State) {
        self.state = State::Taxi;
        self.taxi_next = Some(next_state);
        self.taxi_heading = None;
        self.ground_timer = 0.0;
        self.taxi_waypoints.clear();

        let cur = [self.position[0], self.position[1]];
        // Route via TAXIWAY_X unless already on the runway moving along it.
        let same_lane = (cur[0] - dest[0]).abs() < 3.0;
        if !same_lane {
            if (cur[0] - TAXIWAY_X).abs() > 2.0 {
                self.taxi_waypoints.push([TAXIWAY_X, cur[1]]);
            }
            if (cur[1] - dest[1]).abs() > 2.0 {
                self.taxi_waypoints.push([TAXIWAY_X, dest[1]]);
            }
        }
        self.taxi_waypoints.push(dest);
    }

    /// Taxi to `spot`, then settle into its state facing its heading.
    fn taxi_to_spot(&mut self, spot: GroundSpot) {
        self.taxi_to(spot.pos, spot.state);
        self.taxi_heading = Some(spot.heading);
    }

    /// The final destination of any in-progress taxi, if taxiing.
    pub fn taxi_destination(&self) -> Option<[f32; 2]> {
        self.taxi_waypoints.last().copied()
    }

    /// Cleared for takeoff: taxi out via the taxiway, hold short, then roll.
    pub fn release(&mut self) {
        self.orbit_center = None;
        self.hold_reason = None;
        self.cleared_for_takeoff = true;
        self.after_landing = None;
        match self.state {
            // On the ground: taxi to the runway threshold first.
            State::Parked | State::Landed | State::Taxi => {
                self.taxi_to(RUNWAY_THRESHOLD, State::Ready);
            }
            // Already lined up or holding on taxiway: if already at threshold roll now,
            // otherwise taxi onto the runway threshold first.
            State::Ready => {
                let d = (self.position[0] - RUNWAY_THRESHOLD[0])
                    .hypot(self.position[1] - RUNWAY_THRESHOLD[1]);
                if d < 6.0 {
                    self.state = State::Takeoff;
                    self.heading = RUNWAY_HEADING;
                    self.ground_speed = 0.0;
                } else {
                    self.taxi_to(RUNWAY_THRESHOLD, State::Ready);
                }
            }
            // On approach: go around and rejoin the zone pattern.
            State::Landing => self.go_around(),
            // Airborne hold: climb out, then rejoin the zone pattern.
            _ => {
                self.leg = None;
                self.state = State::Takeoff;
            }
        }
    }

    /// Abandon an approach: a touch-and-go on the runway, otherwise climb
    /// back out on a curved leg to the zone pattern.
    fn go_around(&mut self) {
        let rolling = self.approach == Some(Approach::Rollout);
        self.approach = None;
        self.after_landing = None;
        self.leg = None;
        self.state = if rolling { State::Takeoff } else { State::Cruising };
    }

    /// Fly a smooth approach: leave the current track on a curved inbound leg
    /// to the final approach fix, descend the glideslope, touch down just past
    /// the threshold and roll out. `World::tick` then assigns a free gate.
    pub fn land(&mut self) {
        self.begin_approach(None);
    }

    /// Start (or retarget) an approach that ends taxiing to `after`, if given.
    pub fn begin_approach(&mut self, after: Option<GroundSpot>) {
        if self.state == State::Landing {
            self.after_landing = after;
            return;
        }
        if self.on_ground() || (self.state == State::Takeoff && self.altitude <= 0.0) {
            // Already down (or a rejected takeoff): just taxi over.
            self.cleared_for_takeoff = false;
            match after {
                Some(spot) => self.taxi_to_spot(spot),
                None if self.state == State::Takeoff => {
                    self.state = State::Landed;
                    self.ground_timer = 0.0;
                }
                None => {}
            }
            return;
        }
        self.state = State::Landing;
        self.hold_reason = None;
        self.orbit_center = None;
        self.cleared_for_takeoff = false;
        self.after_landing = after;
        self.approach = Some(Approach::Inbound);
        self.leg = None;
    }

    /// Thread done: leave the airspace. Ground aircraft taxi out and take
    /// off; airborne ones turn away from the island and climb off the map.
    pub fn leave(&mut self) {
        self.hold_reason = None;
        self.after_landing = None;
        self.orbit_center = None;
        self.leg = None;
        match self.state {
            State::Outbound => {}
            State::Parked | State::Landed | State::Taxi | State::Ready => {
                self.release();
                self.leaving = true;
            }
            State::Takeoff => self.leaving = true,
            State::Landing if self.approach == Some(Approach::Rollout) => {
                self.approach = None;
                self.state = State::Takeoff;
                self.leaving = true;
            }
            _ => {
                self.approach = None;
                self.ground_speed = self.speed;
                self.state = State::Outbound;
            }
        }
    }

    /// True once an outbound aircraft has flown off the edge of the map.
    pub fn is_gone(&self) -> bool {
        self.state == State::Outbound
            && self.position[0].abs().max(self.position[1].abs()) > OUTBOUND_EXIT
    }

    /// Place the aircraft rolled out on the runway immediately (demo seeding).
    pub fn touch_down(&mut self) {
        self.state = State::Landed;
        self.altitude = 0.0;
        self.position[2] = 0.0;
        self.hold_reason = None;
        self.orbit_center = None;
        self.cleared_for_takeoff = false;
        self.taxi_waypoints.clear();
        self.taxi_next = None;
        self.leg = None;
        self.approach = None;
        self.after_landing = None;
        self.position[0] = RUNWAY_MID[0];
        self.position[1] = RUNWAY_MID[1];
        self.heading = RUNWAY_HEADING;
        self.ground_timer = 0.0;
    }

    /// Steer toward an airborne thread state without teleporting: ground
    /// aircraft taxi out and depart, approaches go around, and airborne
    /// aircraft enter or leave the hold where they are.
    pub fn fly(&mut self, target: State, reason: Option<String>) {
        match self.state {
            State::Parked | State::Landed | State::Taxi | State::Ready => {
                if !self.cleared_for_takeoff {
                    self.release();
                }
                self.hold_reason = reason;
                return;
            }
            State::Takeoff => {
                self.hold_reason = reason;
                return;
            }
            State::Landing => self.go_around(),
            State::Outbound => return,
            State::Cruising | State::Holding => {}
        }
        if self.state == State::Takeoff {
            self.hold_reason = reason;
            return;
        }
        if target == State::Holding {
            let why = reason.unwrap_or_else(|| "awaiting your input".to_string());
            self.hold(&why);
        } else {
            if self.state == State::Holding {
                self.state = State::Cruising;
                self.orbit_center = None;
            }
            self.hold_reason = reason;
        }
    }

    /// Bring this aircraft to rest at `spot`: airborne aircraft fly the
    /// approach and taxi there after rolling out; ground aircraft taxi over.
    pub fn park_at(&mut self, spot: GroundSpot) {
        if !self.on_ground() {
            self.begin_approach(Some(spot));
            return;
        }
        let bound_for = self.taxi_destination().unwrap_or([self.position[0], self.position[1]]);
        let retarget = (bound_for[0] - spot.pos[0]).hypot(bound_for[1] - spot.pos[1]) > 1.0
            || self.cleared_for_takeoff
            || (self.state == State::Taxi && self.taxi_next != Some(spot.state));
        if !retarget {
            return;
        }
        self.cleared_for_takeoff = false;
        if (self.position[0] - spot.pos[0]).hypot(self.position[1] - spot.pos[1]) > 1.0 {
            self.taxi_to_spot(spot);
        } else {
            self.taxi_waypoints.clear();
            self.taxi_next = None;
            self.state = spot.state;
            self.heading = spot.heading;
            self.ground_timer = 0.0;
        }
    }

    /// Snap straight onto the zone pattern (first sighting, nothing to animate).
    pub fn place_on_pattern(&mut self, sectors: &[Sector]) {
        let Some(sec) = self.sector(sectors) else { return };
        let (p, h, _) = self.pattern_pose(sec, self.patrol_angle);
        let alt = self.cruise_altitude() + 2.5 * (2.0 * self.patrol_angle).sin();
        self.set_pose(p, h, alt);
    }

    fn sector<'s>(&self, sectors: &'s [Sector]) -> Option<&'s Sector> {
        let topic = self.topic.as_ref()?;
        sectors.iter().find(|s| s.name == *topic)
    }

    fn set_pose(&mut self, p: [f32; 2], heading: f32, alt: f32) {
        self.position[0] = wrap_centered(p[0]);
        self.position[1] = wrap_centered(p[1]);
        self.heading = heading;
        self.altitude = alt;
        self.position[2] = alt;
    }

    /// Heading after one tick of turning toward `want` at the standard rate.
    fn turned_toward(&self, want: f32, dt: f32) -> f32 {
        let max_turn = TURN_RATE * dt;
        wrap_angle(self.heading + wrap_angle(want - self.heading).clamp(-max_turn, max_turn))
    }

    fn turn_toward(&mut self, want: f32, dt: f32) {
        self.heading = self.turned_toward(want, dt);
    }

    /// Fly one tick of `st` guidance at `speed`; true once across the target
    /// within the capture tolerance. A miss loops back round for another try.
    fn steer(&mut self, st: &mut Steer, speed: f32, dt: f32) -> bool {
        let (dx, dz) = (st.heading.cos(), st.heading.sin());
        let rx = self.position[0] - st.target[0];
        let rz = self.position[1] - st.target[1];
        let along = rx * dx + rz * dz;
        let cross = rz * dx - rx * dz;
        if !st.intercept && along < -st.lead {
            st.intercept = true;
        }
        if st.intercept && along >= 0.0 {
            if cross.abs() <= st.capture {
                return true;
            }
            st.intercept = false;
        }
        let aim_along = if st.intercept {
            (along + STEER_LOOKAHEAD).min(0.0)
        } else {
            -(st.lead + 40.0)
        };
        let aim = [st.target[0] + dx * aim_along, st.target[1] + dz * aim_along];
        let want = (aim[1] - self.position[1]).atan2(aim[0] - self.position[0]);
        self.turn_toward(want, dt);
        // Positions stay unwrapped while steering so guidance never chases a
        // target across the world seam; set_pose re-wraps on hand-over.
        self.position[0] += self.heading.cos() * speed * dt;
        self.position[1] += self.heading.sin() * speed * dt;
        false
    }

    /// Pose on the zone pattern at parameter `t`: `([x, z], heading, ds/dt)`.
    /// Frontier jets carve wide high-altitude figure-8s; props and drones fly
    /// scenic ovals and lazy S-curves, all at constant true airspeed.
    fn pattern_pose(&self, sec: &Sector, t: f32) -> ([f32; 2], f32, f32) {
        let span = match (self.kind, self.model_class) {
            (Kind::Agent, ModelClass::Frontier) => sec.radius * 1.18,
            (Kind::Drone, _) => sec.radius * 0.78,
            _ => sec.radius * 0.88,
        };
        let rot = (self.id as f32 * 1.17) % std::f32::consts::TAU;
        let (cr, sr) = (rot.cos(), rot.sin());
        let pattern = if self.kind == Kind::Drone { 1 } else { self.id % 3 };
        let (u, v, du, dv) = match pattern {
            // Pattern 0: wide figure-8 (Lissajous 1:2)
            0 => {
                let a = span * 1.12;
                let b = span * 0.56;
                (a * t.sin(), b * (2.0 * t).sin(), a * t.cos(), 2.0 * b * (2.0 * t).cos())
            }
            // Pattern 1: broad tilted scenic oval
            1 => {
                let a = span * 1.15;
                let b = span * 0.68;
                (a * t.cos(), b * t.sin(), -a * t.sin(), b * t.cos())
            }
            // Pattern 2: gentle three-lobe scenic S-loop
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
        (
            [sec.center[0] + ox, sec.center[1] + oz],
            vz.atan2(vx),
            du.hypot(dv).max(1.0),
        )
    }

    /// Advance the simulation by `dt` seconds (for non-wingman aircraft).
    #[cfg(test)]
    pub fn tick(&mut self, dt: f32) {
        self.tick_with_sectors(dt, &default_sectors());
    }

    /// Advance the simulation by `dt` seconds using the current world sectors.
    pub fn tick_with_sectors(&mut self, dt: f32, sectors: &[Sector]) {
        match self.state {
            State::Holding => {
                if self.orbit_center.is_none() {
                    // Centre the orbit off the left wing so the turn starts
                    // tangent to the current track instead of snapping 90°.
                    let side = self.heading + std::f32::consts::FRAC_PI_2;
                    let cx = self.position[0] + side.cos() * HOLD_TURN_RADIUS;
                    let cy = self.position[1] + side.sin() * HOLD_TURN_RADIUS;
                    self.orbit_center = Some([cx, cy]);
                }
                let [cx, cy] = self.orbit_center.unwrap();
                let dx = wrap_delta(self.position[0] - cx);
                let dy = wrap_delta(self.position[1] - cy);
                let angle = dy.atan2(dx);
                let omega = HOLD_DRIFT_SPEED / HOLD_TURN_RADIUS;
                let angle = angle + omega * dt;
                let nx = cx + angle.cos() * HOLD_TURN_RADIUS;
                let ny = cy + angle.sin() * HOLD_TURN_RADIUS;
                self.heading = angle + std::f32::consts::FRAC_PI_2;
                self.position[0] = wrap_centered(nx);
                self.position[1] = wrap_centered(ny);
                // Climb gently back to the hold level after a low go-around.
                let hold_alt = self.cruise_altitude();
                if self.altitude < hold_alt - 2.0 {
                    self.altitude = (self.altitude + TAKEOFF_RATE * dt).min(hold_alt);
                    self.position[2] = self.altitude;
                }
            }
            State::Cruising => {
                let base_alt = self.cruise_altitude();
                if let Some(sec) = self.sector(sectors) {
                    let wave = |t: f32| 2.5 * (2.0 * t).sin();
                    let (p, h, _) = self.pattern_pose(sec, self.patrol_angle);
                    if self.leg.is_none() {
                        // Off the pattern (just departed, left a hold, changed
                        // zone): steer back on instead of jumping.
                        let off = (self.position[0] - self.sep_offset[0] - p[0])
                            .hypot(self.position[1] - self.sep_offset[1] - p[1]);
                        let misaligned = wrap_angle(self.heading - h).abs() > 0.1;
                        if off > REJOIN_THRESHOLD || misaligned {
                            self.leg = Some(Steer::new(p, h, REJOIN_LEAD, 12.0));
                        } else if off > 1e-3 {
                            // Close enough: absorb the gap into the decaying offset.
                            self.sep_offset = [
                                self.position[0] - p[0],
                                self.position[1] - p[1],
                                self.altitude - (base_alt + wave(self.patrol_angle)),
                            ];
                        }
                    }
                    if let Some(mut st) = self.leg.take() {
                        let target_alt = base_alt + wave(self.patrol_angle);
                        self.altitude = ease_toward(self.altitude, target_alt, TAKEOFF_RATE * dt);
                        self.position[2] = self.altitude;
                        if self.steer(&mut st, self.speed, dt) && (self.altitude - target_alt).abs() < 3.0 {
                            // Fold the small residual into the decaying offset so
                            // the hand-over to the pattern is seamless.
                            self.sep_offset = [
                                self.position[0] - p[0],
                                self.position[1] - p[1],
                                self.altitude - target_alt,
                            ];
                            self.position[0] = wrap_centered(self.position[0]);
                            self.position[1] = wrap_centered(self.position[1]);
                        } else {
                            self.leg = Some(st);
                        }
                        return;
                    }

                    let (_, _, ds_dt) = self.pattern_pose(sec, self.patrol_angle);
                    self.patrol_angle =
                        (self.patrol_angle + (self.speed / ds_dt) * dt) % std::f32::consts::TAU;
                    let (p, h, _) = self.pattern_pose(sec, self.patrol_angle);
                    let s = self.sep_offset;
                    // Gentle vertical breath while cruising + TCAS altitude offset.
                    let alt = (base_alt + wave(self.patrol_angle) + s[2]).max(30.0);
                    self.set_pose([p[0] + s[0], p[1] + s[1]], h, alt);
                    return;
                }
                self.leg = None;
                self.altitude = ease_toward(self.altitude, base_alt, TAKEOFF_RATE * dt);
                self.position[2] = self.altitude;
                let dx = self.heading.cos() * self.speed * dt;
                let dy = self.heading.sin() * self.speed * dt;
                self.position[0] = wrap_centered(self.position[0] + dx);
                self.position[1] = wrap_centered(self.position[1] + dy);
            }
            State::Landed => {
                // Rolled out on the runway; World::tick assigns a free gate once
                // the landing pause elapses so no two aircraft share a gate.
                self.altitude = 0.0;
                self.position[2] = 0.0;
                self.ground_timer += dt;
            }
            State::Taxi => {
                self.altitude = 0.0;
                self.position[2] = 0.0;
                let mut rem = TAXI_SPEED * dt;
                while rem > 0.0 {
                    let Some(&target) = self.taxi_waypoints.first() else {
                        self.finish_taxi();
                        break;
                    };
                    let dx = target[0] - self.position[0];
                    let dy = target[1] - self.position[1];
                    let dist = dx.hypot(dy);
                    if rem >= dist {
                        self.position[0] = target[0];
                        self.position[1] = target[1];
                        rem -= dist;
                        self.taxi_waypoints.remove(0);
                        if self.taxi_waypoints.is_empty() {
                            self.finish_taxi();
                            break;
                        }
                    } else {
                        self.heading = dy.atan2(dx);
                        self.position[0] += dx / dist * rem;
                        self.position[1] += dy / dist * rem;
                        break;
                    }
                }
            }
            State::Ready => {
                self.altitude = 0.0;
                self.position[2] = 0.0;
                self.ground_speed = 0.0;
                // Only roll onto takeoff when explicitly cleared via `release()`;
                // planes queued on the taxiway wait here until awakened.
                if self.cleared_for_takeoff {
                    self.ground_timer += dt;
                    if self.ground_timer >= HOLD_SHORT_TIME {
                        self.cleared_for_takeoff = false;
                        self.state = State::Takeoff;
                    }
                }
            }
            State::Takeoff => {
                // Roll and accelerate down the runway, rotate, then climb out
                // to a safe height before curving onto the zone pattern.
                let rolling = self.altitude <= 0.0 && self.ground_speed < self.speed * 0.9;
                if rolling {
                    self.ground_speed += TAKEOFF_ACCEL * dt;
                } else {
                    self.ground_speed = self.speed;
                    self.altitude += TAKEOFF_RATE * dt;
                }
                self.position[2] = self.altitude;
                let dx = self.heading.cos() * self.ground_speed * dt;
                let dy = self.heading.sin() * self.ground_speed * dt;
                self.position[0] = wrap_centered(self.position[0] + dx);
                self.position[1] = wrap_centered(self.position[1] + dy);
                if self.altitude >= DEPARTURE_ALTITUDE.min(self.cruise_altitude()) {
                    self.state = if self.leaving { State::Outbound } else { State::Cruising };
                    self.leaving = false;
                    self.leg = None;
                }
            }
            State::Outbound => {
                // Turn away from the island and climb out past the map edge.
                // Positions stay unwrapped so it never reappears on the far side.
                let want = self.position[1].atan2(self.position[0]);
                self.turn_toward(want, dt);
                self.ground_speed = (self.ground_speed.max(self.speed) + 2.0 * dt)
                    .min(self.speed * OUTBOUND_SPEED_FACTOR);
                let ceiling = self.cruise_altitude() + OUTBOUND_CLIMB;
                self.altitude = ease_toward(self.altitude, ceiling, TAKEOFF_RATE * dt);
                self.position[2] = self.altitude;
                self.position[0] += self.heading.cos() * self.ground_speed * dt;
                self.position[1] += self.heading.sin() * self.ground_speed * dt;
            }
            State::Landing => self.fly_approach(dt),
            State::Parked => {
                self.altitude = 0.0;
                self.position[2] = 0.0;
            }
        }
    }

    /// Settle at the end of a taxi route into the queued state and heading.
    fn finish_taxi(&mut self) {
        self.state = self.taxi_next.take().unwrap_or(State::Parked);
        self.ground_timer = 0.0;
        if let Some(h) = self.taxi_heading.take() {
            self.heading = h;
        } else if self.state == State::Ready {
            self.heading = RUNWAY_HEADING;
        } else if self.state == State::Parked {
            self.heading = GATE_HEADING;
        }
    }

    /// Inbound leg → glideslope down the extended centreline → rollout.
    fn fly_approach(&mut self, dt: f32) {
        match self.approach.unwrap_or(Approach::Inbound) {
            Approach::Inbound => {
                let mut st = self.leg.take().unwrap_or_else(|| {
                    Steer::new(FINAL_APPROACH_FIX, RUNWAY_HEADING, APPROACH_LEAD, 3.0)
                });
                // Descend toward a height proportional to distance from the
                // FAF, never climbing, so the plane arrives near FAF altitude.
                let to_faf = (self.position[0] - FINAL_APPROACH_FIX[0])
                    .hypot(self.position[1] - FINAL_APPROACH_FIX[1]);
                let want = FAF_ALTITUDE + to_faf * INBOUND_GRADIENT;
                self.altitude = self
                    .altitude
                    .min(ease_toward(self.altitude, want, DESCENT_RATE * dt));
                self.position[2] = self.altitude;
                if self.steer(&mut st, self.speed, dt) {
                    self.approach = Some(Approach::Final);
                } else {
                    self.leg = Some(st);
                }
            }
            Approach::Final => {
                // Keep chasing a carrot down the centreline at the same turn
                // rate, so the hand-over from the intercept never snaps.
                let speed = self.speed * FINAL_SPEED_FACTOR;
                let want = (-STEER_LOOKAHEAD).atan2(TOUCHDOWN[0] - self.position[0]);
                let heading = self.turned_toward(want, dt);
                let x = self.position[0] + heading.cos() * speed * dt;
                let z = self.position[1] + heading.sin() * speed * dt;
                let span = FINAL_APPROACH_FIX[1] - TOUCHDOWN[1];
                if z <= TOUCHDOWN[1] {
                    self.set_pose(TOUCHDOWN, RUNWAY_HEADING, 0.0);
                    self.ground_speed = speed;
                    self.approach = Some(Approach::Rollout);
                } else {
                    let glide = FAF_ALTITUDE * ((z - TOUCHDOWN[1]) / span).clamp(0.0, 1.0);
                    let alt = self
                        .altitude
                        .min(glide.max(self.altitude - FINAL_DESCENT_MAX * dt));
                    self.set_pose([x, z], heading, alt);
                }
            }
            Approach::Rollout => {
                self.ground_speed = (self.ground_speed - ROLLOUT_DECEL * dt).max(TAXI_SPEED);
                let z = self.position[1] - self.ground_speed * dt;
                self.set_pose([TOUCHDOWN[0], z], RUNWAY_HEADING, 0.0);
                if self.ground_speed <= TAXI_SPEED {
                    self.approach = None;
                    self.ground_timer = 0.0;
                    match self.after_landing.take() {
                        Some(spot) => self.taxi_to_spot(spot),
                        None => self.state = State::Landed,
                    }
                }
            }
        }
    }
}

/// Compute the target `[x, z, alt]` for a wingman fighter in formation slot `slot`
/// relative to a parent aircraft at `parent_pos` with `parent_heading`.
pub fn formation_position(
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

    let x = wrap_centered(parent_pos[0] + fwd_x * aft + right_x * lateral);
    let z = wrap_centered(parent_pos[1] + fwd_z * aft + right_z * lateral);
    let alt = (parent_pos[2] + alt_offset).max(0.0);
    [x, z, alt]
}

/// Snapshot of the whole world, ready to serialise over the wire.
#[derive(Clone, Debug, Serialize)]
pub struct WorldSnapshot {
    pub aircraft: Vec<Aircraft>,
    pub sectors: Vec<Sector>,
    pub airports: Vec<Airport>,
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
    pub sectors: Vec<Sector>,
}

impl Default for World {
    fn default() -> Self {
        Self {
            aircraft: Vec::new(),
            airports: vec![Airport::home("local", "Tower")],
            sectors: default_sectors(),
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

    /// Remove an aircraft by callsign (adapter reconciliation).
    pub fn remove_by_callsign(&mut self, callsign: &str) {
        self.aircraft.retain(|a| a.callsign != callsign);
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

    /// Pick an unoccupied home terminal gate (rotating by `prefer_idx` for variety),
    /// or `None` if every gate is full.
    pub fn find_free_gate(&self, exclude_id: Option<u64>, prefer_idx: usize) -> Option<[f32; 2]> {
        for offset in 0..HOME_GATES.len() {
            let gate = HOME_GATES[(prefer_idx + offset) % HOME_GATES.len()];
            if !self.is_gate_occupied(gate, exclude_id) {
                return Some(gate);
            }
        }
        None
    }

    /// Advance every aircraft by `dt` seconds. Non-wingman aircraft move first,
    /// landed aircraft claim an unoccupied gate before taxiing, and fighter
    /// wingmen smoothly track their parent's wing formation.
    pub fn tick(&mut self, dt: f32) {
        let sectors = self.sectors.clone();
        for a in &mut self.aircraft {
            if a.parent_id.is_none() {
                a.tick_with_sectors(dt, &sectors);
            }
        }

        // Assign unoccupied gates to landed aircraft ready to clear the runway.
        let ready_to_taxi: Vec<u64> = self
            .aircraft
            .iter()
            .filter(|a| {
                a.parent_id.is_none()
                    && a.state == State::Landed
                    && a.ground_timer >= TAXI_DELAY_AFTER_LANDING
            })
            .map(|a| a.id)
            .collect();

        for id in ready_to_taxi {
            if let Some(gate) = self.find_free_gate(Some(id), id as usize) {
                if let Some(a) = self.aircraft.iter_mut().find(|a| a.id == id) {
                    a.taxi_to(gate, State::Parked);
                }
            }
        }

        // Snapshot parent telemetry so wingmen can read it without borrow conflict.
        let parents: std::collections::HashMap<u64, ([f32; 3], f32, State)> = self
            .aircraft
            .iter()
            .filter(|a| a.parent_id.is_none())
            .map(|a| (a.id, (a.position, a.heading, a.state)))
            .collect();

        let blend = (6.0 * dt).min(1.0);
        for a in &mut self.aircraft {
            let Some(pid) = a.parent_id else { continue };
            let Some(&(p_pos, p_heading, p_state)) = parents.get(&pid) else {
                // Parent vanished: cruise independently until reconciled.
                a.tick_with_sectors(dt, &sectors);
                continue;
            };
            if p_state == State::Outbound {
                // Parent's thread is done: peel off and leave with it.
                a.parent_id = None;
                a.wing_slot = None;
                a.leave();
                continue;
            }
            let on_ground = matches!(
                p_state,
                State::Parked | State::Landed | State::Taxi | State::Ready
            );
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
                _ => State::Cruising,
            };
        }

        // TCAS airborne separation: smoothly sidestep & climb/descend away from
        // conflicting traffic, then enforce hard 3D clearance so no two airborne
        // planes can ever overlap.
        self.enforce_air_separation(dt);

        // Outbound aircraft that have flown off the map are gone.
        self.aircraft.retain(|a| !a.is_gone());
    }

    fn enforce_air_separation(&mut self, dt: f32) {
        // Approach and low climb-out traffic is sequenced by the tower, so
        // TCAS never shoves it off the glideslope or runway centreline.
        let is_airborne = |a: &Aircraft| match a.state {
            State::Cruising | State::Holding => true,
            State::Takeoff => a.altitude > 40.0,
            _ => false,
        };
        let is_formation_pair = |a: &Aircraft, b: &Aircraft| {
            a.parent_id == Some(b.id)
                || b.parent_id == Some(a.id)
                || (a.parent_id.is_some() && a.parent_id == b.parent_id)
        };

        // Gently relax previous TCAS offsets back toward zero when clear.
        let decay = (-0.45 * dt).exp();
        for a in &mut self.aircraft {
            if is_airborne(a) {
                a.sep_offset[0] *= decay;
                a.sep_offset[1] *= decay;
                a.sep_offset[2] *= decay;
            } else {
                a.sep_offset = [0.0, 0.0, 0.0];
            }
        }

        let n = self.aircraft.len();
        let mut push: Vec<[f32; 3]> = vec![[0.0, 0.0, 0.0]; n];
        let mut hard_alt: Vec<f32> = vec![0.0; n];

        for i in 0..n {
            if !is_airborne(&self.aircraft[i]) {
                continue;
            }
            for j in (i + 1)..n {
                if !is_airborne(&self.aircraft[j]) {
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

                if hdist < TCAS_HORIZ_BUFFER && vdist < TCAS_VERT_BUFFER {
                    // Lateral unit vector (deterministic fallback if exactly co-located).
                    let (nx, nz) = if hdist > 0.1 {
                        (dx / hdist, dz / hdist)
                    } else {
                        let ang = (self.aircraft[i].id as f32) * 2.1;
                        (ang.cos(), ang.sin())
                    };
                    // Higher (or higher-ID tie-break) climbs, lower descends.
                    let sign_v = if dalt > 0.01 {
                        1.0
                    } else if dalt < -0.01 {
                        -1.0
                    } else if self.aircraft[j].id >= self.aircraft[i].id {
                        1.0
                    } else {
                        -1.0
                    };

                    let urgency = (1.0 - hdist / TCAS_HORIZ_BUFFER)
                        .max(1.0 - vdist / TCAS_VERT_BUFFER)
                        .clamp(0.15, 1.0);
                    let lat_step = 18.0 * urgency * dt;
                    let vert_step = 14.0 * urgency * dt;

                    push[i][0] -= nx * lat_step;
                    push[i][1] -= nz * lat_step;
                    push[i][2] -= sign_v * vert_step;
                    push[j][0] += nx * lat_step;
                    push[j][1] += nz * lat_step;
                    push[j][2] += sign_v * vert_step;

                    // Hard vertical safety floor whenever two planes cross tightly.
                    if hdist < TCAS_HARD_HORIZ && vdist < TCAS_HARD_VERT {
                        let need = (TCAS_HARD_VERT - vdist) * 0.5 + 0.5;
                        hard_alt[i] -= sign_v * need;
                        hard_alt[j] += sign_v * need;
                    }
                }
            }
        }

        for i in 0..n {
            if !is_airborne(&self.aircraft[i]) {
                continue;
            }
            let a = &mut self.aircraft[i];
            a.sep_offset[0] = (a.sep_offset[0] + push[i][0]).clamp(-32.0, 32.0);
            a.sep_offset[1] = (a.sep_offset[1] + push[i][1]).clamp(-32.0, 32.0);
            a.sep_offset[2] = (a.sep_offset[2] + push[i][2] + hard_alt[i]).clamp(-26.0, 36.0);

            if push[i][0] != 0.0 || push[i][1] != 0.0 || push[i][2] != 0.0 || hard_alt[i] != 0.0 {
                a.position[0] = wrap_centered(a.position[0] + push[i][0]);
                a.position[1] = wrap_centered(a.position[1] + push[i][1]);
                a.altitude = (a.altitude + push[i][2] + hard_alt[i]).max(30.0);
                a.position[2] = a.altitude;
            }
        }
    }

    pub fn snapshot(&self) -> WorldSnapshot {
        WorldSnapshot {
            aircraft: self.aircraft.clone(),
            sectors: self.sectors.clone(),
            airports: self.airports.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut arriving = Aircraft::new("SKY9", Kind::Agent);
        arriving.touch_down();
        world.add(arriving);

        for _ in 0..600 {
            world.tick(0.1);
        }
        let sky9 = world.iter().find(|a| a.callsign == "SKY9").unwrap();
        assert_eq!(sky9.state, State::Parked);
        let d = (sky9.position[0] - HOME_GATES[last][0])
            .hypot(sky9.position[1] - HOME_GATES[last][1]);
        assert!(d < 1.0, "SKY9 should park at the only free gate, got {:?}", sky9.position);

        // Land another plane when ALL gates are full — must stay Landed, never overlap!
        let mut overflow = Aircraft::new("SKY10", Kind::Agent);
        overflow.touch_down();
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

        for step in 0..200 {
            world.tick(0.05);
            let p1 = world.aircraft[0].position;
            let p2 = world.aircraft[1].position;
            let hdist = (p1[0] - p2[0]).hypot(p1[1] - p2[1]);
            let vdist = (p1[2] - p2[2]).abs();
            assert!(
                hdist >= TCAS_HARD_HORIZ || vdist >= TCAS_HARD_VERT - 0.1,
                "collision at step {step}: hdist={hdist:.2}, vdist={vdist:.2}"
            );
        }
    }
    /// Largest per-tick horizontal step and the tick it happened at.
    fn max_step(a: &mut Aircraft, secs: f32, sectors: &[Sector], until: impl Fn(&Aircraft) -> bool) -> f32 {
        let dt = 0.05;
        let mut worst: f32 = 0.0;
        for _ in 0..(secs / dt) as usize {
            let before = a.position;
            a.tick_with_sectors(dt, sectors);
            let step = wrap_delta(a.position[0] - before[0]).hypot(wrap_delta(a.position[1] - before[1]));
            worst = worst.max(step);
            if until(a) {
                break;
            }
        }
        worst
    }

    fn on_pattern(a: &Aircraft, sectors: &[Sector]) -> bool {
        let Some(sec) = a.sector(sectors) else { return false };
        let (p, _, _) = a.pattern_pose(sec, a.patrol_angle);
        a.state == State::Cruising
            && a.leg.is_none()
            && wrap_delta(a.position[0] - p[0]).hypot(wrap_delta(a.position[1] - p[1])) < 1.0
    }

    fn cruising_in(sector: &str, model: ModelClass, sectors: &[Sector]) -> Aircraft {
        let mut a = Aircraft::with_model("SKY1", Kind::Agent, model);
        a.assign(sector);
        a.place_on_pattern(sectors);
        a
    }

    #[test]
    fn approach_glides_from_zone_onto_runway_without_jumps() {
        let sectors = default_sectors();
        for sector in ["Build", "Homelab", "AI & Models", "Research"] {
            let mut a = cruising_in(sector, ModelClass::Frontier, &sectors);
            a.land();
            assert_eq!(a.state, State::Landing);
            let mut touchdown = None;
            let mut last_alt = a.altitude;
            let dt = 0.05;
            for _ in 0..(300.0 / dt) as usize {
                let before = a.position;
                a.tick_with_sectors(dt, &sectors);
                let step = wrap_delta(a.position[0] - before[0]).hypot(wrap_delta(a.position[1] - before[1]));
                assert!(step <= a.speed * dt * 1.05, "{sector}: jumped {step}");
                assert!(a.altitude <= last_alt + 1e-3, "{sector}: climbed on approach");
                last_alt = a.altitude;
                if touchdown.is_none() && a.altitude <= 0.0 {
                    touchdown = Some((a.position, a.heading));
                }
                if a.state == State::Landed {
                    break;
                }
            }
            let (pos, hdg) = touchdown.expect("never touched down");
            assert!((pos[0] - TOUCHDOWN[0]).abs() < 0.5 && (pos[1] - TOUCHDOWN[1]).abs() < 0.5,
                "{sector}: touched down at {pos:?}");
            assert!((hdg - RUNWAY_HEADING).abs() < 1e-3, "{sector}: not aligned with runway");
            assert_eq!(a.state, State::Landed, "{sector}: never rolled out");
            assert!(a.position[1] < TOUCHDOWN[1] && a.position[1] > -76.0, "rolled off the runway");
        }
    }

    #[test]
    fn departure_climbs_out_and_curves_onto_pattern_smoothly() {
        let sectors = default_sectors();
        let mut a = Aircraft::with_model("SKY1", Kind::Agent, ModelClass::Local);
        a.assign("Homelab");
        a.position = [RUNWAY_THRESHOLD[0], RUNWAY_THRESHOLD[1], 0.0];
        a.altitude = 0.0;
        a.heading = RUNWAY_HEADING;
        a.state = State::Ready;
        a.release();
        assert_eq!(a.state, State::Takeoff);
        let worst = max_step(&mut a, 200.0, &sectors, |a| on_pattern(a, &sectors));
        assert!(worst <= a.speed * 0.05 * 1.05, "jumped {worst}");
        assert_eq!(a.state, State::Cruising);
        assert!(a.leg.is_none(), "never rejoined the pattern");
        let sec = a.sector(&sectors).unwrap();
        let (p, _, _) = a.pattern_pose(sec, a.patrol_angle);
        assert!((a.position[0] - p[0]).hypot(a.position[1] - p[1]) < 1.0);
        assert!((a.altitude - a.cruise_altitude()).abs() < 4.0, "not at cruise level");
    }

    #[test]
    fn hold_entry_and_exit_stay_on_track() {
        let sectors = default_sectors();
        let mut a = cruising_in("Build", ModelClass::Frontier, &sectors);
        a.tick_with_sectors(0.05, &sectors);
        let heading = a.heading;
        a.hold("awaiting approval");
        a.tick_with_sectors(0.05, &sectors);
        let turn = wrap_angle(a.heading - heading).abs();
        assert!(turn < 0.05, "hold entry snapped heading by {turn}");
        max_step(&mut a, 20.0, &sectors, |_| false);
        a.fly(State::Cruising, None);
        let worst = max_step(&mut a, 200.0, &sectors, |a| on_pattern(a, &sectors));
        assert!(worst <= a.speed * 0.05 * 1.05, "rejoin jumped {worst}");
        assert!(on_pattern(&a, &sectors), "never rejoined the pattern");
    }

    #[test]
    fn finished_thread_lands_and_taxis_to_its_queue_slot() {
        let sectors = default_sectors();
        let mut a = cruising_in("Research", ModelClass::Local, &sectors);
        let (slot, hdg) = TAXIWAY_QUEUE[2];
        a.park_at(GroundSpot { pos: slot, heading: hdg, state: State::Ready });
        assert_eq!(a.state, State::Landing);
        max_step(&mut a, 400.0, &sectors, |a| a.state == State::Ready);
        assert_eq!(a.state, State::Ready);
        assert!((a.position[0] - slot[0]).hypot(a.position[1] - slot[1]) < 0.5);
        assert!((a.heading - hdg).abs() < 1e-3);

        // Thread resumes: taxi out, take off, and fly back to the zone.
        a.fly(State::Cruising, None);
        assert_eq!(a.state, State::Taxi);
        let worst = max_step(&mut a, 400.0, &sectors, |a| on_pattern(a, &sectors));
        assert!(worst <= a.speed * 0.05 * 1.05, "jumped {worst}");
        assert!(on_pattern(&a, &sectors), "never got back to the zone");
    }

    #[test]
    fn go_around_from_final_rejoins_pattern() {
        let sectors = default_sectors();
        let mut a = cruising_in("Build", ModelClass::Local, &sectors);
        a.land();
        max_step(&mut a, 400.0, &sectors, |a| a.approach == Some(Approach::Final));
        a.fly(State::Cruising, None);
        assert_eq!(a.state, State::Cruising);
        let worst = max_step(&mut a, 400.0, &sectors, |a| on_pattern(a, &sectors));
        assert!(worst <= a.speed * 0.05 * 1.05, "jumped {worst}");
        assert!(on_pattern(&a, &sectors), "never rejoined the pattern");
    }
    /// Tick the world until `callsign` is gone; returns (seconds, worst step / speed·dt).
    fn fly_until_gone(world: &mut World, callsign: &str, max_secs: f32) -> (f32, f32) {
        let dt = 0.05;
        let mut worst: f32 = 0.0;
        let mut t = 0.0;
        while t < max_secs {
            let before = world.iter().find(|a| a.callsign == callsign).map(|a| (a.position, a.speed));
            world.tick(dt);
            t += dt;
            let Some((bp, speed)) = before else { break };
            let Some(a) = world.iter().find(|a| a.callsign == callsign) else { return (t, worst) };
            let step = (a.position[0] - bp[0]).hypot(a.position[1] - bp[1]);
            worst = worst.max(step / (speed * dt));
        }
        (t, worst)
    }

    #[test]
    fn done_thread_in_flight_flies_off_the_map() {
        let sectors = default_sectors();
        let mut world = World::new();
        let mut a = cruising_in("Build", ModelClass::Frontier, &sectors);
        a.leave();
        assert_eq!(a.state, State::Outbound);
        world.add(a);
        let (t, worst) = fly_until_gone(&mut world, "SKY1", 120.0);
        assert!(world.iter().all(|a| a.callsign != "SKY1"), "still in the airspace after {t}s");
        assert!(worst <= OUTBOUND_SPEED_FACTOR * 1.01, "jumped {worst}x speed");
    }

    #[test]
    fn done_thread_at_gate_taxis_out_takes_off_and_leaves() {
        let mut world = World::new();
        let mut a = Aircraft::with_model("GT1", Kind::Agent, ModelClass::Local);
        a.state = State::Parked;
        a.position = [HOME_GATES[4][0], HOME_GATES[4][1], 0.0];
        a.altitude = 0.0;
        a.heading = GATE_HEADING;
        a.leave();
        assert_eq!(a.state, State::Taxi);
        world.add(a);
        let mut saw = (false, false);
        for _ in 0..(300.0 / 0.05) as usize {
            world.tick(0.05);
            match world.iter().find(|a| a.callsign == "GT1") {
                Some(a) if a.state == State::Takeoff => saw.0 = true,
                Some(a) if a.state == State::Outbound => saw.1 = true,
                Some(a) => assert_ne!(a.state, State::Cruising, "rejoined a zone instead of leaving"),
                None => break,
            }
        }
        assert!(saw.0 && saw.1, "expected takeoff then outbound, saw {saw:?}");
        assert!(world.iter().all(|a| a.callsign != "GT1"), "never left");
    }

    #[test]
    fn wingmen_peel_off_with_a_done_parent() {
        let sectors = default_sectors();
        let mut world = World::new();
        let parent = cruising_in("Research", ModelClass::Frontier, &sectors);
        let pid = parent.id;
        let start = formation_position(parent.position, parent.heading, false, 0);
        world.add(Aircraft::wingman("SKY1-A", pid, 0, start, parent.heading));
        world.add(parent);
        world.get_mut("SKY1").unwrap().leave();
        world.tick(0.05);
        world.tick(0.05);
        let w = world.iter().find(|a| a.callsign == "SKY1-A").unwrap();
        assert_eq!(w.state, State::Outbound);
        assert_eq!(w.parent_id, None);
        fly_until_gone(&mut world, "SKY1", 120.0);
        fly_until_gone(&mut world, "SKY1-A", 120.0);
        assert!(world.aircraft.is_empty(), "everyone should have left");
    }
}
