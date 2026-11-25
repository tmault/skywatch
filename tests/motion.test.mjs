import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from '../web/vendor/three.module.js';
import {
  clampFrameDt, createMotionTracker, trackSnapshot, stepRoll, followPose, groundContactWeight,
} from '../web/motion.js';

const TICK = 0.05;
// Deterministic network delay: bursts, gaps and out-of-step arrivals, 0-90 ms.
const delay = (n) => [0.002, 0.041, 0.09, 0.043, 0.001, 0.012, 0.067, 0.018][n % 8];

/** Replay server ticks through the client's per-snapshot and per-frame steps. */
function fly({ fps, seconds, pose, onGround = () => false, jitter = true, maxBank = 0.4 }) {
  const motion = createMotionTracker();
  const [x0, z0, alt0] = pose(0).position;
  const entry = {
    state: 'cruising',
    group: { position: new THREE.Vector3(x0, alt0, z0) },
    target: new THREE.Vector3(x0, alt0, z0),
    lead: new THREE.Vector3(x0, alt0, z0),
    heading: pose(0).heading,
    leadHeading: pose(0).heading,
    targetHeading: pose(0).heading,
  };
  const frames = [];
  let next = 0;
  let arrival = 0;
  let roll = 0;
  for (let t = 0; t < seconds; t += 1 / fps) {
    while (arrival <= t) {
      const time = next * TICK;
      const a = pose(time);
      trackSnapshot(motion, a, time, onGround(time));
      entry.target.set(a.position[0], a.position[2], a.position[1]);
      entry.targetHeading = a.heading;
      next += 1;
      // WebSocket frames stay ordered, so a late one delays those behind it.
      arrival = Math.max(arrival, next * TICK + (jitter ? delay(next) : 0));
    }
    followPose(entry, 1 / fps);
    roll = stepRoll(roll, motion.turnRate, maxBank, 1 / fps);
    frames.push({
      t, roll, pitch: motion.flightPitch,
      position: entry.group.position.clone(), heading: entry.heading,
    });
  }
  return frames;
}

const steps = (values) => values.slice(1).map((v, i) => v - values[i]);
const spread = (values) => Math.max(...values) - Math.min(...values);

const orbit = (rate, speed = 12) => (t) => ({
  position: [
    (speed / rate) * Math.sin(rate * t),
    (speed / rate) * (1 - Math.cos(rate * t)),
    120,
  ],
  heading: Math.atan2(Math.sin(rate * t), Math.cos(rate * t)),
});

for (const fps of [60, 144]) {
  test(`a steady turn banks steadily at ${fps} fps despite jittery 20 Hz snapshots`, () => {
    const frames = fly({ fps, seconds: 60, pose: orbit(0.25) });
    const settled = frames.filter((f) => f.t > 3).map((f) => f.roll);
    assert.ok(Math.abs(settled.at(-1) - (0.25 - 0.02) * 1.15) < 0.002, `bank ${settled.at(-1)}`);
    // Includes heading wrapping through +/-PI several times.
    assert.ok(spread(settled) < 0.002, `roll rattles over ${spread(settled)} rad`);
    const entry = steps(frames.filter((f) => f.t < 3).map((f) => f.roll));
    assert.ok(entry.every((d) => d > -1e-4), 'roll-in reverses');
    assert.ok(entry.every((d) => d <= 0.8 / fps + 1e-9), 'roll rate is unbounded');
  });
}

test('heading noise and micro altitude corrections leave wings and nose level', () => {
  const frames = fly({
    fps: 60,
    seconds: 20,
    pose: (t) => {
      const sign = Math.round(t / TICK) % 2 ? 1 : -1;
      return { position: [12 * t, 0, 120 + sign * 0.01], heading: sign * 0.0004 };
    },
  });
  const settled = frames.filter((f) => f.t > 2);
  assert.ok(settled.every((f) => Math.abs(f.roll) < 0.002), 'wings chatter');
  assert.ok(settled.every((f) => Math.abs(f.pitch) < 0.002), 'nose chatters');
});

