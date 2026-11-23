use rand::Rng;
use serde::{Deserialize, Serialize};

/// What kind of entity this is: a piloted agent or an autonomous drone.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Agent,
    Drone,
}

/// Lifecycle state of an aircraft in the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Cruising,
    Holding,
    Landed,
    /// Rolling on the ground between runway and parking.
    Taxi,
    /// Parked at a stand, engines off.
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aircraft {
    pub id: u64,
    pub callsign: String,
    pub kind: Kind,
    /// Airframe class: what model powers this agent (agents only).
    #[serde(default)]
    pub model_class: ModelClass,
    pub state: State,
    pub position: [f32; 3],
    pub heading: f32,
    pub altitude: f32,
    pub hold_reason: Option<String>,
    /// Topic sector this aircraft works in (None = unassigned).
    #[serde(default)]
    pub topic: Option<String>,
    /// Human-readable subject (thread title for T3 aircraft).
    #[serde(default)]
    pub title: Option<String>,
    /// Holding-orbit centre (derived on entry, not serialised).
    #[serde(skip)]
    orbit_center: Option<[f32; 2]>,
    /// Patrol orbit angle for sector-bound aircraft.
    #[serde(default)]
    patrol_angle: f32,
    /// Ground-movement target (taxiing), not serialised.
    #[serde(skip)]
    taxi_target: Option<[f32; 2]>,
    /// Where taxiing leads next (Ready at the runway, Parked at the stand).
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

/// Sim area is 1000x1000; positions wrap around at the edges.
const AREA_SIZE: f32 = 700.0;
const TAKEOFF_RATE: f32 = 5.0; // m/s climb
const CRUISE_ALTITUDE: f32 = 120.0;
const AGENT_SPEED: f32 = 20.0; // m/s, local models
const FRONTIER_SPEED: f32 = 32.0; // m/s, frontier jets are faster
const DRONE_SPEED: f32 = 6.0; // m/s, drones always slow loiter
const ORBIT_RADIUS: f32 = 40.0;
const HOLD_TURN_RADIUS: f32 = 60.0; // m, holding orbit radius
const HOLD_DRIFT_SPEED: f32 = 12.0; // m/s along the holding orbit

/// Airport geometry (matches the client's island rendering).
const RUNWAY_MID: [f32; 2] = [88.0, -28.0];
const RUNWAY_THRESHOLD: [f32; 2] = [88.0, -70.0];
const RUNWAY_HEADING: f32 = -std::f32::consts::FRAC_PI_2; // rolling north
const PARKING_STANDS: [[f32; 2]; 3] = [[106.0, -4.0], [106.0, 16.0], [72.0, 14.0]];
const TAXI_SPEED: f32 = 5.0; // m/s on the ground
const TAXI_DELAY_AFTER_LANDING: f32 = 6.0; // s parked on runway, then taxi
const HOLD_SHORT_TIME: f32 = 5.0; // s lined up before rolling

/// Wrap a coordinate into [-AREA_SIZE/2, AREA_SIZE/2) so the world is
/// centred on the origin (where the island sits).
fn wrap_centered(v: f32) -> f32 {
    (v + AREA_SIZE / 2.0).rem_euclid(AREA_SIZE) - AREA_SIZE / 2.0
}

/// Topic sectors: (name, centre [x, z], radius). Client renders these from
/// the snapshot; cruising aircraft patrol inside their sector.
pub const SECTORS: &[(&str, [f32; 2], f32)] = &[
    ("Deploy", [-240.0, 60.0], 130.0),
    ("Refactor", [210.0, 110.0], 130.0),
    ("Monitor", [30.0, -250.0], 130.0),
];

/// The graveyard apron: where stale threads are queued, parked and faded.
pub const GRAVEYARD_CENTER: [f32; 2] = [150.0, -70.0];

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Aircraft {
    /// New aircraft with a random-ish start position, cruising.
    pub fn new(callsign: &str, kind: Kind) -> Self {
        Self::with_model(callsign, kind, ModelClass::Local)
    }

    /// New aircraft with an explicit model class.
    pub fn with_model(callsign: &str, kind: Kind, model_class: ModelClass) -> Self {
        let mut rng = rand::thread_rng();
        let speed = match (kind, model_class) {
            (Kind::Agent, ModelClass::Frontier) => FRONTIER_SPEED,
            (Kind::Agent, ModelClass::Local) => AGENT_SPEED,
            (Kind::Drone, _) => DRONE_SPEED,
        };
        Aircraft {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            callsign: callsign.to_string(),
            kind,
            model_class,
            state: State::Cruising,
            position: [
                rng.gen_range(0.0..AREA_SIZE),
                rng.gen_range(0.0..AREA_SIZE),
                CRUISE_ALTITUDE,
            ],
            heading: rng.gen_range(0.0..std::f32::consts::TAU),
            altitude: CRUISE_ALTITUDE,
            hold_reason: None,
            topic: None,
            title: None,
            orbit_center: None,
            patrol_angle: 0.0,
            taxi_target: None,
            taxi_next: None,
            ground_timer: 0.0,
            speed,
        }
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

    /// Touch down: roll out, then taxi to the parking stand.
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

    /// Advance the simulation by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        match self.state {
            State::Holding => {
                // Proper racetrack-free holding pattern: a clean circle.
                // Position rides a fixed-radius orbit around a per-aircraft
                // centre; heading is exactly the orbit tangent, so the
                // aircraft never appears to pirouette in place.
                if self.orbit_center.is_none() {
                    // Enter the orbit 90° right of the current heading.
                    let r = HOLD_TURN_RADIUS;
                    let cx = self.position[0] + self.heading.cos() * r;
                    let cy = self.position[1] + self.heading.sin() * r;
                    self.orbit_center = Some([cx, cy]);
                }
                let [cx, cy] = self.orbit_center.unwrap();
                let angle = (self.position[1] - cy).atan2(self.position[0] - cx);
                let omega = HOLD_DRIFT_SPEED / HOLD_TURN_RADIUS; // rad/s
                let angle = angle + omega * dt;
                let nx = cx + angle.cos() * HOLD_TURN_RADIUS;
                let ny = cy + angle.sin() * HOLD_TURN_RADIUS;
                // Tangent (CCW): derivative of the circle.
                self.heading = angle + std::f32::consts::FRAC_PI_2;
                self.position[0] = wrap_centered(nx);
                self.position[1] = wrap_centered(ny);
            }
            State::Cruising => {
                // Assigned aircraft fly a smooth circular patrol orbit inside
                // their sector; unassigned aircraft transit straight and wrap.
                if let Some(topic) = &self.topic {
                    if let Some((_, centre, radius)) =
                        SECTORS.iter().find(|(name, _, _)| name == topic)
                    {
                        // Per-aircraft patrol orbit: wide, gentle circuits.
                        let r = (self.id as f32 * 37.7 % 30.0) + radius * 0.45;
                        let omega = self.speed / r; // rad/s
                        self.patrol_angle += omega * dt;
                        let nx = centre[0] + self.patrol_angle.cos() * r;
                        let ny = centre[1] + self.patrol_angle.sin() * r;
                        // Tangent of the CCW orbit.
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
            State::Takeoff => {
                self.altitude += TAKEOFF_RATE * dt;
                self.position[2] = self.altitude;
                if self.altitude >= CRUISE_ALTITUDE {
                    self.altitude = self.altitude.min(CRUISE_ALTITUDE);
                    self.position[2] = self.altitude;
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
            State::Landed => {
                // Rolled out on the runway; after a short pause, taxi to the stand.
                self.ground_timer += dt;
                if self.ground_timer >= TAXI_DELAY_AFTER_LANDING {
                    let stand = PARKING_STANDS[(self.id as usize) % PARKING_STANDS.len()];
                    self.taxi_target = Some(stand);
                    self.taxi_next = Some(State::Parked);
                    self.state = State::Taxi;
                }
            }
            State::Taxi => {
                if let Some(target) = self.taxi_target {
                    let dx = target[0] - self.position[0];
                    let dy = target[1] - self.position[1];
                    let dist = dx.hypot(dy);
                    let step = (TAXI_SPEED * dt).min(dist);
                    if step >= dist {
                        // Arrived: Parked at the stand, or Ready at the threshold.
                        self.position[0] = target[0];
                        self.position[1] = target[1];
                        self.state = self.taxi_next.unwrap_or(State::Parked);
                        self.taxi_target = None;
                        self.ground_timer = 0.0;
                        if self.state == State::Ready {
                            self.heading = RUNWAY_HEADING;
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
                // Lined up, holding short for clearance.
                self.ground_timer += dt;
                if self.ground_timer >= HOLD_SHORT_TIME {
                    self.state = State::Takeoff;
                }
            }
            State::Takeoff => {
                self.altitude += TAKEOFF_RATE * dt;
                self.position[2] = self.altitude;
                // Roll forward along the runway while climbing.
                let dx = self.heading.cos() * self.speed * dt;
                let dy = self.heading.sin() * self.speed * dt;
                self.position[0] = wrap_centered(self.position[0] + dx);
                self.position[1] = wrap_centered(self.position[1] + dy);
                if self.altitude >= CRUISE_ALTITUDE {
                    self.altitude = self.altitude.min(CRUISE_ALTITUDE);
                    self.position[2] = self.altitude;
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
                // Engines off at the stand until clearance arrives.
                self.altitude = 0.0;
                self.position[2] = 0.0;
            }
        }
    }
}

/// Snapshot of the whole world, ready to serialise over the wire.
#[derive(Clone, Debug, Serialize)]
pub struct WorldSnapshot {
    pub aircraft: Vec<Aircraft>,
    pub sectors: Vec<Sector>,
    pub graveyard: Option<GraveyardInfo>,
}

/// Where the client draws the boneyard apron for retired threads.
#[derive(Clone, Debug, Serialize)]
pub struct GraveyardInfo {
    pub center: [f32; 2],
}

/// A topic sector, serialised for the client to render.
#[derive(Clone, Debug, Serialize)]
pub struct Sector {
    pub name: String,
    pub center: [f32; 2],
    pub radius: f32,
}

#[derive(Clone, Debug, Default)]
pub struct World {
    pub aircraft: Vec<Aircraft>,
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

    /// Advance every aircraft by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        for a in &mut self.aircraft {
            a.tick(dt);
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
            graveyard: Some(GraveyardInfo {
                center: GRAVEYARD_CENTER,
            }),
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
}
