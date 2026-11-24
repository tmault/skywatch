const providerNames = {
  anthropic: 'Claude',
  openai: 'OpenAI',
  google: 'Gemini',
  opencode: 'OpenCode',
  deepseek: 'DeepSeek',
  zhipu: 'Z.ai',
  qwen: 'Qwen',
  mistral: 'Mistral',
  xai: 'Grok',
  meta: 'Llama',
  moonshot: 'Kimi',
};

function resetTimestamp(iso) {
  if (typeof iso !== 'string' || !/T\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)$/.test(iso)) {
    return NaN;
  }
  return Date.parse(iso);
}

export function formatCountdown(iso, now = Date.now()) {
  const ms = resetTimestamp(iso) - now;
  if (!Number.isFinite(ms)) return 'reset unknown';
  if (ms <= 0) return 'reset time passed';
  const seconds = Math.ceil(ms / 1000);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remaining = [];
  if (hours) remaining.push(`${hours}h`);
  if (minutes) remaining.push(`${minutes}m`);
  remaining.push(`${seconds % 60}s`);
  return `resets in ${remaining.join(' ')}`;
}

export function formatTankerWait(aircraft, now = Date.now()) {
  const provider = providerNames[aircraft.provider] || 'Unknown provider';
  const reason = aircraft.hold_reason || 'Provider usage limit reached';
  const reset = resetTimestamp(aircraft.refuel_reset_at);
  if (!Number.isFinite(reset)) return `${provider} · ${reason} · Reset time unknown`;
  const timestamp = new Date(reset).toISOString().replace('T', ' ').replace(/\.\d+Z$/, ' UTC');
  const countdown = formatCountdown(aircraft.refuel_reset_at, now);
  if (aircraft.data_stale) return `${provider} · ${reason} · Last reported reset ${timestamp}`;
  const status = reset <= now ? 'Reset time passed · still waiting' : countdown;
  return `${provider} · ${reason} · Reset ${timestamp} · ${status}`;
}
