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
    /// True after climb-out: fly a smooth transit to the sector's patrol path
    /// instead of snapping onto it.
    #[serde(skip)]
    joining: bool,
    /// Landing: false while inbound to the final approach fix, true on final.
    #[serde(skip)]
    on_final: bool,
    /// Arrival hand-off: after rollout, backtrack the runway and park here
    /// `(position, heading)` in `State::Ready` instead of taxiing to a gate.
    #[serde(skip)]
    arrival_slot: Option<([f32; 2], f32)>,
    /// Set by `World::tick` while another aircraft is using the runway.
    #[serde(skip)]
    runway_hold: bool,
    /// Ground speed in m/s (serde default keeps old snapshots deserialisable).
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_speed() -> f32 {
    AGENT_SPEED
}

/// Sim area is 820x820; positions wrap around at the edges.
const AREA_SIZE: f32 = 820.0;
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
const TAXI_DELAY_AFTER_LANDING: f32 = 5.0; // s rolled out on runway, then taxi
const HOLD_SHORT_TIME: f32 = 4.0; // s lined up before rolling
/// Altitude at which a climbing aircraft turns toward its sector.
const CLIMB_OUT_ALT: f32 = 45.0;
/// Final approach fix on the extended Runway 36 centerline (south of the island).
const FINAL_APPROACH_FIX: [f32; 2] = [RUNWAY_THRESHOLD[0], RUNWAY_THRESHOLD[1] + 230.0];
/// Altitude crossing the final approach fix.
const APPROACH_ALT: f32 = 42.0;
/// Main-gear touchdown point just past the threshold.
const TOUCHDOWN_Z: f32 = RUNWAY_THRESHOLD[1] - 12.0;
const ROLLOUT_DECEL: f32 = 5.0; // m/s^2 braking after touchdown

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
            departing_forever: false,
            departure_heading: None,
            sep_offset: [0.0, 0.0, 0.0],
            joining: false,
            on_final: false,
            arrival_slot: None,
            runway_hold: false,
            speed,
        }
    }

    /// Nominal cruise speed for this airframe class.
    pub fn cruise_speed(&self) -> f32 {
        match (self.kind, self.model_class) {
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

    /// Fly an approach to Runway 36, touch down, roll out and backtrack to
    /// `slot` (if any). Returns false if not airborne.
    pub fn begin_approach(&mut self, slot: Option<([f32; 2], f32)>) -> bool {
        if !matches!(self.state, State::Cruising | State::Holding | State::Takeoff | State::Landing)
            || (self.state == State::Takeoff && self.altitude <= 0.0)
        {
            return false;
        }
        if self.state != State::Landing {
            self.on_final = false;
        }
        self.state = State::Landing;
        self.hold_reason = None;
        self.orbit_center = None;
        self.joining = false;
        self.cleared_for_takeoff = false;
        self.arrival_slot = slot;
        true
    }

    /// Abort an arrival and climb back out to the sector (thread started again).
    pub fn go_around(&mut self) {
        self.arrival_slot = None;
        if self.state == State::Landing {
            self.state = State::Cruising;
            self.joining = true;
        } else if matches!(self.state, State::Landed | State::Taxi | State::Ready) {
            self.release();
        }
    }

    /// True while rolling, on short final, or backtracking on Runway 36.
    fn occupies_runway(&self) -> bool {
        match self.state {
            State::Takeoff => self.altitude < 20.0,
            State::Landing => self.on_final && self.position[1] < TOUCHDOWN_Z + 120.0,
            State::Landed => true,
            State::Departing => self.altitude <= 0.0,
            State::Taxi => {
                self.arrival_slot.is_some()
                    && (self.position[0] - RUNWAY_THRESHOLD[0]).abs() < 8.0
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

    /// Fly the Runway 36 approach: inbound to the final approach fix, then
    /// pursue the extended centerline down a glidepath to touchdown. A badly
    /// lined-up approach goes around and tries again.
    fn fly_approach(&mut self, dt: f32) {
        let target_speed = self.approach_speed();
        self.speed += (target_speed - self.speed).clamp(-4.0 * dt, 4.0 * dt);
        let (px, pz) = (self.position[0], self.position[1]);

        if !self.on_final {
            let dx = wrap_centered(FINAL_APPROACH_FIX[0] - px);
            let dz = wrap_centered(FINAL_APPROACH_FIX[1] - pz);
            let dist = dx.hypot(dz);
            self.turn_toward(dz.atan2(dx), (0.6 + 20.0 / dist.max(1.0)) * dt);
            // Descend (or climb after a go-around) to cross the fix at approach altitude.
            let time_left = (dist / self.speed.max(1.0)).max(1.0);
            let rate = ((self.altitude - APPROACH_ALT) / time_left).clamp(-6.0, 14.0);
            self.altitude -= rate * dt;
            if dist < 30.0 {
                self.on_final = true;
            }
        } else {
            // Pure pursuit of a point on the centerline ahead lines us up smoothly.
            let aim_z = (pz - 70.0).max(TOUCHDOWN_Z - 60.0);
            self.turn_toward((aim_z - pz).atan2(RUNWAY_THRESHOLD[0] - px), 0.9 * dt);
            let frac = ((pz - TOUCHDOWN_Z) / (FINAL_APPROACH_FIX[1] - TOUCHDOWN_Z)).clamp(0.0, 1.0);
            let glide = APPROACH_ALT * frac;
            if self.altitude > glide {
                self.altitude = (self.altitude - 10.0 * dt).max(glide);
            }
            if pz <= TOUCHDOWN_Z + 1.0 {
                let mut dh = self.heading - RUNWAY_HEADING;
                while dh > std::f32::consts::PI {
                    dh -= std::f32::consts::TAU;
                }
                while dh < -std::f32::consts::PI {
                    dh += std::f32::consts::TAU;
                }
                let lined_up = (px - RUNWAY_THRESHOLD[0]).abs() < 5.0 && dh.abs() < 0.2;
                if lined_up && self.altitude < 2.0 {
                    // Touchdown: roll out along the centerline.
                    self.state = State::Landed;
                    self.on_final = false;
                    self.altitude = 0.0;
                    self.position[0] = RUNWAY_THRESHOLD[0];
                    self.position[2] = 0.0;
                    self.heading = RUNWAY_HEADING;
                    self.ground_timer = 0.0;
                    return;
                }
                // Not lined up: go around and fly the approach again.
                self.on_final = false;
            }
        }
        self.advance(dt);
        self.position[2] = self.altitude;
    }

    /// Fly a smooth transit onto a moving patrol-path point after climb-out.
    /// Returns true while still joining (position already updated).
    fn join_path(&mut self, target: [f32; 2], target_alt: f32, path_heading: f32, dt: f32) -> bool {
        let dx = wrap_centered(target[0] - self.position[0]);
        let dz = wrap_centered(target[1] - self.position[1]);
        let dist = dx.hypot(dz);
        let transit = self.cruise_speed() * 2.4;
        self.speed += (transit - self.speed).clamp(-5.0 * dt, 5.0 * dt);
        self.altitude += (target_alt - self.altitude).clamp(-10.0 * dt, 10.0 * dt);
        if dist > 40.0 {
            self.turn_toward(dz.atan2(dx), (0.55 + 30.0 / dist) * dt);
            self.advance(dt);
        } else {
            // Close in: ease onto the path and match its heading.
            let k = (2.0 * dt).min(1.0);
            self.position[0] = wrap_centered(self.position[0] + dx * k);
            self.position[1] = wrap_centered(self.position[1] + dz * k);
            self.turn_toward(path_heading, 1.2 * dt);
            if dist < 2.0 && (target_alt - self.altitude).abs() < 2.0 {
                self.joining = false;
                self.speed = self.cruise_speed();
                return false;
            }
        }
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
    }

    /// Start taxiing to `dest` via the yellow taxiway centerline so aircraft
    /// never cut diagonally across parked planes at neighboring gates.
    pub fn taxi_to(&mut self, dest: [f32; 2], next_state: State) {
        self.state = State::Taxi;
        self.taxi_next = Some(next_state);
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

    /// The final destination of any in-progress taxi, if taxiing.
    pub fn taxi_destination(&self) -> Option<[f32; 2]> {
        self.taxi_waypoints.last().copied()
    }

    /// Cleared for takeoff: taxi out via the taxiway, hold short, then roll.
    pub fn release(&mut self) {
        self.orbit_center = None;
        self.hold_reason = None;
        self.cleared_for_takeoff = true;
        self.arrival_slot = None;
        match self.state {
            // On the ground: taxi to the runway threshold, hold short, then roll.
            State::Parked | State::Landed | State::Taxi | State::Ready => {
                let d = (self.position[0] - RUNWAY_THRESHOLD[0])
                    .hypot(self.position[1] - RUNWAY_THRESHOLD[1]);
                self.speed = 0.0;
                if d < 6.0 {
                    self.state = State::Ready;
                    self.heading = RUNWAY_HEADING;
                    self.ground_timer = 0.0;
                } else {
                    self.route_to_runway(State::Ready);
                }
            }
            // Airborne hold: climb straight out.
            _ => self.state = State::Takeoff,
        }
    }

    /// Taxi to the Runway 36 threshold along the apron lane / Taxiway Alpha.
    fn route_to_runway(&mut self, next: State) {
        self.state = State::Taxi;
        self.taxi_next = Some(next);
        self.ground_timer = 0.0;
        self.taxi_waypoints.clear();
        let cur = [self.position[0], self.position[1]];
        if cur[0] > 96.0 {
            // Gate concourse: pull back onto apron centerline, south to connector, west to runway.
            if (cur[0] - APRON_X).abs() > 2.0 {
                self.taxi_waypoints.push([APRON_X, cur[1]]);
            }
            if (cur[1] - RUNWAY_THRESHOLD[1]).abs() > 2.0 {
                self.taxi_waypoints.push([APRON_X, RUNWAY_THRESHOLD[1]]);
            }
            self.taxi_waypoints.push([TAXIWAY_X, RUNWAY_THRESHOLD[1]]);
        } else if cur[0] > 64.0 {
            // Taxiway Alpha or south holding bay.
            if (cur[1] - RUNWAY_THRESHOLD[1]).abs() > 2.0 {
                if (cur[0] - TAXIWAY_X).abs() > 2.0 {
                    self.taxi_waypoints.push([TAXIWAY_X, cur[1]]);
                }
                self.taxi_waypoints.push([TAXIWAY_X, RUNWAY_THRESHOLD[1]]);
            }
        }
        // From the runway itself this simply backtracks down the centerline.
        self.taxi_waypoints.push(RUNWAY_THRESHOLD);
    }

    /// Touch down: roll out, then wait for World::tick to assign a free gate.
    pub fn land(&mut self) {
        self.state = State::Landed;
        self.speed = 0.0;
        self.on_final = false;
        self.joining = false;
        self.arrival_slot = None;
        self.altitude = 0.0;
        self.position[2] = 0.0;
        self.hold_reason = None;
        self.orbit_center = None;
        self.cleared_for_takeoff = false;
        self.departing_forever = false;
        self.departure_heading = None;
        self.taxi_waypoints.clear();
        self.taxi_next = None;
        // Touch down mid-runway, rolling north.
        self.position[0] = RUNWAY_MID[0];
        self.position[1] = RUNWAY_MID[1];
        self.heading = RUNWAY_HEADING;
        self.ground_timer = 0.0;
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
        self.cleared_for_takeoff = false;
        self.topic = None;
        self.taxi_waypoints.clear();
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
            State::Cruising | State::Holding | State::Takeoff | State::Landing => {
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
                let dist_to_thresh =
                    (cur[0] - RUNWAY_THRESHOLD[0]).hypot(cur[1] - RUNWAY_THRESHOLD[1]);
                let on_runway = (cur[0] - RUNWAY_THRESHOLD[0]).abs() < 8.0
                    && cur[1] <= RUNWAY_THRESHOLD[1] + 6.0;

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

                // Route cleanly along the apron lane / Taxiway Alpha to Runway 36 threshold.
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
        match self.state {
            State::Holding => {
                if self.orbit_center.is_none() {
                    let r = HOLD_TURN_RADIUS;
                    let cx = self.position[0] + self.heading.cos() * r;
                    let cy = self.position[1] + self.heading.sin() * r;
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
                if self.altitude < 40.0 {
                    self.altitude = hold_alt;
                    self.position[2] = hold_alt;
                }
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
                        let target_alt = (base_alt + wave + self.sep_offset[2]).max(30.0);
                        if self.joining && self.join_path(target, target_alt, path_heading, dt) {
                            return;
                        }
                        self.heading = path_heading;
                        self.position[0] = wrap_centered(target[0]);
                        self.position[1] = wrap_centered(target[1]);
                        self.altitude = target_alt;
                        self.position[2] = self.altitude;
                        return;
                    }
                }
                if self.joining {
                    // Unassigned: climb to cruise on the current heading.
                    self.altitude += (base_alt - self.altitude).clamp(-10.0 * dt, 10.0 * dt);
                    self.speed += (self.cruise_speed() - self.speed).clamp(-5.0 * dt, 5.0 * dt);
                    self.joining = (base_alt - self.altitude).abs() > 0.5;
                } else {
                    self.altitude = base_alt;
                }
                self.position[2] = self.altitude;
                let dx = self.heading.cos() * self.speed * dt;
                let dy = self.heading.sin() * self.speed * dt;
                self.position[0] = wrap_centered(self.position[0] + dx);
                self.position[1] = wrap_centered(self.position[1] + dy);
            }
            State::Landed => {
                // Decelerate down the runway; once stopped, World::tick assigns a
                // free gate (or the arrival slot) after the landing pause.
                self.altitude = 0.0;
                self.position[2] = 0.0;
                if self.speed > 0.3 {
                    self.speed = (self.speed - ROLLOUT_DECEL * dt).max(0.0);
                    self.advance(dt);
                } else {
                    self.speed = 0.0;
                    self.ground_timer += dt;
                }
            }
            State::Taxi => {
                self.altitude = 0.0;
                self.position[2] = 0.0;
                let taxi_speed = if self.departing_forever {
                    DEPARTURE_TAXI_SPEED
                } else {
                    TAXI_SPEED
                };
                let mut rem = taxi_speed * dt;
                while rem > 0.0 {
                    let Some(&target) = self.taxi_waypoints.first() else {
                        self.state = self.taxi_next.take().unwrap_or(State::Parked);
                        if self.state == State::Departing {
                            self.heading = RUNWAY_HEADING;
                            self.ground_timer = 0.0;
                            self.speed = 6.0;
                        }
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
                            self.state = self.taxi_next.take().unwrap_or(State::Parked);
                            self.ground_timer = 0.0;
                            if let (State::Ready, Some((_, h))) = (self.state, self.arrival_slot) {
                                self.heading = h;
                            } else if self.state == State::Ready {
                                self.heading = RUNWAY_HEADING;
                            } else if self.state == State::Parked {
                                self.heading = GATE_HEADING;
                            } else if self.state == State::Departing {
                                self.heading = RUNWAY_HEADING;
                                self.speed = 6.0;
                            }
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
                    if self.speed >= self.liftoff_speed() || self.position[1] <= RUNWAY_LIFTOFF_Z {
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

                // Phase 1: Takeoff ground roll along Runway 36 before rotation.
                if self.altitude <= 0.0 && self.position[1] > RUNWAY_LIFTOFF_Z && self.ground_timer < 8.0 {
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
                    if self.position[1] <= RUNWAY_LIFTOFF_Z
                        || (self.speed >= liftoff_speed * 0.92 && self.position[1] <= 8.0)
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

        // Runway occupancy: hold departures short while anyone else is rolling,
        // on short final, or backtracking after landing.
        let runway_users: Vec<u64> = self
            .aircraft
            .iter()
            .filter(|a| a.parent_id.is_none() && a.occupies_runway())
            .map(|a| a.id)
            .collect();
        for a in &mut self.aircraft {
            a.runway_hold = runway_users.iter().any(|&id| id != a.id);
        }

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
            // Arrivals backtrack the runway to their taxiway slot instead of a gate.
            if let Some(a) = self
                .aircraft
                .iter_mut()
                .find(|a| a.id == id && a.arrival_slot.is_some())
            {
                let (slot, _) = a.arrival_slot.unwrap();
                a.state = State::Taxi;
                a.taxi_next = Some(State::Ready);
                a.ground_timer = 0.0;
                a.taxi_waypoints = vec![RUNWAY_THRESHOLD, slot];
                continue;
            }
            if let Some(gate) = self.find_free_gate(Some(id), id as usize) {
                if let Some(a) = self.aircraft.iter_mut().find(|a| a.id == id) {
                    a.taxi_to(gate, State::Parked);
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

        // TCAS airborne separation: smoothly sidestep & climb/descend away from
        // conflicting traffic, then enforce hard 3D clearance so no two airborne
        // planes can ever overlap.
        self.enforce_air_separation(dt);
    }

    fn enforce_air_separation(&mut self, dt: f32) {
        // Takeoffs and approaches are sequenced by the tower, not TCAS (its
        // 30u altitude floor would yank them off the glidepath).
        let is_airborne = |s: State| matches!(s, State::Cruising | State::Holding);
        let is_formation_pair = |a: &Aircraft, b: &Aircraft| {
            a.parent_id == Some(b.id)
                || b.parent_id == Some(a.id)
                || (a.parent_id.is_some() && a.parent_id == b.parent_id)
        };

        // Gently relax previous TCAS offsets back toward zero when clear.
        let decay = (-0.45 * dt).exp();
        for a in &mut self.aircraft {
            if is_airborne(a.state) {
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
            if !is_airborne(self.aircraft[i].state) {
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
        arriving.land();
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
        overflow.land();
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
            assert!((p[0] - RUNWAY_THRESHOLD[0]).abs() < 0.01);
            assert!((p[1] - TOUCHDOWN_Z).abs() < 3.0, "touched down at {p:?}");
            // Rollout stops on the runway (which ends at z = -77).
            for _ in 0..400 {
                a.tick(0.05);
            }
            assert_eq!(a.speed, 0.0);
            assert!(a.position[1] > -70.0, "overran the runway: {:?}", a.position);
        }
    }
}
