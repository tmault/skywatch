// When retractable gear should be down, and how fast it travels. Progress is
// 1 down-and-locked, 0 stowed; the airframe poses itself through
// `gear.userData.setProgress` (see aircraft.js).

export const GEAR_CYCLE_SECONDS = 2.2;

/** Commanded gear position: stowed en route, climbing away and high on approach. */
export function gearTarget(state, altitude) {
  const stowed = state === 'cruising'
    || state === 'holding'
    || state === 'refueling'
    || ((state === 'departing' || state === 'takeoff') && altitude > 18)
    || (state === 'landing' && altitude > 70);
  return stowed ? 0 : 1;
}

/** Linear travel toward `target`; ease the result before posing the airframe. */
export function advanceGear(progress, target, dt) {
  const step = Math.max(0, dt) / GEAR_CYCLE_SECONDS;
  return progress < target
    ? Math.min(target, progress + step)
    : Math.max(target, progress - step);
}

export function easeGear(progress) {
  const p = Math.min(1, Math.max(0, progress));
  return p * p * (3 - 2 * p);
}
