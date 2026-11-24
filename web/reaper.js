// Scheduled-job Reaper drones: read-only labels and radio details derived
// from the server's `scheduled_job` summary. Timestamps are formatted here so
// countdowns stay live between snapshots.

export function isReaper(a) {
  return Boolean(a && a.kind === 'drone' && a.scheduled_job);
}

function parseIso(iso) {
  if (typeof iso !== 'string' || !/T\d\d:\d\d(?::\d\d(?:\.\d+)?)?(?:Z|[+-]\d\d:\d\d)$/.test(iso)) return NaN;
  return Date.parse(iso);
}

/** "2h 10m", "4m 05s", "35s" (always positive). */
export function formatSpan(ms) {
  const secs = Math.max(0, Math.round(ms / 1000));
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (d) return h ? `${d}d ${h}h` : `${d}d`;
  if (h) return m ? `${h}h ${m}m` : `${h}h`;
  if (m) return m < 10 ? `${m}m ${String(s).padStart(2, '0')}s` : `${m}m`;
  return `${s}s`;
}

/** Live countdown to the next run: "next in 2h 10m", "due now", or null. */
export function formatNextRun(job, now = Date.now()) {
  const at = parseIso(job?.next_run_at);
  if (!Number.isFinite(at)) return null;
  const ms = at - now;
  if (ms <= 0) return ms > -60_000 ? 'due now' : `overdue ${formatSpan(-ms)}`;
  return `next in ${formatSpan(ms)}`;
}

/** Absolute UTC time without seconds: "2026-11-24 02:00 UTC". */
export function formatUtc(iso) {
  const at = parseIso(iso);
  if (!Number.isFinite(at)) return null;
  return new Date(at).toISOString().replace('T', ' ').replace(/:\d\d(?:\.\d+)?Z$/, ' UTC');
}

/** Relative age: "3h ago", "just now". */
export function formatAgo(iso, now = Date.now()) {
  const at = parseIso(iso);
  if (!Number.isFinite(at)) return null;
  const ms = now - at;
  if (ms < 60_000) return 'just now';
  return `${formatSpan(ms).replace(/ \d+s$/, '')} ago`;
}

const PHASE_TEXT = {
  parked: 'ON RAMP',
  disabled: 'DISABLED',
  taxi_out: 'TAXIING OUT',
  holding_short: 'HOLDING SHORT',
  launching: 'LAUNCHING',
  on_station: 'RUNNING',
  returning: 'RETURNING',
};

/** Secondary label line after the callsign. */
export function reaperLabelStatus(a, now = Date.now()) {
  const job = a.scheduled_job;
  if (!job) return '';
  if (!job.enabled && job.phase === 'disabled') return 'DISABLED';
  if (job.phase === 'parked' || job.phase === 'taxi_out' || job.phase === 'holding_short') {
    const next = formatNextRun(job, now);
    if (job.phase !== 'parked') return `${PHASE_TEXT[job.phase]} · ${next || 'due'}`.toUpperCase();
    const failed = job.last_run_status === 'failed' ? 'LAST RUN FAILED · ' : '';
    return `${failed}${(next || 'IDLE').toUpperCase()}`;
  }
  return PHASE_TEXT[job.phase] || String(job.phase || '').toUpperCase();
}

/** Radio-panel state line. */
export function reaperStateText(a) {
  const job = a.scheduled_job;
  return {
    parked: 'parked on Reaper ramp',
    disabled: 'disabled · parked',
    taxi_out: 'due · taxiing out',
    holding_short: 'due · holding short',
    launching: 'run started · launching',
    on_station: 'running · on station',
    returning: 'run finished · returning',
  }[job?.phase] || a.state;
}

function statusText(job) {
  const s = (job.last_run_status || '').toLowerCase();
  if (s === 'never') return 'never run';
  if (s === 'running') return 'running now';
  return s || 'unknown';
}

/** Read-only detail lines for the radio panel. */
export function reaperDetails(a, now = Date.now()) {
  const job = a.scheduled_job || {};
  const lines = [];
  lines.push(`SCHEDULE: ${job.schedule || 'unknown'}${job.enabled ? '' : ' · disabled'}`);
  if (job.running) {
    lines.push('RUN: in progress');
  }
  const nextAbs = formatUtc(job.next_run_at);
  const next = formatNextRun(job, now);
  if (!job.enabled) lines.push('NEXT RUN: none while disabled');
  else if (nextAbs) lines.push(`NEXT RUN: ${nextAbs} · ${next}`);
  else lines.push('NEXT RUN: not scheduled');
  const lastAbs = formatUtc(job.last_run_at);
  const last = lastAbs ? `${lastAbs} (${formatAgo(job.last_run_at, now)}) · ${statusText(job)}` : statusText(job);
  lines.push(`LAST RUN: ${last}`);
  if (Number.isFinite(job.run_count)) lines.push(`RUNS: ${job.run_count}`);
  return lines;
}
