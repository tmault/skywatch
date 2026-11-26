// Fuel apron: threads that hit a provider usage limit park on a fuel stand
// south of the holding bay, with a bowser alongside, until the limit resets.
// `refuelStatus` turns an aircraft's reset time into the countdown shown on
// its label and in the radio panel.

import * as THREE from './vendor/three.module.js';

export const FUEL_ACCENT = 0xb8f03a; // fuelling: lime
export const FUELLED_ACCENT = 0x4dff9e; // limit reset: green

function pad2(n) {
  return String(n).padStart(2, '0');
}

/** Format a remaining duration in ms as `1h 05m` or `14:07`. */
export function formatCountdown(ms) {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0 ? `${h}h ${pad2(m)}m` : `${pad2(m)}:${pad2(s)}`;
}

/**
 * Countdown state for a refuelling aircraft at `nowMs`:
 * `{ clock, fuelled, progress, short, long }`. `clock` is null when T3 did
 * not record a reset time; `progress` runs 0..1 from limit hit to reset.
 */
export function refuelStatus(a, nowMs = Date.now()) {
  const auto = Boolean(a.auto_resume);
  const resets = a.refuel_resets_at ?? null;
  if (resets === null) {
    return {
      clock: null,
      fuelled: false,
      progress: 0,
      short: auto ? 'FUEL · AUTO T/O AT RESET' : 'FUEL · RESET TIME UNKNOWN',
      long: auto
        ? 'Usage limit reached. T3 will resume this thread when the limit resets.'
        : 'Usage limit reached. Reset time unknown; send a message to resume.',
    };
  }

  const left = resets - nowMs;
  const started = a.refuel_started_at ?? resets - 5 * 3600 * 1000;
  const span = Math.max(1, resets - started);
  const progress = Math.min(1, Math.max(0, (nowMs - started) / span));
  const at = new Date(resets).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

  if (left > 0) {
    const clock = formatCountdown(left);
    return {
      clock,
      fuelled: false,
      progress,
      short: auto ? `FUEL ${clock} · AUTO T/O` : `FUEL ${clock} · MANUAL`,
      long: auto
        ? `Refuelling. Usage limit resets at ${at}; auto-resume takes off in ${clock}.`
        : `Refuelling. Usage limit resets at ${at} (${clock}); auto-resume is off, so send a message to take off after that.`,
    };
  }
  return {
    clock: '00:00',
    fuelled: true,
    progress: 1,
    short: auto ? 'FUELLED · TAKING OFF' : 'FUELLED · AWAITING PILOT',
    long: auto
      ? `Limit reset at ${at}. Auto-resume is starting the thread.`
      : `Limit reset at ${at}. Fuelled and ready; send a message to take off.`,
  };
}

/** A fuel bowser: cab plus a white tank with a red band and an amber beacon. */
function makeBowser() {
  const g = new THREE.Group();
  const cabMat = new THREE.MeshStandardMaterial({ color: 0xd93829, roughness: 0.55 });
  const tankMat = new THREE.MeshStandardMaterial({ color: 0xeef1f3, roughness: 0.35, metalness: 0.3 });
  const bandMat = new THREE.MeshStandardMaterial({ color: 0xd93829, roughness: 0.5 });
  const darkMat = new THREE.MeshStandardMaterial({ color: 0x1b1f24, roughness: 0.9 });

  const chassis = new THREE.Mesh(new THREE.BoxGeometry(2.4, 0.5, 7.4), darkMat);
  chassis.position.y = 0.75;
  g.add(chassis);
  const cab = new THREE.Mesh(new THREE.BoxGeometry(2.4, 1.9, 1.8), cabMat);
  cab.position.set(0, 1.9, -2.8);
  g.add(cab);
  const tank = new THREE.Mesh(new THREE.CylinderGeometry(1.15, 1.15, 5.0, 14), tankMat);
  tank.rotation.x = Math.PI / 2;
  tank.position.set(0, 2.15, 0.9);
  g.add(tank);
  const band = new THREE.Mesh(new THREE.CylinderGeometry(1.18, 1.18, 0.6, 14), bandMat);
  band.rotation.x = Math.PI / 2;
  band.position.set(0, 2.15, 0.9);
  g.add(band);
  for (const wz of [-2.6, 0.2, 2.6]) {
    for (const wx of [-1.2, 1.2]) {
      const wheel = new THREE.Mesh(new THREE.CylinderGeometry(0.5, 0.5, 0.4, 10), darkMat);
      wheel.rotation.z = Math.PI / 2;
      wheel.position.set(wx, 0.5, wz);
      g.add(wheel);
    }
  }
  const beacon = new THREE.Mesh(
    new THREE.SphereGeometry(0.32, 8, 8),
    new THREE.MeshBasicMaterial({ color: 0xffb020, transparent: true }),
  );
  beacon.position.set(0, 3.05, -2.8);
  g.add(beacon);
  return { group: g, beacon };
}

