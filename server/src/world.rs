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
    /// Ground-movement target (taxiing), not serialised.
    #[serde(skip)]
    taxi_target: Option<[f32; 2]>,
    /// Where taxiing leads next (Ready at the runway, Parked at the gate).
    #[serde(skip)]
    taxi_next: Option<State>,
    /// Ground timer: auto-taxi after landing, hold-short before takeoff.
    #[serde(skip)]
    ground_timer: f32,
    /// Ground speed in m/s (serde default keeps old snapshots deserialisable).
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_speed() -> f32 {
    AGENT_SPEED
}

/// Sim area is 700x700; positions wrap around at the edges.
const AREA_SIZE: f32 = 700.0;
const TAKEOFF_RATE: f32 = 5.0; // m/s climb
const CRUISE_ALTITUDE: f32 = 100.0;
const AGENT_SPEED: f32 = 20.0; // m/s, local models
const FRONTIER_SPEED: f32 = 30.0; // m/s, frontier jets are faster
const DRONE_SPEED: f32 = 7.0; // m/s, drones slow loiter
const FIGHTER_SPEED: f32 = 42.0; // m/s, wingman catch-up speed
const HOLD_TURN_RADIUS: f32 = 55.0; // m, holding orbit radius
const HOLD_DRIFT_SPEED: f32 = 12.0; // m/s along the holding orbit

/// Home airport geometry (matches the client's island rendering).
pub const RUNWAY_MID: [f32; 2] = [66.0, -10.0];
pub const RUNWAY_THRESHOLD: [f32; 2] = [66.0, 36.0];
pub const RUNWAY_HEADING: f32 = -std::f32::consts::FRAC_PI_2; // rolling north (-Z)
/// Six terminal gates along the eastern apron of the home island.
pub const HOME_GATES: [[f32; 2]; 6] = [
    [98.0, -55.0],
    [98.0, -35.0],
    [98.0, -15.0],
    [98.0, 5.0],
    [98.0, 25.0],
    [98.0, 45.0],
];
/// Heading when nose-in at the home terminal (facing east, +X).
pub const GATE_HEADING: f32 = 0.0;

const TAXI_SPEED: f32 = 6.0; // m/s on the ground
const TAXI_DELAY_AFTER_LANDING: f32 = 5.0; // s rolled out on runway, then taxi
const HOLD_SHORT_TIME: f32 = 4.0; // s lined up before rolling

/// Satellite device airport coordinates placed in open water clear of sectors.
pub const SATELLITE_SLOTS: [[f32; 2]; 5] = [
    [-235.0, -210.0], // NW
    [235.0, -205.0],  // NE
    [-10.0, 245.0],   // S
    [-245.0, 240.0],  // SW
    [245.0, 240.0],   // SE
];

/// Wrap a coordinate into [-AREA_SIZE/2, AREA_SIZE/2) so the world is
/// centred on the origin (where the home island sits).
fn wrap_centered(v: f32) -> f32 {
    (v + AREA_SIZE / 2.0).rem_euclid(AREA_SIZE) - AREA_SIZE / 2.0
}

