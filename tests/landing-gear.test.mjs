import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from '../web/vendor/three.module.js';
import {
  APPROACH_EXTEND_ALT,
  APPROACH_RETRACT_ALT,
  GEAR_EXTEND_DURATION,
  GEAR_RETRACT_DURATION,
  LIFTOFF_RETRACT_ALT,
  applyLandingGearPose,
  buildJetLandingGear,
  buildTankerLandingGear,
  computeGearKinematics,
  gearTargetFor,
  initLandingGear,
  stepGearPhase,
  updateLandingGear,
} from '../web/landing-gear.js';

function dummyKit() {
  const m = () => new THREE.MeshBasicMaterial();
  return { metal: m(), rubber: m(), dark: m(), wing: m(), belly: m(), plain: m() };
}

test('gearTargetFor keeps gear down on ground and up at cruise/hold/refuel', () => {
  for (const state of ['parked', 'ready', 'taxi', 'landed']) {
    assert.equal(gearTargetFor(state, 0), 1, `${state} on ground`);
  }
  for (const state of ['cruising', 'holding', 'refueling']) {
    assert.equal(gearTargetFor(state, 160), 0, `${state} at altitude`);
    assert.equal(gearTargetFor(state, 40), 0, `${state} low altitude`);
  }
});

test('gearTargetFor handles takeoff/departing liftoff and landing approach with hysteresis', () => {
  for (const state of ['takeoff', 'departing']) {
    assert.equal(gearTargetFor(state, 0), 1);
    assert.equal(gearTargetFor(state, LIFTOFF_RETRACT_ALT), 1);
    assert.equal(gearTargetFor(state, LIFTOFF_RETRACT_ALT + 1), 0);
  }

  // Descending into approach.
  assert.equal(gearTargetFor('landing', 150, 0), 0);
  assert.equal(gearTargetFor('landing', APPROACH_EXTEND_ALT + 5, 0), 0);
  assert.equal(gearTargetFor('landing', APPROACH_EXTEND_ALT, 0), 1);
  assert.equal(gearTargetFor('landing', 20, 1), 1);

  // Hysteresis prevents chatter around APPROACH_EXTEND_ALT, while a go-around retracts above APPROACH_RETRACT_ALT.
  assert.equal(gearTargetFor('landing', APPROACH_EXTEND_ALT + 4, 1), 1);
  assert.equal(gearTargetFor('landing', APPROACH_RETRACT_ALT + 1, 1), 0);
});

test('stepGearPhase is frame-rate independent and completes in ~1.5-2s', () => {
  assert.ok(GEAR_RETRACT_DURATION >= 1.5 && GEAR_RETRACT_DURATION <= 2.0);
  assert.ok(GEAR_EXTEND_DURATION >= 1.5 && GEAR_EXTEND_DURATION <= 2.0);

  const simulate = (start, target, fps, seconds) => {
    const dt = 1 / fps;
    const steps = Math.round(seconds * fps);
    let phase = start;
    for (let i = 0; i < steps; i++) phase = stepGearPhase(phase, target, dt);
    return phase;
  };

  // Midway comparison across 30fps, 60fps, 120fps.
  const mid30 = simulate(1, 0, 30, 0.9);
  const mid60 = simulate(1, 0, 60, 0.9);
  const mid120 = simulate(1, 0, 120, 0.9);
  assert.ok(Math.abs(mid30 - mid60) < 1e-6);
  assert.ok(Math.abs(mid60 - mid120) < 1e-6);
  assert.ok(mid60 > 0.4 && mid60 < 0.6);

  // Reaches exact endpoints without lingering tail.
  assert.equal(simulate(1, 0, 60, GEAR_RETRACT_DURATION + 0.05), 0);
  assert.equal(simulate(0, 1, 60, GEAR_EXTEND_DURATION + 0.05), 1);
});

test('computeGearKinematics opens doors before struts emerge and hides at phase 0', () => {
  const up = computeGearKinematics(0);
  assert.equal(up.visible, false);
  assert.equal(up.doorOpen, 0);
  assert.equal(up.noseStrut, 0);
  assert.equal(up.mainLeftStrut, 0);
  assert.equal(up.mainRightStrut, 0);

  // Early phase: doors opening while struts remain tucked inside bays.
  const cracking = computeGearKinematics(0.10);
  assert.equal(cracking.visible, true);
  assert.ok(cracking.doorOpen > 0.2);
  assert.equal(cracking.noseStrut, 0);
  assert.equal(cracking.mainLeftStrut, 0);
  assert.equal(cracking.mainRightStrut, 0);

  // Mid phase: doors fully open, nose leading main struts slightly.
  const mid = computeGearKinematics(0.55);
  assert.equal(mid.doorOpen, 1);
  assert.ok(mid.noseStrut > mid.mainLeftStrut);
  assert.ok(mid.mainLeftStrut > mid.mainRightStrut);

  // Fully extended.
  const down = computeGearKinematics(1);
  assert.equal(down.visible, true);
  assert.equal(down.doorOpen, 1);
  assert.equal(down.noseStrut, 1);
  assert.equal(down.mainLeftStrut, 1);
  assert.equal(down.mainRightStrut, 1);
});

