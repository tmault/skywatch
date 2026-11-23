//! Demo seeding for the Skywatch server.
//!
//! When demo mode is enabled, `seed` populates the world with a cast of
//! aircraft (agents, fighter wingmen, drones, and gate-parked threads) across
//! sample device airports, and `run_script` drives a scripted state machine so
//! the world evolves over time without any adapters connected.

use std::time::Duration;

use crate::world::{
    default_sectors, formation_position, Aircraft, Airport, Kind, ModelClass, Sector, State, World,
    GATE_HEADING, HOME_GATES, PROJECT_SECTOR_RADIUS, PROJECT_SECTOR_SLOTS, RUNWAY_MID,
    SATELLITE_SLOTS,
};

/// Starting `[x, z, alt]` positions for the 5 primary demo aircraft.
const POSITIONS: [[f32; 3]; 5] = [
    [-225.0, 245.0, 100.0],
    [285.0, 30.0, 100.0],
    [RUNWAY_MID[0], RUNWAY_MID[1], 0.0],
    [0.0, -275.0, 65.0],
    [60.0, -255.0, 65.0],
];

const HEADINGS: [f32; 5] = [0.8, 2.4, 0.0, 3.9, 5.5];
const ALTITUDES: [f32; 5] = [100.0, 100.0, 0.0, 65.0, 65.0];

/// Populate a fresh world with the demo cast and sample device airports.
pub fn seed(world: &mut World) {
    world.airports = vec![
        Airport::home("local", "omarchy"),
        Airport::satellite("m5-air", "Tom's M5 Air", SATELLITE_SLOTS[0], true),
        Airport::satellite("m1", "m1", SATELLITE_SLOTS[1], true),
        Airport::satellite("devbox", "devbox", SATELLITE_SLOTS[2], false),
    ];

    let mut sectors = default_sectors();
    sectors.push(Sector {
        name: "Skywatch".to_string(),
        center: PROJECT_SECTOR_SLOTS[0],
        radius: PROJECT_SECTOR_RADIUS,
        kind: "project".to_string(),
    });
    world.sectors = sectors;

    let callsigns = [
        ("SKY1", Kind::Agent, ModelClass::Frontier, "Skywatch"),
        ("SKY2", Kind::Agent, ModelClass::Local, "AI & Models"),
        ("SKY3", Kind::Agent, ModelClass::Local, "Build"),
        ("REAP1", Kind::Drone, ModelClass::Local, "Homelab"),
        ("REAP2", Kind::Drone, ModelClass::Local, "Homelab"),
    ];

    let mut sky1_info = None;

    for (i, (callsign, kind, model_class, sector)) in callsigns.iter().enumerate() {
        let mut a = Aircraft::with_model(callsign, *kind, *model_class);
        a.position = POSITIONS[i];
        a.heading = HEADINGS[i];
        a.altitude = ALTITUDES[i];
        a.device = Some("omarchy".to_string());
        a.assign(sector);
        match *callsign {
            "SKY1" => {
                a.title = Some("Deploy pipeline orchestrator".to_string());
                sky1_info = Some((a.id, a.position, a.heading));
            }
            "SKY2" => {
                a.title = Some("Refactor auth middleware".to_string());
                a.hold("awaiting approval of refactor plan");
            }
            "SKY3" => {
                a.title = Some("Fix flaky integration test".to_string());
                a.land();
            }
            "REAP1" => a.title = Some("Tower health monitor".to_string()),
            "REAP2" => a.title = Some("Nightly backup cron".to_string()),
            _ => {}
        }
        world.add(a);
    }

    // Spawn two sub-agent fighter jets flying wingman formation on SKY1!
    if let Some((pid, pos, heading)) = sky1_info {
        for (slot, suffix, role) in [
            (0usize, "A", "Codebase researcher sub-agent"),
            (1usize, "B", "Test verifier sub-agent"),
        ] {
            let start = formation_position(pos, heading, false, slot);
            let mut wing = Aircraft::wingman(&format!("SKY1-{suffix}"), pid, slot, start, heading);
            wing.title = Some(role.to_string());
            wing.device = Some("omarchy".to_string());
            world.add(wing);
        }
    }

    // Seed two idle (>24h) threads parked neatly at the home airport gates.
    for (i, (cs, title, mc)) in [
        ("SKY4", "Weekend blog draft", ModelClass::Local),
        ("SKY5", "Benchmark notes", ModelClass::Frontier),
    ]
    .iter()
    .enumerate()
    {
        let gate = HOME_GATES[i];
        let mut a = Aircraft::with_model(cs, Kind::Agent, *mc);
        a.state = State::Parked;
        a.position = [gate[0], gate[1], 0.0];
        a.altitude = 0.0;
        a.heading = GATE_HEADING;
        a.title = Some((*title).to_string());
        a.device = Some("omarchy".to_string());
        world.add(a);
    }
}

