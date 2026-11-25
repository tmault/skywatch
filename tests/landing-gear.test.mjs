import test from 'node:test';
import assert from 'node:assert/strict';
import * as THREE from '../web/vendor/three.module.js';
import {
  GEAR_CYCLE_SECONDS, gearTarget, advanceGear, easeGear,
} from '../web/landing-gear.js';

// Liveries paint onto canvases; geometry tests only need the calls to succeed.
const canvasStub = new Proxy(function () {}, {
  get: (_, key) => (key === Symbol.toPrimitive ? () => 0 : canvasStub),
  apply: () => canvasStub,
  set: () => true,
});
globalThis.document = { createElement: () => ({ getContext: () => canvasStub }) };
const aircraft = await import('../web/aircraft.js');

const RETRACTABLE = [
  'makeJetMesh', 'makeJumboMesh', 'makeFighterMesh', 'makeDroneMesh', 'makeTankerMesh',
];

function build(name) {
  const built = aircraft[name]('claude-sonnet-5-5');
  return { ...built, gear: built.group.getObjectByName('gear') };
}

// World-space vertices of every gear mesh (the model is unscaled here).
function pose(built) {
  built.group.updateMatrixWorld(true);
  const vertices = [];
  built.gear.traverse((o) => {
    if (!o.isMesh) return;
    const p = o.geometry.attributes.position;
    for (let i = 0; i < p.count; i++) {
      vertices.push(new THREE.Vector3().fromBufferAttribute(p, i).applyMatrix4(o.matrixWorld));
    }
  });
  return vertices;
}
// A wheel is a tyre cylinder plus a hub cylinder; every other group is a hinge.
const isWheel = (o) => o.isGroup && o.children.length === 2
  && o.children.every((c) => c.geometry?.type === 'CylinderGeometry');
const groups = (gear, wanted) => {
  const found = [];
  gear.traverse((o) => { if (o !== gear && o.isGroup && isWheel(o) === wanted) found.push(o); });
  return found;
};
const heights = (built) => {
  built.group.updateMatrixWorld(true);
  return groups(built.gear, true).map((w) => w.getWorldPosition(new THREE.Vector3()).y);
};
const mean = (values) => values.reduce((sum, v) => sum + v, 0) / values.length;
const furthest = (a, b) => a.reduce((d, v, i) => Math.max(d, v.distanceTo(b[i])), 0);

for (const name of RETRACTABLE) {
  test(`${name}: deployed gear sits at ground height and survives a full cycle`, () => {
    const built = build(name);
    const authored = pose(built);
    // finish() measured wheelDrop from the lowest tyre of the deployed gear.
    const groundY = () => new THREE.Box3().setFromObject(built.gear).min.y * built.scale;
    assert.ok(Math.abs(groundY() + built.wheelDrop) < 1e-9);

    built.gear.userData.setProgress(0);
    built.gear.userData.setProgress(1);
    assert.ok(furthest(authored, pose(built)) < 1e-9);
    assert.ok(Math.abs(groundY() + built.wheelDrop) < 1e-9);
  });

  test(`${name}: legs fold up smoothly and monotonically from t=1 to t=0`, () => {
    const built = build(name);
    const hinges = groups(built.gear, false);
    const deployed = heights(built);
    let previous = pose(built);
    let previousHeights = deployed;
    let previousAngles = hinges.map(() => 0);
    const steps = 100;
    for (let i = 1; i <= steps; i++) {
      built.gear.userData.setProgress(1 - i / steps);
      const current = pose(built);
      const angles = hinges.map((h) => 2 * Math.acos(Math.min(1, Math.abs(h.quaternion.w))));
      angles.forEach((a, j) => assert.ok(a >= previousAngles[j] - 1e-6, `hinge ${j} reversed at step ${i}`));
      // The wheels climb as a set. A tyre trailing its trunnion may dip a few
      // centimetres as the swing starts, but never visibly through the deck.
      const currentHeights = heights(built);
      assert.ok(mean(currentHeights) >= mean(previousHeights) - 0.005, `gear dipped at step ${i}`);
      currentHeights.forEach((y, j) => assert.ok(y >= deployed[j] - 0.05, `wheel ${j} sank at step ${i}`));
      assert.ok(furthest(previous, current) < 0.3, `gear jumped at step ${i}`);
      previous = current;
      previousHeights = currentHeights;
      previousAngles = angles;
    }
    assert.ok(deployed.length >= 3);
    previousHeights.forEach((y, j) => assert.ok(y - deployed[j] > 0.8, `wheel ${j} ends well up in the bay`));
    assert.ok(previousAngles.some((a) => a > 1.4), 'legs swing through a large angle');
  });

  test(`${name}: gear hides only once stowed`, () => {
    const { gear } = build(name);
    for (const t of [1, 0.5, 0.02, 0.0051]) {
      gear.userData.setProgress(t);
      assert.equal(gear.visible, true, `visible at ${t}`);
    }
    for (const t of [0.005, 0.001, 0]) {
      gear.userData.setProgress(t);
      assert.equal(gear.visible, false, `hidden at ${t}`);
    }
    gear.userData.setProgress(0.5);
    assert.equal(gear.visible, true);
  });
}

test('747 wheel-well doors close with the gear and reopen to their authored pose', () => {
  const built = build('makeJumboMesh');
  const doors = [];
  built.gear.traverse((o) => {
    if (o.isMesh && o.geometry.type === 'BoxGeometry') doors.push(o);
  });
  assert.equal(doors.length, 6);
  const up = () => {
    built.group.updateMatrixWorld(true);
    return doors.map((d) => new THREE.Vector3(0, 1, 0).transformDirection(d.matrixWorld).y);
  };
  built.gear.userData.setProgress(0.5);
  assert.ok(up().every((y) => y > 0.999), 'doors stay open while legs travel');
  built.gear.userData.setProgress(0);
  assert.ok(up().every((y) => Math.abs(y) < 1e-6), 'doors lie flat when stowed');
  built.gear.userData.setProgress(1);
  assert.ok(up().every((y) => y > 0.999));
});

test('fixed-gear Cessna has no retractable gear group', () => {
  assert.equal(aircraft.makeAgentMesh('local').group.getObjectByName('gear'), undefined);
});

test('gear is commanded up en route, climbing away and high on approach', () => {
  for (const state of ['cruising', 'holding', 'refueling']) assert.equal(gearTarget(state, 5), 0);
  for (const state of ['departing', 'takeoff']) {
    assert.equal(gearTarget(state, 18), 1);
    assert.equal(gearTarget(state, 18.1), 0);
  }
  assert.equal(gearTarget('landing', 70), 1);
  assert.equal(gearTarget('landing', 70.1), 0);
  for (const state of ['parked', 'ready', 'taxi', 'landed']) assert.equal(gearTarget(state, 0), 1);
});

test('a full cycle takes the mechanical time in either direction without overshoot', () => {
  for (const [from, to] of [[1, 0], [0, 1]]) {
    let progress = from;
    let eased = easeGear(from);
    let seconds = 0;
    while (progress !== to && seconds < 10) {
      progress = advanceGear(progress, to, 1 / 60);
      seconds += 1 / 60;
      assert.ok((easeGear(progress) - eased) * (to - from) >= 0);
      eased = easeGear(progress);
      assert.ok(eased >= 0 && eased <= 1);
    }
    assert.ok(Math.abs(seconds - GEAR_CYCLE_SECONDS) < 2 / 60, `${seconds}s`);
    assert.equal(eased, to);
  }
  assert.equal(advanceGear(0.5, 0, 60), 0);
  assert.equal(advanceGear(0.5, 1, 60), 1);
});
