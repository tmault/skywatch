import assert from 'node:assert/strict';
import test from 'node:test';
import { formatNextRun, reaperDetails, reaperLabelStatus } from '../web/reaper.js';
import { classifyAircraftUrgency } from '../web/alerts.js';
const now = Date.parse('2026-01-01T00:00:00Z');
const job = { enabled: true, schedule: 'every 5 min', next_run_at: '2026-01-01T02:10:00Z', last_run_status: 'failed', phase: 'parked', run_count: 4 };
test('countdowns advance from timestamps and reject ambiguous dates', () => {
  assert.equal(formatNextRun(job, now), 'next in 2h 10m');
  assert.equal(formatNextRun(job, now + 7800000), 'due now');
  assert.equal(formatNextRun({ next_run_at: '2026-01-01T02:10:00' }, now), null);
  assert.match(reaperLabelStatus({ scheduled_job: job }, now), /LAST RUN FAILED/);
});
test('radio shows safe summaries and disabled status', () => {
  assert.deepEqual(reaperDetails({ scheduled_job: { ...job, enabled: false } }, now), [
    'SCHEDULE: every 5 min · disabled', 'NEXT RUN: none while disabled', 'LAST RUN: failed', 'RUNS: 4',
  ]);
});
test('failed jobs need attention in every lifecycle state, running jobs do not', () => {
  for (const state of ['parked', 'taxi', 'ready', 'landing', 'landed']) {
    const aircraft = { kind: 'drone', state, scheduled_job: job };
    assert.equal(classifyAircraftUrgency(aircraft).kind, 'failed');
    assert.equal(classifyAircraftUrgency({ ...aircraft, scheduled_job: { ...job, running: true } }), null);
  }
});
