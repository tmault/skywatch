use super::{
    provider_info, refuel_formation_position, wrap_centered, Aircraft, Kind, State, World,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::f32::consts::{PI, TAU};

use super::TANKER_SPEED;

struct Receiver {
    id: u64,
    tanker: Option<u64>,
    slot: Option<usize>,
}

impl World {
    /// Retain occupied tracks and rendezvous slots as other aircraft come and go.
    pub fn sync_tankers(&mut self) {
        let mut groups = BTreeMap::<_, (&str, Vec<Receiver>)>::new();
        for a in &mut self.aircraft {
            if a.kind == Kind::Tanker {
                continue;
            }
            if a.state == State::Refueling && !a.data_stale {
                let provider = a
                    .provider
                    .get_or_insert_with(|| provider_info(a.model.as_deref(), None).0.to_string());
                // Demo aircraft have no live limit scope and share by provider.
                let scope = a.refuel_scope.as_ref().unwrap_or(provider).clone();
                groups
                    .entry(scope)
                    .or_insert_with(|| (super::provider_descriptor(provider).0, Vec::new()))
                    .1
                    .push(Receiver {
                        id: a.id,
                        tanker: a.tanker_id,
                        slot: a.refuel_slot,
                    });
            } else {
                a.tanker_id = None;
                a.refuel_slot = None;
                a.refuel_connected = false;
                if a.state != State::Refueling {
                    a.refuel_reset_at = None;
                }
            }
        }

        self.aircraft.retain(|a| {
            a.kind != Kind::Tanker
                || a.refuel_scope
                    .as_ref()
                    .is_some_and(|scope| groups.contains_key(scope))
        });
        let mut used_tracks: HashSet<_> = self
            .aircraft
            .iter()
            .filter(|a| a.kind == Kind::Tanker)
            .map(|a| a.tanker_orbit_idx)
            .collect();
        let mut assignments = HashMap::new();
        for (scope, (provider, receivers)) in groups {
            let reason = format!("{} waiting refuelling aircraft", receivers.len());
            let tanker_id = if let Some(tanker) = self
                .aircraft
                .iter_mut()
                .find(|a| a.kind == Kind::Tanker && a.refuel_scope.as_deref() == Some(&scope))
            {
                tanker.hold_reason = Some(reason);
                tanker.id
            } else {
                let track = (0..)
                    .find(|track| used_tracks.insert(*track))
                    .expect("free track");
                let mut tanker = Aircraft::tanker(provider, &scope, track);
                if self.aircraft.iter().any(|a| a.callsign == tanker.callsign) {
                    tanker.callsign = format!("{}-{}", tanker.callsign, track + 1);
                }
                tanker.hold_reason = Some(reason);
                let id = tanker.id;
                self.aircraft.push(tanker);
                id
            };

            let mut occupied = HashSet::new();
            for r in &receivers {
                if let Some(slot) = r
                    .slot
                    .filter(|slot| r.tanker == Some(tanker_id) && occupied.insert(*slot))
                {
                    assignments.insert(r.id, (tanker_id, slot));
                }
            }
            for r in receivers {
                assignments.entry(r.id).or_insert_with(|| {
                    let slot = (0..)
                        .find(|slot| occupied.insert(*slot))
                        .expect("free slot");
                    (tanker_id, slot)
                });
            }
        }

        for a in &mut self.aircraft {
            if let Some(&(id, slot)) = assignments.get(&a.id) {
                if a.tanker_id != Some(id) || a.refuel_slot != Some(slot) {
                    a.refuel_connected = false;
                }
                a.tanker_id = Some(id);
                a.refuel_slot = Some(slot);
            }
        }
    }

    pub(super) fn tick_refuel_receivers(&mut self, dt: f32) {
        let tankers: HashMap<_, _> = self
            .aircraft
            .iter()
            .filter(|a| a.kind == Kind::Tanker)
            .map(|a| (a.id, (a.position, a.heading)))
            .collect();

        for a in &mut self.aircraft {
            let Some((&(position, heading), slot)) = a
                .tanker_id
                .and_then(|id| tankers.get(&id))
                .zip(a.refuel_slot)
            else {
                continue;
            };
            let target = refuel_formation_position(position, heading, slot);
            let dx = wrap_centered(target[0] - a.position[0]);
            let dz = wrap_centered(target[1] - a.position[1]);
            let distance = dx.hypot(dz);
            let altitude_error = target[2] - a.altitude;
            let heading_error = (heading - a.heading + PI).rem_euclid(TAU) - PI;

            if distance > 22.0 {
                // Intercept from below until clear of the tanker's flight path.
                let intercept_speed = (a.cruise_speed() * 2.2).max(26.0);
                a.speed += (intercept_speed - a.speed).clamp(-8.0 * dt, 8.0 * dt);
                a.turn_toward(dz.atan2(dx), (0.85 + 24.0 / distance) * dt);
                a.advance(dt);
            } else {
                let blend = (5.0 * dt).min(1.0).min(26.0 * dt / distance.max(0.01));
                a.position[0] = wrap_centered(a.position[0] + dx * blend);
                a.position[1] = wrap_centered(a.position[1] + dz * blend);
                a.turn_toward(heading, 1.2 * dt);
                a.speed += (TANKER_SPEED - a.speed).clamp(-8.0 * dt, 8.0 * dt);
            }
            let join_altitude = if distance > 60.0 {
                altitude_error - 20.0
            } else {
                altitude_error
            };
            a.altitude += join_altitude.clamp(-18.0 * dt, 18.0 * dt);
            a.position[2] = a.altitude;
            a.refuel_connected = distance < 6.0
                && altitude_error.abs() < 4.0
                && heading_error.abs() < 0.35
                && (a.speed - TANKER_SPEED).abs() < 3.0;
        }
    }
}