/// Topic sectors: (name, centre [x, z], radius). Client renders these from
/// the snapshot; cruising aircraft patrol inside their sector.
pub const SECTORS: &[(&str, [f32; 2], f32)] = &[
    ("Deploy", [-230.0, 55.0], 100.0),
    ("Refactor", [225.0, 95.0], 100.0),
    ("Monitor", [15.0, -245.0], 100.0),
];

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Aircraft {
    /// New aircraft with a random-ish start position, cruising.
    #[cfg(test)]
    pub fn new(callsign: &str, kind: Kind) -> Self {
        Self::with_model(callsign, kind, ModelClass::Local)
    }

    /// New aircraft with an explicit model class.
    pub fn with_model(callsign: &str, kind: Kind, model_class: ModelClass) -> Self {
        let mut rng = rand::thread_rng();
        let speed = match (kind, model_class) {
            (Kind::Fighter, _) => FIGHTER_SPEED,
            (Kind::Agent, ModelClass::Frontier) => FRONTIER_SPEED,
            (Kind::Agent, ModelClass::Local) => AGENT_SPEED,
            (Kind::Drone, _) => DRONE_SPEED,
        };
        let alt = match kind {
            Kind::Drone => 65.0,
            _ => CRUISE_ALTITUDE,
        };
        Aircraft {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            callsign: callsign.to_string(),
            kind,
            model_class,
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
            taxi_target: None,
            taxi_next: None,
            ground_timer: 0.0,
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

    /// Enter a holding pattern with a reason (e.g. "awaiting approval").
    pub fn hold(&mut self, reason: &str) {
        if self.state != State::Holding {
            self.orbit_center = None; // re-derive the orbit from current pos
        }
        self.state = State::Holding;
        self.hold_reason = Some(reason.to_string());
    }

    /// Cleared for takeoff: taxi out, hold short, then roll.
    pub fn release(&mut self) {
        self.orbit_center = None;
        self.hold_reason = None;
        match self.state {
            // On the ground: taxi to the runway threshold first.
            State::Parked | State::Landed | State::Taxi => {
                self.state = State::Taxi;
                self.taxi_target = Some(RUNWAY_THRESHOLD);
                self.taxi_next = Some(State::Ready);
                self.ground_timer = 0.0;
            }
            // Already lined up: roll now.
            State::Ready => {
                self.state = State::Takeoff;
                self.heading = RUNWAY_HEADING;
            }
            // Airborne hold: climb straight out.
            _ => self.state = State::Takeoff,
        }
    }

    /// Touch down: roll out, then taxi to a terminal gate.
    pub fn land(&mut self) {
        self.state = State::Landed;
        self.altitude = 0.0;
        self.position[2] = 0.0;
        self.hold_reason = None;
        self.orbit_center = None;
        // Touch down mid-runway, rolling north.
        self.position[0] = RUNWAY_MID[0];
        self.position[1] = RUNWAY_MID[1];
        self.heading = RUNWAY_HEADING;
        self.ground_timer = 0.0;
    }

    /// Advance the simulation by `dt` seconds (for non-wingman aircraft).
    pub fn tick(&mut self, dt: f32) {
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
                if self.altitude < 40.0 {
                    self.altitude = CRUISE_ALTITUDE;
                    self.position[2] = CRUISE_ALTITUDE;
                }
            }
            State::Cruising => {
                let target_alt = if self.kind == Kind::Drone {
                    65.0
                } else {
                    CRUISE_ALTITUDE
                };
                self.altitude = target_alt;
                self.position[2] = target_alt;

                if let Some(topic) = &self.topic {
                    if let Some((_, centre, radius)) =
                        SECTORS.iter().find(|(name, _, _)| name == topic)
                    {
                        let r = (self.id as f32 * 23.7 % 24.0) + radius * 0.48;
                        let omega = self.speed / r;
                        self.patrol_angle += omega * dt;
                        let nx = centre[0] + self.patrol_angle.cos() * r;
                        let ny = centre[1] + self.patrol_angle.sin() * r;
                        self.heading = self.patrol_angle + std::f32::consts::FRAC_PI_2;
                        self.position[0] = wrap_centered(nx);
                        self.position[1] = wrap_centered(ny);
                        return;
                    }
                }
                let dx = self.heading.cos() * self.speed * dt;
                let dy = self.heading.sin() * self.speed * dt;
                self.position[0] = wrap_centered(self.position[0] + dx);
                self.position[1] = wrap_centered(self.position[1] + dy);
            }
            State::Landed => {
                // Rolled out on the runway; after a short pause, taxi to a gate.
                self.altitude = 0.0;
                self.position[2] = 0.0;
                self.ground_timer += dt;
                if self.ground_timer >= TAXI_DELAY_AFTER_LANDING {
                    let gate = HOME_GATES[(self.id as usize) % HOME_GATES.len()];
                    self.taxi_target = Some(gate);
                    self.taxi_next = Some(State::Parked);
                    self.state = State::Taxi;
                }
            }
            State::Taxi => {
                self.altitude = 0.0;
                self.position[2] = 0.0;
                if let Some(target) = self.taxi_target {
                    let dx = target[0] - self.position[0];
                    let dy = target[1] - self.position[1];
                    let dist = dx.hypot(dy);
                    let step = (TAXI_SPEED * dt).min(dist);
                    if step >= dist {
                        self.position[0] = target[0];
                        self.position[1] = target[1];
                        self.state = self.taxi_next.unwrap_or(State::Parked);
                        self.taxi_target = None;
                        self.ground_timer = 0.0;
                        if self.state == State::Ready {
                            self.heading = RUNWAY_HEADING;
                        } else if self.state == State::Parked {
                            self.heading = GATE_HEADING;
                        }
                    } else {
                        self.heading = dy.atan2(dx);
                        self.position[0] += dx / dist * step;
                        self.position[1] += dy / dist * step;
                    }
                } else {
                    self.state = State::Parked;
                }
            }
            State::Ready => {
                self.altitude = 0.0;
                self.position[2] = 0.0;
                self.ground_timer += dt;
                if self.ground_timer >= HOLD_SHORT_TIME {
                    self.state = State::Takeoff;
                }
            }
            State::Takeoff => {
                self.altitude += TAKEOFF_RATE * dt;
                self.position[2] = self.altitude;
                let dx = self.heading.cos() * self.speed * dt;
                let dy = self.heading.sin() * self.speed * dt;
                self.position[0] = wrap_centered(self.position[0] + dx);
                self.position[1] = wrap_centered(self.position[1] + dy);
                if self.altitude >= CRUISE_ALTITUDE {
                    self.altitude = CRUISE_ALTITUDE;
                    self.position[2] = CRUISE_ALTITUDE;
                    self.state = State::Cruising;
                }
            }
            State::Landing => {
                self.altitude = (self.altitude - TAKEOFF_RATE * dt).max(0.0);
                self.position[2] = self.altitude;
                if self.altitude <= 0.0 {
                    self.state = State::Landed;
                }
            }
            State::Parked => {
                self.altitude = 0.0;
                self.position[2] = 0.0;
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

/// A topic sector, serialised for the client to render.
#[derive(Clone, Debug, Serialize)]
pub struct Sector {
    pub name: String,
    pub center: [f32; 2],
    pub radius: f32,
}

#[derive(Clone, Debug)]
pub struct World {
    pub aircraft: Vec<Aircraft>,
    pub airports: Vec<Airport>,
}

impl Default for World {
    fn default() -> Self {
        Self {
            aircraft: Vec::new(),
            airports: vec![Airport::home("local", "Tower")],
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

    /// Advance every aircraft by `dt` seconds. Non-wingman aircraft move first,
    /// then fighter wingmen smoothly track their parent's wing formation.
    pub fn tick(&mut self, dt: f32) {
        for a in &mut self.aircraft {
            if a.parent_id.is_none() {
                a.tick(dt);
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
                a.tick(dt);
                continue;
            };
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
    }

    pub fn snapshot(&self) -> WorldSnapshot {
        WorldSnapshot {
            aircraft: self.aircraft.clone(),
            sectors: SECTORS
                .iter()
                .map(|(name, centre, radius)| Sector {
                    name: name.to_string(),
                    center: *centre,
                    radius: *radius,
                })
                .collect(),
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
}