test('jet and tanker gear preserve ground contact height and hide cleanly when retracted', () => {
  const jetRig = buildJetLandingGear(dummyKit(), { wingY: -0.95 });
  assert.equal(jetRig.group.name, 'gear');
  assert.ok(jetRig.group.getObjectByName('noseGear'));
  assert.ok(jetRig.group.getObjectByName('mainGearLeft'));
  assert.ok(jetRig.group.getObjectByName('mainGearRight'));

  // Extended jet lowest point must remain -3.98 so built.wheelDrop is unchanged.
  jetRig.group.updateMatrixWorld(true);
  const jetBoxDown = new THREE.Box3().setFromObject(jetRig.group);
  assert.ok(Math.abs(jetBoxDown.min.y - (-3.98)) < 0.01, `expected -3.98, got ${jetBoxDown.min.y}`);

  // When stowed just before hiding, retracted nose & main gear stay inside fuselage/belly envelope.
  applyLandingGearPose(jetRig, 0.15);
  jetRig.group.updateMatrixWorld(true);
  const noseBoxTucked = new THREE.Box3().setFromObject(jetRig.nose.pivot);
  assert.ok(noseBoxTucked.min.y > -1.35, `nose tucked min.y ${noseBoxTucked.min.y} inside nose belly`);
  assert.ok(noseBoxTucked.max.y < 1.1, `nose tucked max.y ${noseBoxTucked.max.y} inside nose top`);
  const mainLeftBoxTucked = new THREE.Box3().setFromObject(jetRig.mainLeft.pivot);
  assert.ok(mainLeftBoxTucked.min.y > -1.55, `main tucked min.y ${mainLeftBoxTucked.min.y} inside belly fairing`);
  assert.ok(mainLeftBoxTucked.max.y < -0.1, `main tucked max.y ${mainLeftBoxTucked.max.y} below wing top`);

  // Phase 0 hides completely.
  applyLandingGearPose(jetRig, 0);
  assert.equal(jetRig.group.visible, false);

  // Tanker gear: widebody 4-wheel bogies + twin nose wheels at y = -4.22.
  const tankerRig = buildTankerLandingGear(dummyKit(), { wingY: -1.10 });
  assert.equal(tankerRig.wheels.length, 10); // 2 nose + 4 left main + 4 right main
  tankerRig.group.updateMatrixWorld(true);
  const tankerBoxDown = new THREE.Box3().setFromObject(tankerRig.group);
  assert.ok(Math.abs(tankerBoxDown.min.y - (-4.22)) < 0.01, `expected -4.22, got ${tankerBoxDown.min.y}`);

  applyLandingGearPose(tankerRig, 0.15);
  tankerRig.group.updateMatrixWorld(true);
  const tankerNoseTucked = new THREE.Box3().setFromObject(tankerRig.nose.pivot);
  assert.ok(tankerNoseTucked.min.y > -1.8, `tanker nose tucked min.y ${tankerNoseTucked.min.y}`);
  const tankerMainTucked = new THREE.Box3().setFromObject(tankerRig.mainLeft.pivot);
  assert.ok(tankerMainTucked.min.y > -2.0, `tanker main tucked min.y ${tankerMainTucked.min.y}`);
  assert.ok(tankerMainTucked.max.y < -0.2, `tanker main tucked max.y ${tankerMainTucked.max.y}`);
});

test('initLandingGear avoids spawn snap and updateLandingGear spins wheels on ground roll', () => {
  const entry = {
    landingGear: buildJetLandingGear(dummyKit(), { wingY: -0.95 }),
    scale: 0.74,
    state: 'cruising',
    altitude: 160,
    speed: 14,
  };
  initLandingGear(entry, entry.state, entry.altitude);
  assert.equal(entry.gearPhase, 0);
  assert.equal(entry.landingGear.group.visible, false);

  // Ground spawn starts fully down.
  entry.state = 'taxi';
  entry.altitude = 0;
  entry.speed = 6;
  initLandingGear(entry, entry.state, entry.altitude);
  assert.equal(entry.gearPhase, 1);
  assert.equal(entry.landingGear.group.visible, true);

  const beforeRot = entry.landingGear.wheels[0].group.rotation.x;
  updateLandingGear(entry, 0.05);
  assert.ok(entry.landingGear.wheels[0].group.rotation.x > beforeRot, 'wheels roll forward on ground');

  // Liftoff begins smooth retraction without snapping to 0.
  entry.state = 'takeoff';
  entry.altitude = 12;
  updateLandingGear(entry, 0.05);
  assert.ok(entry.gearPhase < 1 && entry.gearPhase > 0.9, `smooth retraction start: ${entry.gearPhase}`);
});
