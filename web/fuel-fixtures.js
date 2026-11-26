// Explicitly opt-in synthetic readings; never used by the live source.
export function syntheticFuelSnapshot(now = Date.now()) {
  const date = offset => new Date(now + offset).toISOString();
  const window = (label, remaining, minutes, extra = {}) => ({ kind: label === 'Weekly' ? 'weekly' : label === 'Monthly' ? 'monthly' : 'session', label, remaining_percent: remaining, duration_minutes: label === '5-hour' ? 300 : label === 'Weekly' ? 10080 : 43200, resets_at: date(minutes * 60000), model_ids: [], provider_ids: [], ...extra });
  const account = (key, provider, status, windows, checked = date(0)) => ({ account_key: key, provider_label: provider, account_label: `Demo account ${key}`, status, checked_at: checked, windows });
  return { source_kind: 'synthetic-demo', status: 'available', accounts: [
    account('A', 'Codex', 'available', [window('5-hour', 63, 134), window('Weekly', 28, 2640), window('Weekly', 12, 2640, { model_ids: ['demo-model'] })]),
    account('B', 'Claude', 'available', [window('5-hour', 0, 95), window('Weekly', 100, 3200)]),
    account('C', 'Cursor', 'available', [window('Monthly', 74, 12000)]),
    account('D', 'Codex', 'stale', [window('5-hour', 17, -10), window('Weekly', null, 1000, { resets_at: null })], date(-1800000)),
    account('E', 'OpenCode', 'unavailable', []),
  ] };
}
