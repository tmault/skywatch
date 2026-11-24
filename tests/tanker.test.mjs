import assert from 'node:assert/strict';
import test from 'node:test';
import { formatCountdown, formatTankerWait } from '../web/tanker.js';

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
