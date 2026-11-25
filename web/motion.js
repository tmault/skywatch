// Aircraft motion smoothing between 20 Hz server snapshots and 60-144 Hz frames.
// Rates (turn, climb) are estimated once per snapshot from simulated time; frames
// only ease toward them, so nothing sawtooths at the snapshot rate.

const INTERP_RATE = 10;        // 1/s overall pose smoothing (mean lag 0.1 s)
const MAX_FRAME_DT = 0.1;      // s, longest frame step after a hitch or tab switch
const MIN_SAMPLE_DT = 0.025;   // s, closer snapshots merge into the next sample
const MAX_SAMPLE_DT = 1;       // s, longer gaps restart the estimates
const RATE_TAU = 0.3;          // s, low-pass time constant for snapshot rates
const MAX_TURN_RATE = 3;       // rad/s, faster heading changes are snaps, not turns
const TURN_DEADBAND = 0.02;    // rad/s
const CLIMB_DEADBAND = 0.3;    // units/s
const MIN_PITCH_SPEED = 1;     // units/s of ground speed before a flight path exists
const BANK_GAIN = 1.15;        // radians of bank per rad/s of turn
const ROLL_RATE = 5;           // 1/s easing toward the target bank
const MAX_ROLL_SPEED = 0.8;    // rad/s
const WRAP_DISTANCE = 200;     // units, a longer jump is a map wrap, not movement
const MAX_SNAPSHOT_RISE = 25;  // units, a bigger altitude jump is a reposition

export const SNAPSHOT_STALE_AFTER = 0.5; // s without frames before wings level

export const clampFrameDt = (dt) => Math.min(Math.max(dt, 0), MAX_FRAME_DT);

export const wrapAngle = (a) => Math.atan2(Math.sin(a), Math.cos(a));

const softDeadband = (v, band) => Math.sign(v) * Math.max(0, Math.abs(v) - band);

export function createMotionTracker() {
  return { sample: null, turnRate: 0, groundSpeed: 0, climbRate: 0, flightPitch: 0 };
}

function resetRates(m) {
  m.turnRate = 0;
  m.groundSpeed = 0;
  m.climbRate = 0;
  m.flightPitch = 0;
}

/**
 * Feed one server snapshot of an aircraft. `time` is the snapshot's simulated
 * time in seconds. `flightPitch` follows the three.js convention: negative
 * pitches the nose up.
 */
export function trackSnapshot(m, a, time, onGround) {
  const [x, z, alt] = a.position;
  const prev = m.sample;
  const dt = prev ? time - prev.time : NaN;
  // Repeated or bunched frames: keep the older sample so the next delta spans them.
  if (dt >= 0 && dt < MIN_SAMPLE_DT) {
    if (onGround) m.flightPitch = 0;
    return;
  }
  m.sample = { time, x, z, alt, heading: a.heading };
  // First sight, server restart or a long gap.
  if (!(dt > 0 && dt <= MAX_SAMPLE_DT)) {
    resetRates(m);
    return;
  }
  const alpha = 1 - Math.exp(-dt / RATE_TAU);
  const turn = wrapAngle(a.heading - prev.heading) / dt;
  if (Math.abs(turn) <= MAX_TURN_RATE) m.turnRate += (turn - m.turnRate) * alpha;

  const distance = Math.hypot(x - prev.x, z - prev.z);
  const rise = alt - prev.alt;
  if (onGround) {
    m.groundSpeed = distance < WRAP_DISTANCE ? distance / dt : 0;
    m.climbRate = 0;
    m.flightPitch = 0;
    return;
  }
  if (distance >= WRAP_DISTANCE || Math.abs(rise) >= MAX_SNAPSHOT_RISE) return;
  // Filter the two velocities, not their angle: a tiny step makes a wild angle.
  m.groundSpeed += (distance / dt - m.groundSpeed) * alpha;
  m.climbRate += (rise / dt - m.climbRate) * alpha;
  m.flightPitch = m.groundSpeed > MIN_PITCH_SPEED
    ? -Math.atan2(softDeadband(m.climbRate, CLIMB_DEADBAND), m.groundSpeed)
    : 0;
}

/** Next bank angle: ease toward the angle for `turnRate`, at a bounded roll speed. */
export function stepRoll(roll, turnRate, maxBank, dt) {
  const bank = softDeadband(turnRate, TURN_DEADBAND) * BANK_GAIN;
  const target = Math.max(-maxBank, Math.min(maxBank, bank));
  const step = (target - roll) * (1 - Math.exp(-ROLL_RATE * dt));
  return roll + Math.max(-MAX_ROLL_SPEED * dt, Math.min(MAX_ROLL_SPEED * dt, step));
}

/**
 * Move `entry` toward its latest snapshot pose through two cascaded low-pass
 * stages. One stage lurches each time the 20 Hz target steps; two stay smooth at
 * the same lag and never overshoot, so aircraft don't bounce as they slow down.
 * Returns true when the aircraft wrapped across the map and was snapped.
 */
export function followPose(entry, dt) {
  const alpha = 1 - Math.exp(-2 * INTERP_RATE * dt);
  const position = entry.group.position;
  const wrapped = entry.state !== 'departing'
    && position.distanceToSquared(entry.target) > WRAP_DISTANCE ** 2;
  if (wrapped) {
    position.copy(entry.target);
    entry.lead.copy(entry.target);
  } else {
    entry.lead.lerp(entry.target, alpha);
    position.lerp(entry.lead, alpha);
  }
  entry.leadHeading += wrapAngle(entry.targetHeading - entry.leadHeading) * alpha;
  entry.heading += wrapAngle(entry.leadHeading - entry.heading) * alpha;
  return wrapped;
}

/**
 * How much of the wheel-contact height correction applies: all of it on the deck,
 * fading in over the last part of the approach so it never appears as a hop.
 */
export function groundContactWeight(state, altitude) {
  if (state === 'landed' || state === 'taxi') return 1;
  if (state !== 'landing') return 0;
  const t = Math.max(0, Math.min(1, (24 - altitude) / 18));
  return t * t * (3 - 2 * t);
}
