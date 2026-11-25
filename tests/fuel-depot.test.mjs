import test from 'node:test';
import assert from 'node:assert/strict';
import { windowSummary, visibleWindows, disposeGroup } from '../web/fuel-depot.js';
const now = Date.parse('2026-11-25T10:00:00Z');
test('true zero/full, unknown and reset expiry never refill', () => {
  for (const value of [0, 100]) assert.match(windowSummary({ remaining_percent: value }, 'available', now), new RegExp(`${value}% remaining`));
  assert.match(windowSummary({ remaining_percent: null }, 'unknown', now), /Unknown/);
  assert.match(windowSummary({ remaining_percent: 28, resets_at: '2026-11-25T09:00:00Z' }, 'available', now), /28% remaining.*refresh pending/);
  assert.match(windowSummary({ remaining_percent: 63 }, 'stale', now), /Stale/);
});
test('only unscoped actual windows occupy tanks; scoped details remain separate', () => {
  const monthly = { kind: 'monthly', model_ids: [], provider_ids: [] };
  assert.deepEqual(visibleWindows([monthly, { kind: 'weekly', model_ids: ['x'] }]), [monthly]);
});
test('disposal frees shared geometry/material/texture exactly once', () => {
  const count = { geometry: 0, material: 0, texture: 0 };
  const texture = { isTexture: true, dispose() { count.texture++; } };
  const material = { map: texture, dispose() { count.material++; } };
  const geometry = { dispose() { count.geometry++; } };
  disposeGroup({ traverse(fn) { fn({ geometry, material }); fn({ geometry, material }); } });
  assert.deepEqual(count, { geometry: 1, material: 1, texture: 1 });
});
test('synthetic fixtures stay explicit and include every edge case', async () => {
  const { syntheticFuelSnapshot } = await import('../web/fuel-fixtures.js');
  const snapshot = syntheticFuelSnapshot(now);
  assert.equal(snapshot.source_kind, 'synthetic-demo');
  assert.equal(snapshot.accounts.length, 5);
  assert.equal(snapshot.accounts[2].windows[0].kind, 'monthly');
  assert.equal(snapshot.accounts[4].status, 'unavailable');
  assert.match(windowSummary(snapshot.accounts[3].windows[0], 'stale', now), /refresh pending/);
});
