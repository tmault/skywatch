import assert from 'node:assert/strict';
import test from 'node:test';
import {
  FLAGSHIP_MODEL_PATTERNS, isFlagshipAgent, isFlagshipModel,
} from '../web/flagship.js';

test('flagship patterns exist as an extensible list', () => {
  assert.ok(Array.isArray(FLAGSHIP_MODEL_PATTERNS));
  assert.ok(FLAGSHIP_MODEL_PATTERNS.length >= 2);
});

test('Anthropic Fable and OpenAI Astra families match flagship models', () => {
  const matches = [
    'claude-fable-5-2',
    'claude-fable',
    'anthropic/claude-fable-5-2',
    'claude-fable-5-2:high',
    'CLAUDE-FABLE-6',
    'gpt-6.2-astra',
    'gpt-6-astra',
    'openai/gpt-6.2-astra',
    'gpt-6.2-astra-preview',
    'gpt-6-astra:max',
  ];
  for (const model of matches) {
    assert.equal(isFlagshipModel(model), true, `expected flagship match for ${model}`);
  }
});

test('non-flagship frontier and local models stay on standard airframes', () => {
  const nonMatches = [
    'claude-opus-5-5',
    'claude-sonnet-5-5',
    'claude-haiku-4-5',
    'gpt-6.2-sol',
    'gpt-6.1-sol',
    'gpt-6-luna',
    'gpt-5.4',
    'gemini-4-argon',
    'local-models/qwen3.8-27b',
    'ollama/deepseek-v4.1-flash',
    'opencode-go/glm-5.3-flash',
    'fable-local',
    'astra-mini',
    '',
    null,
    undefined,
  ];
  for (const model of nonMatches) {
    assert.equal(isFlagshipModel(model), false, `did not expect flagship match for ${String(model)}`);
  }
});

test('only frontier agent aircraft receive the flagship 747 tier', () => {
  assert.equal(isFlagshipAgent({
    kind: 'agent', model_class: 'frontier', model: 'claude-fable-5-2',
  }), true);
  assert.equal(isFlagshipAgent({
    kind: 'agent', model_class: 'frontier', model: 'gpt-6.2-astra',
  }), true);
  assert.equal(isFlagshipAgent({
    kind: 'agent', model_class: 'frontier', model: 'claude-opus-5-5',
  }), false);
  assert.equal(isFlagshipAgent({
    kind: 'agent', model_class: 'local', model: 'claude-fable-5-2',
  }), false);
  assert.equal(isFlagshipAgent({
    kind: 'tanker', model_class: 'frontier', model: 'claude-fable-5-2',
  }), false);
  assert.equal(isFlagshipAgent({
    kind: 'fighter', model_class: 'frontier', model: 'gpt-6.2-astra',
  }), false);
  assert.equal(isFlagshipAgent({
    kind: 'drone', model_class: 'frontier', model: 'gpt-6.2-astra',
  }), false);
});