/// One scripted action: apply it to the world at a given cycle.
#[derive(Clone, Copy, Debug)]
enum Action {
    Hold(&'static str),
    Release,
    Land,
}

struct Step {
    callsign: &'static str,
    action: Action,
    cycle: u32,
}

const SCRIPT_LEN: u32 = 6;

const SCRIPT: [Step; 6] = [
    Step {
        callsign: "SKY1",
        action: Action::Hold("requesting vector"),
        cycle: 1,
    },
    Step {
        callsign: "SKY1",
        action: Action::Release,
        cycle: 2,
    },
    Step {
        callsign: "SKY3",
        action: Action::Release,
        cycle: 3,
    },
    Step {
        callsign: "SKY3",
        action: Action::Land,
        cycle: 5,
    },
    Step {
        callsign: "SKY1",
        action: Action::Hold("requesting vector"),
        cycle: 5,
    },
    Step {
        callsign: "SKY1",
        action: Action::Release,
        cycle: 0,
    },
];

fn apply_step(world: &mut World, step: &Step) {
    let Some(a) = world.get_mut(step.callsign) else {
        tracing::warn!(callsign = step.callsign, "scripted aircraft missing");
        return;
    };
    match step.action {
        Action::Hold(reason) => {
            if a.state == State::Cruising {
                tracing::info!(callsign = step.callsign, reason, "script: holding");
                a.hold(reason);
            }
        }
        Action::Release => {
            if a.state == State::Holding {
                tracing::info!(callsign = step.callsign, "script: released");
                a.release();
            } else if step.callsign == "SKY3" && a.state == State::Landed {
                tracing::info!(callsign = step.callsign, "script: cleared for takeoff");
                a.release();
            }
        }
        Action::Land => {
            if a.state == State::Cruising || a.state == State::Takeoff {
                tracing::info!(callsign = step.callsign, "script: landing");
                a.land();
            }
        }
    }
}

fn run_cycle(world: &mut World, cycle: u32) {
    let cycle = cycle % SCRIPT_LEN;
    for step in SCRIPT.iter().filter(|s| s.cycle == cycle) {
        apply_step(world, step);
    }
}

pub const SCRIPT_INTERVAL: Duration = Duration::from_secs(60);

pub async fn run_script(world: crate::ws::SharedWorld) {
    let mut cycle: u32 = 0;
    let mut interval = tokio::time::interval(SCRIPT_INTERVAL);
    loop {
        interval.tick().await;
        let mut w = world.lock().await;
        run_cycle(&mut w, cycle);
        cycle = cycle.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_creates_expected_cast_and_wingmen() {
        let mut world = World::new();
        seed(&mut world);
        assert!(world.airports.len() >= 3);
        let wingmen: Vec<_> = world.iter().filter(|a| a.kind == Kind::Fighter).collect();
        assert_eq!(wingmen.len(), 2);
        let sky1 = world.iter().find(|a| a.callsign == "SKY1").unwrap();
        assert!(wingmen.iter().all(|w| w.parent_id == Some(sky1.id)));
    }

    #[test]
    fn seed_initial_states() {
        let mut world = World::new();
        seed(&mut world);
        let state_of = |cs: &str| world.iter().find(|a| a.callsign == cs).unwrap().state;
        assert_eq!(state_of("SKY1"), State::Cruising);
        assert_eq!(state_of("SKY2"), State::Holding);
        assert_eq!(state_of("SKY3"), State::Landed);
        assert_eq!(state_of("SKY4"), State::Parked);
        assert_eq!(state_of("REAP1"), State::Cruising);
        assert_eq!(state_of("REAP2"), State::Cruising);
    }

    #[test]
    fn sky1_hold_then_release_cycle() {
        let mut world = World::new();
        seed(&mut world);
        run_cycle(&mut world, 1);
        let a = world.iter().find(|a| a.callsign == "SKY1").unwrap();
        assert_eq!(a.state, State::Holding);
        run_cycle(&mut world, 2);
        let a = world.iter().find(|a| a.callsign == "SKY1").unwrap();
        assert_eq!(a.state, State::Takeoff);
    }

    #[test]
    fn sky3_takeoff_then_land() {
        let mut world = World::new();
        seed(&mut world);
        run_cycle(&mut world, 3);
        let a = world.iter().find(|a| a.callsign == "SKY3").unwrap();
        assert_eq!(a.state, State::Taxi);
        world.tick(12.0);
        let a = world.iter().find(|a| a.callsign == "SKY3").unwrap();
        assert_eq!(a.state, State::Ready);
        world.tick(5.0);
        world.tick(30.0);
        let a = world.iter().find(|a| a.callsign == "SKY3").unwrap();
        assert_eq!(a.state, State::Cruising);
        run_cycle(&mut world, 5);
        let a = world.iter().find(|a| a.callsign == "SKY3").unwrap();
        assert_eq!(a.state, State::Landed);
        world.tick(6.0);
        let a = world.iter().find(|a| a.callsign == "SKY3").unwrap();
        assert_eq!(a.state, State::Taxi);
        world.tick(40.0);
        let a = world.iter().find(|a| a.callsign == "SKY3").unwrap();
        assert_eq!(a.state, State::Parked);
    }
}
