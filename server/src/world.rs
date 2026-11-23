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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aircraft {
    pub id: u64,
    pub callsign: String,
    pub kind: Kind,
    pub state: State,
    pub position: [f32; 3],
    pub heading: f32,
    pub altitude: f32,
    pub hold_reason: Option<String>,
    /// Ground speed in m/s (serde default keeps old snapshots deserialisable).
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_speed() -> f32 {
    AGENT_SPEED
}

/// Sim area is 1000x1000; positions wrap around at the edges.
const AREA_SIZE: f32 = 1000.0;
const TAKEOFF_RATE: f32 = 5.0; // m/s climb
const CRUISE_ALTITUDE: f32 = 120.0;
const AGENT_SPEED: f32 = 20.0; // m/s
const DRONE_SPEED: f32 = 6.0; // m/s, drones always slow loiter
const ORBIT_RADIUS: f32 = 40.0;
const HOLD_TURN_RATE: f32 = 0.4; // rad/s turn rate while holding
const HOLD_DRIFT_SPEED: f32 = ORBIT_RADIUS * 0.1; // m/s drift while holding

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Aircraft {
    /// New aircraft with a random-ish start position, cruising.
    pub fn new(callsign: &str, kind: Kind) -> Self {
        let mut rng = rand::thread_rng();
        let speed = match kind {
            Kind::Agent => AGENT_SPEED,
            Kind::Drone => DRONE_SPEED,
        };
        Aircraft {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            callsign: callsign.to_string(),
            kind,
            state: State::Cruising,
            position: [
                rng.gen_range(0.0..AREA_SIZE),
                rng.gen_range(0.0..AREA_SIZE),
                CRUISE_ALTITUDE,
            ],
            heading: rng.gen_range(0.0..std::f32::consts::TAU),
            altitude: CRUISE_ALTITUDE,
            hold_reason: None,
            speed,
        }
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
                self.position[0] = (self.position[0] + dx).rem_euclid(AREA_SIZE);
                self.position[1] = (self.position[1] + dy).rem_euclid(AREA_SIZE);
            }
            State::Cruising => {
                let dx = self.heading.cos() * self.speed * dt;
                let dy = self.heading.sin() * self.speed * dt;
                self.position[0] = (self.position[0] + dx).rem_euclid(AREA_SIZE);
                self.position[1] = (self.position[1] + dy).rem_euclid(AREA_SIZE);
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub aircraft: Vec<Aircraft>,
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
