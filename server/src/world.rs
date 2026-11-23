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
const HOLD_TURN_RATE: f32 = 0.4; // rad/s turn rate while holding
const HOLD_DRIFT_SPEED: f32 = ORBIT_RADIUS * 0.1; // m/s drift while holding

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

/// Turn at most this many rad/s toward the sector centre when leaving it.
const PATROL_TURN_RATE: f32 = 0.5;

/// Steer heading toward a target point (shortest arc, limited turn rate).
fn steer_toward(heading: &mut f32, target: [f32; 2], pos: [f32; 2], dt: f32) {
    let desired = (target[1] - pos[1]).atan2(target[0] - pos[0]);
    let mut dh = desired - *heading;
    while dh > std::f32::consts::PI {
        dh -= std::f32::consts::TAU;
    }
    while dh < -std::f32::consts::PI {
        dh += std::f32::consts::TAU;
    }
    let turn = PATROL_TURN_RATE * dt;
    *heading += dh.clamp(-turn, turn);
}

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
            speed,
        }
    }

    /// Assign this aircraft to a topic sector.
    pub fn assign(&mut self, topic: &str) {
        self.topic = Some(topic.to_string());
    }

    /// Enter a holding pattern with a reason (e.g. "awaiting approval").
    pub fn hold(&mut self, reason: &str) {
        self.state = State::Holding;
        self.hold_reason = Some(reason.to_string());
    }

    /// Cleared for takeoff: climb until reaching cruise altitude.
    pub fn release(&mut self) {
        self.state = State::Takeoff;
        self.hold_reason = None;
    }

    /// Touch down: sit still at ground level.
    pub fn land(&mut self) {
        self.state = State::Landed;
        self.altitude = 0.0;
        self.hold_reason = None;
    }

    /// Advance the simulation by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        match self.state {
            State::Holding => {
                // Circle a small orbit: advance heading and drift along it.
                self.heading = (self.heading + dt * HOLD_TURN_RATE) % std::f32::consts::TAU;
                let dx = self.heading.cos() * HOLD_DRIFT_SPEED * dt;
                let dy = self.heading.sin() * HOLD_DRIFT_SPEED * dt;
                self.position[0] = wrap_centered(self.position[0] + dx);
                self.position[1] = wrap_centered(self.position[1] + dy);
            }
            State::Cruising => {
                // Patrol inside the topic sector: steer back when near its edge.
                if let Some(topic) = &self.topic {
                    if let Some((_, centre, radius)) =
                        SECTORS.iter().find(|(name, _, _)| name == topic)
                    {
                        let dist =
                            (self.position[0] - centre[0]).hypot(self.position[1] - centre[1]);
                        if dist > radius * 0.6 {
                            steer_toward(
                                &mut self.heading,
                                *centre,
                                [self.position[0], self.position[1]],
                                dt,
                            );
                        }
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
                // Sit still at ground level.
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
