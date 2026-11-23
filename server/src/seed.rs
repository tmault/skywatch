//! Demo seeding for the Skywatch server.
//!
//! When demo mode is enabled, `seed` populates the world with a fixed cast of
//! five aircraft (three agents, two drones) so the demo starts with something
//! interesting on screen, and `run_script` drives a simple scripted state
//! machine so the world evolves over time without any adapters connected.
//!
//! Demo mode is controlled by the `SKYWATCH_DEMO` env var: set it to `0` to
//! disable seeding. It defaults to **on** for now — real adapters (a later
//! task) will take over seeding/behaviour, at which point the default flips.
//!
//! Scripted behaviour (one step applied every `SCRIPT_INTERVAL`, ~60s):
//! - `SKY1`: occasionally enters holding ("requesting vector"), released the
//!   next cycle.
//! - `SKY3`: stays landed for 2 cycles, then takes off → cruises, and later
//!   lands again. The sequence loops.
//! - `REAP1`/`REAP2` drones: just loiter; the script emits nothing for them
//!   (no wire chatter — chatter is client-side from state changes).

use std::time::Duration;

use crate::world::{Aircraft, Kind, State, World};

/// Positions spread across the 1000x1000 map so aircraft don't overlap.
const POSITIONS: [[f32; 3]; 5] = [
    [200.0, 300.0, 120.0],
    [500.0, 150.0, 120.0],
    [700.0, 600.0, 0.0],
    [350.0, 750.0, 40.0],
    [850.0, 350.0, 40.0],
];

const HEADINGS: [f32; 5] = [0.8, 2.4, 0.0, 3.9, 5.5];
const ALTITUDES: [f32; 5] = [120.0, 120.0, 0.0, 40.0, 40.0];

/// Populate a fresh world with the demo cast.
pub fn seed(world: &mut World) {
    let callsigns = [
        ("SKY1", Kind::Agent),
        ("SKY2", Kind::Agent),
        ("SKY3", Kind::Agent),
        ("REAP1", Kind::Drone),
        ("REAP2", Kind::Drone),
    ];

    for (i, (callsign, kind)) in callsigns.iter().enumerate() {
        let mut a = Aircraft::new(callsign, *kind);
        a.position = POSITIONS[i];
        a.heading = HEADINGS[i];
        a.altitude = ALTITUDES[i];
        match *callsign {
            "SKY2" => a.hold("awaiting approval of refactor plan"),
            "SKY3" => a.land(),
            _ => {}
        }
        world.add(a);
    }
}

/// One scripted action: apply it to the world at a given cycle.
#[derive(Clone, Copy, Debug)]
enum Action {
    /// Aircraft enters holding with the given reason.
    Hold(&'static str),
    /// Release a holding aircraft (→ takeoff → cruising).
    Release,
    /// Touch down.
    Land,
}

/// A scheduled (callsign, action, cycle) step. Cycles are modulo the script
/// length, so the sequence repeats forever.
struct Step {
    callsign: &'static str,
    action: Action,
    cycle: u32,
}

/// Script length (cycles) before it repeats.
const SCRIPT_LEN: u32 = 6;

const SCRIPT: [Step; 6] = [
    // SKY1: hold one cycle, then released.
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
    // SKY3: 2 landed cycles, then takeoff; cruises for a while, then lands.
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
    // A second SKY1 hold cycle keeps the sky busy near the end of the loop.
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

/// Drones keep loitering on their own; no scripted steps for them.

fn apply_step(world: &mut World, step: &Step) {
    let Some(a) = world.get_mut(step.callsign) else {
        tracing::warn!(callsign = step.callsign, "scripted aircraft missing");
        return;
    };
    match step.action {
        Action::Hold(reason) => {
            // Only hold if not already holding/landed, to avoid stomping state.
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

/// Apply every scripted step scheduled for `cycle` (cycles wrap mod SCRIPT_LEN).
fn run_cycle(world: &mut World, cycle: u32) {
    let cycle = cycle % SCRIPT_LEN;
    for step in SCRIPT.iter().filter(|s| s.cycle == cycle) {
        apply_step(world, step);
    }
}

/// Interval between scripted cycles.
pub const SCRIPT_INTERVAL: Duration = Duration::from_secs(60);

/// Run the scripted state machine forever, applying one cycle per interval.
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
    fn seed_creates_expected_cast() {
        let mut world = World::new();
        seed(&mut world);
        let mut seen: Vec<(String, Kind)> =
            world.iter().map(|a| (a.callsign.clone(), a.kind)).collect();
        seen.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            seen,
            vec![
                ("REAP1".to_string(), Kind::Drone),
                ("REAP2".to_string(), Kind::Drone),
                ("SKY1".to_string(), Kind::Agent),
                ("SKY2".to_string(), Kind::Agent),
                ("SKY3".to_string(), Kind::Agent),
            ]
        );
    }

    #[test]
    fn seed_initial_states() {
        let mut world = World::new();
        seed(&mut world);
        let state_of = |cs: &str| world.iter().find(|a| a.callsign == cs).unwrap().state;
        assert_eq!(state_of("SKY1"), State::Cruising);
        assert_eq!(state_of("SKY2"), State::Holding);
        assert_eq!(state_of("SKY3"), State::Landed);
        assert_eq!(state_of("REAP1"), State::Cruising);
        assert_eq!(state_of("REAP2"), State::Cruising);
        let sky2 = world.iter().find(|a| a.callsign == "SKY2").unwrap();
        assert_eq!(
            sky2.hold_reason.as_deref(),
            Some("awaiting approval of refactor plan")
        );
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
        assert_eq!(a.state, State::Takeoff);
        // Climb to cruise altitude (~24s of sim at 5 m/s), still cruising later.
        world.tick(30.0);
        run_cycle(&mut world, 4);
        let a = world.iter().find(|a| a.callsign == "SKY3").unwrap();
        assert_eq!(a.state, State::Cruising);
        // ...then lands again.
        run_cycle(&mut world, 5);
        let a = world.iter().find(|a| a.callsign == "SKY3").unwrap();
        assert_eq!(a.state, State::Landed);
    }
}
