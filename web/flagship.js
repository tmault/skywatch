// Flagship frontier model families that earn the Boeing 747 "Queen of the Sky"
// airframe. Extend this list with regexes matched against the normalized model
// id (lowercased, provider prefix and :tag/:effort suffixes stripped).
export const FLAGSHIP_MODEL_PATTERNS = [
  /^claude-fable(?:[-.\d]|$)/,     // Anthropic Fable family (e.g. claude-fable-5-2)
  /^gpt-[\d.]+-astra(?:[-.\d]|$)/, // OpenAI Astra family (e.g. gpt-6-astra, gpt-6.2-astra)
];

function normalizeModelId(model) {
  if (typeof model !== 'string') return '';
  return model.trim().toLowerCase().split('/').pop().replace(/:.*$/, '');
}

export function isFlagshipModel(model) {
  const base = normalizeModelId(model);
  if (!base) return false;
  return FLAGSHIP_MODEL_PATTERNS.some((re) => re.test(base));
}

export function isFlagshipAgent(aircraft) {
  return Boolean(
    aircraft
    && aircraft.kind === 'agent'
    && aircraft.model_class === 'frontier'
    && isFlagshipModel(aircraft.model),
  );
}
