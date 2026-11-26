import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from '../web/vendor/three.module.js';
import { formatCountdown, formatTankerWait } from '../web/tanker.js';

const canvasStub = new Proxy(function () {}, {
  get: (_, key) => (key === Symbol.toPrimitive ? () => 0 : canvasStub),
  apply: () => canvasStub,
  set: () => true,
});
if (!globalThis.document) {
  globalThis.document = { createElement: () => ({ getContext: () => canvasStub }) };
}
const {
  makeAgentMesh, makeJetMesh, makeJumboMesh, makeFighterMesh, makeDroneMesh,
} = await import('../web/aircraft.js');

const beforeReset = Date.parse('2026-11-24T01:59:30Z');
const original = {
  provider: 'anthropic',
  hold_reason: 'API rate limit reached',
  refuel_reset_at: '2026-11-24T02:00:00Z',
};

test('the radio shows the provider, absolute deadline and live countdown', () => {
  assert.equal(formatCountdown(original.refuel_reset_at, beforeReset), 'resets in 30s');
  assert.equal(
    formatTankerWait(original, beforeReset),
    'Claude · API rate limit reached · Reset 2026-11-24 02:00:00 UTC · resets in 30s',
  );
});

test('deadline changes and expiry keep their meaning while a thread still waits', () => {
  const extended = { ...original, refuel_reset_at: '2026-11-24T02:02:00+00:00' };
  assert.match(formatTankerWait(extended, beforeReset), /02:02:00 UTC · resets in 2m 30s/);
  assert.match(formatTankerWait(original, beforeReset + 30_000), /Reset time passed · still waiting/);
});

test('absent dates, dates without a timezone and stale reports are explicit', () => {
  assert.match(formatTankerWait({ provider: 'opencode' }, beforeReset), /^OpenCode.*Reset time unknown$/);
  assert.equal(formatCountdown('2026-11-24T02:00:00', beforeReset), 'reset unknown');
  assert.match(formatTankerWait({ ...original, data_stale: true }, beforeReset), /Last reported reset.*UTC$/);
});

test('every receiver airframe aligns its refuelPort with its nose tip', () => {
  for (const [name, build] of [
    ['cessna', () => makeAgentMesh('llama-3')],
    ['jet', () => makeJetMesh('gpt-6.1-sol')],
    ['jumbo', () => makeJumboMesh('gpt-6.2-astra')],
    ['fighter', () => makeFighterMesh('claude-sonnet-5-5')],
    ['drone', () => makeDroneMesh('claude-sonnet-5-5')],
  ]) {
    const built = build();
    built.group.updateMatrixWorld(true);
    const fuselage = built.group.getObjectByName('fuselage');
    assert.ok(fuselage, `${name}: missing fuselage`);
    const box = new THREE.Box3().setFromObject(fuselage);
    assert.ok(
      Math.abs(built.refuelPort.z - box.max.z) < 0.6,
      `${name}: refuelPort.z=${built.refuelPort.z} not aligned with nose max.z=${box.max.z}`,
    );
    assert.ok(
      built.refuelPort.y <= box.max.y * 0.5 && built.refuelPort.y >= box.min.y * 0.6,
      `${name}: refuelPort.y=${built.refuelPort.y} sits outside nose vertical band [${box.min.y}, ${box.max.y}]`,
    );
  }
});