/** A small white storage tank for the fuel farm. */
function makeFuelTank(r, h) {
  const g = new THREE.Group();
  const shell = new THREE.Mesh(
    new THREE.CylinderGeometry(r, r, h, 20),
    new THREE.MeshStandardMaterial({ color: 0xe4e8ea, roughness: 0.4, metalness: 0.25 }),
  );
  shell.position.y = h / 2;
  g.add(shell);
  const band = new THREE.Mesh(
    new THREE.CylinderGeometry(r * 1.01, r * 1.01, h * 0.12, 20),
    new THREE.MeshStandardMaterial({ color: 0xd93829, roughness: 0.5 }),
  );
  band.position.y = h * 0.72;
  g.add(band);
  const roof = new THREE.Mesh(
    new THREE.ConeGeometry(r, r * 0.35, 20),
    new THREE.MeshStandardMaterial({ color: 0xc9cfd3, roughness: 0.5 }),
  );
  roof.position.y = h + r * 0.17;
  g.add(roof);
  return g;
}

/**
 * Build the fuel apron on the home island. `stands` are world `[x, z]` stand
 * centres; `deck` supplies the island's slab/marking helpers so the apron
 * matches the rest of the airfield. Returns `{ banner, update(now, occupied) }`.
 */
export function addFuelApron(island, stands, deck) {
  const { y, markY, makeDeckSlab, makeMarking, taxiMat, apronMat, yellowLineMat } = deck;
  const xs = stands.map((s) => s[0]);
  const zs = stands.map((s) => s[1]);
  const minX = Math.min(...xs) - 11;
  const maxX = Math.max(...xs) + 11;
  const minZ = Math.min(...zs) - 12;
  const maxZ = Math.max(...zs) + 20; // room for the fuel farm at the back
  const cx = (minX + maxX) / 2;
  const cz = (minZ + maxZ) / 2;

  const apron = makeDeckSlab(maxX - minX, maxZ - minZ, y - 0.03, 0.4, apronMat);
  apron.position.set(cx, apron.position.y, cz);
  island.add(apron);

  // Lead-in from the south end of Taxiway Alpha (x=84).
  const link = makeDeckSlab(14, minZ - 56, y - 0.01, 0.4, taxiMat);
  link.position.set(84, link.position.y, (56 + minZ) / 2);
  island.add(link);
  island.add(makeMarking(0.85, minZ - 56 + 4, 84, (56 + minZ) / 2, markY, yellowLineMat));

  // Lime stand boxes: lead-in line, stop bar, and a stand outline per stand.
  const limeMat = new THREE.MeshBasicMaterial({
    color: FUEL_ACCENT, polygonOffset: true, polygonOffsetFactor: -2,
  });
  const rows = [...new Set(zs)];
  for (const rz of rows) {
    island.add(makeMarking(maxX - minX - 4, 0.7, cx, rz - 9, markY, yellowLineMat));
  }
  const bowsers = [];
  for (const [sx, sz] of stands) {
    // Planes park nose-in facing south (+Z): lead-in from the north, stop bar at the nose.
    island.add(makeMarking(0.7, 14, sx, sz - 2, markY, yellowLineMat));
    island.add(makeMarking(6, 0.9, sx, sz + 7.5, markY, limeMat));
    for (const side of [-7, 7]) island.add(makeMarking(0.45, 16, sx + side, sz, markY, limeMat));

    const bowser = makeBowser();
    bowser.group.position.set(sx + 5.2, y, sz + 9.5);
    bowser.group.rotation.y = Math.PI - 0.35;
    island.add(bowser.group);
    bowsers.push({ stand: [sx, sz], ...bowser });
  }

  // Fuel farm along the back edge.
  for (let i = 0; i < 3; i++) {
    const tank = makeFuelTank(3.6, 6.5);
    tank.position.set(cx - 16 + i * 16, y, maxZ - 7);
    island.add(tank);
  }

  return {
    center: [cx, cz],
    backZ: maxZ,
    /** Blink bowser beacons on occupied stands; `occupied` is a list of `[x, z]`. */
    update(now, occupied) {
      for (const b of bowsers) {
        const busy = occupied.some(([ox, oz]) => Math.hypot(ox - b.stand[0], oz - b.stand[1]) < 4);
        b.beacon.material.opacity = busy ? (Math.sin(now * 7) > 0 ? 1 : 0.15) : 0.25;
      }
    },
  };
}