test('a steady climb holds one pitch attitude through bunched snapshots', () => {
  const frames = fly({
    fps: 60,
    seconds: 20,
    pose: (t) => ({ position: [14 * t, 0, 60 + 2 * t], heading: 0 }),
  });
  const settled = frames.filter((f) => f.t > 3).map((f) => f.pitch);
  assert.ok(Math.abs(settled.at(-1) + Math.atan2(2 - 0.3, 14)) < 0.002, `pitch ${settled.at(-1)}`);
  assert.ok(spread(settled) < 0.001, `pitch jitters over ${spread(settled)} rad`);
});

test('position glides without a 20 Hz speed ripple', () => {
  const speed = 30;
  const frames = fly({
    fps: 144,
    seconds: 5,
    jitter: false,
    pose: (t) => ({ position: [speed * t, 0, 120], heading: 0 }),
  });
  const rates = steps(frames.filter((f) => f.t > 2).map((f) => f.position.x)).map((d) => d * 144);
  // A single smoothing stage ripples by about half the ground speed here.
  assert.ok(spread(rates) < speed * 0.25, `speed ripples by ${spread(rates)}`);
});

test('an aircraft slowing to a stop settles without bouncing back', () => {
  const frames = fly({
    fps: 60,
    seconds: 8,
    onGround: () => true,
    pose: (t) => {
      const s = Math.min(t, 3);
      return { position: [6 * s - s * s, 0, 3], heading: 0 };
    },
  });
  assert.ok(steps(frames.map((f) => f.position.x)).every((d) => d >= -1e-6), 'position bounced');
  assert.ok(Math.abs(frames.at(-1).position.x - 9) < 0.01);
  assert.ok(frames.every((f) => f.pitch === 0));
});

test('repeats keep rates; restarts, long gaps and heading snaps never spike them', () => {
  const m = createMotionTracker();
  const at = (heading, x) => ({ position: [x, 0, 100], heading });
  for (let n = 0; n <= 40; n++) trackSnapshot(m, at(n * TICK * 0.2, n), n * TICK, false);
  const steady = m.turnRate;
  assert.ok(steady > 0.18);
  trackSnapshot(m, at(40 * TICK * 0.2, 40), 40 * TICK, false); // repeated frame
  assert.equal(m.turnRate, steady);
  trackSnapshot(m, at(-2.5, 41), 41 * TICK, false); // server snapped the heading
  assert.equal(m.turnRate, steady);
  trackSnapshot(m, at(1, 500), 600, false); // reconnect long after
  assert.equal(m.turnRate, 0);
  trackSnapshot(m, at(2, 0), 0, false); // server restart rewinds simulated time
  assert.equal(m.turnRate, 0);
  assert.equal(m.flightPitch, 0);
});

test('a stalled frame is capped and a map wrap snaps instead of sweeping', () => {
  assert.equal(clampFrameDt(7), 0.1);
  assert.equal(clampFrameDt(1 / 60), 1 / 60);
  const entry = {
    state: 'cruising',
    group: { position: new THREE.Vector3(409, 100, 0) },
    target: new THREE.Vector3(-409, 100, 0),
    lead: new THREE.Vector3(409, 100, 0),
    heading: 0, leadHeading: 0, targetHeading: 0,
  };
  assert.equal(followPose(entry, 1 / 60), true);
  assert.deepEqual(entry.group.position.toArray(), [-409, 100, 0]);
  assert.equal(followPose(entry, 1 / 60), false);
  assert.deepEqual(entry.group.position.toArray(), [-409, 100, 0]);
});

test('deck contact correction fades in on approach and stays on through taxi', () => {
  assert.equal(groundContactWeight('landing', 90), 0);
  assert.equal(groundContactWeight('landing', 0), groundContactWeight('landed', 0));
  assert.equal(groundContactWeight('taxi', 0), 1);
  assert.equal(groundContactWeight('cruising', 0), 0);
  let previous = 0;
  for (let alt = 30; alt >= 0; alt -= 0.1) {
    const weight = groundContactWeight('landing', alt);
    assert.ok(weight >= previous && weight - previous < 0.02, `hop at ${alt}`);
    previous = weight;
  }
});
