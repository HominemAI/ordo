// node --test bench/token/runner/test/*.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readUsage } from '../lib/usage.mjs';

const line = (o) => JSON.stringify(o);
const usage = (i, o, r, w) => ({ input_tokens: i, output_tokens: o, cache_read_input_tokens: r, cache_creation_input_tokens: w });

const stream = [
  line({ type: 'system', subtype: 'init', session_id: 's1', model: 'claude-sonnet-5-5', tools: ['Bash'], mcp_servers: [] }),
  // One API message arriving as two events: counted once, the later usage kept.
  line({ type: 'assistant', parent_tool_use_id: null, message: { id: 'm1', model: 'claude-sonnet-5-5', usage: usage(10, 1, 100, 50) } }),
  line({ type: 'assistant', parent_tool_use_id: null, message: { id: 'm1', model: 'claude-sonnet-5-5', usage: usage(10, 40, 100, 50) } }),
  line({ type: 'assistant', parent_tool_use_id: null, message: { id: 'm2', model: 'claude-sonnet-5-5', usage: usage(5, 20, 200, 0) } }),
  // A subagent's message.
  line({ type: 'assistant', parent_tool_use_id: 'tu1', message: { id: 'm3', model: 'claude-haiku', usage: usage(7, 9, 0, 30) } }),
  'not json',
  line({
    type: 'result',
    subtype: 'success',
    session_id: 's1',
    num_turns: 2,
    total_cost_usd: 0.5,
    usage: usage(15, 60, 300, 50),
    modelUsage: {
      'claude-sonnet-5-5': { inputTokens: 15, outputTokens: 60, cacheReadInputTokens: 300, cacheCreationInputTokens: 50, costUSD: 0.4 },
      'claude-haiku': { inputTokens: 7, outputTokens: 9, cacheReadInputTokens: 0, cacheCreationInputTokens: 30, costUSD: 0.1 },
    },
  }),
].join('\n');

test('the result modelUsage is what counts, subagents included', () => {
  const u = readUsage(stream);
  assert.equal(u.source, 'result.modelUsage');
  assert.deepEqual(u.counts, { input: 22, output: 69, cache_read: 300, cache_write: 80 });
  assert.equal(u.by_model['claude-haiku'].cost_usd, 0.1);
  assert.equal(u.num_turns, 2);
  assert.equal(u.session_id, 's1');
  assert.equal(u.unparsed_lines, 1);
});

test('the stream sum dedupes by message id and splits off subagents', () => {
  const u = readUsage(stream);
  assert.deepEqual(u.stream.main, { input: 15, output: 60, cache_read: 300, cache_write: 50 });
  assert.deepEqual(u.stream.subagents, { input: 7, output: 9, cache_read: 0, cache_write: 30 });
  assert.equal(u.stream.messages, 3);
  assert.equal(u.stream.main_turns, 2);
});

test('a run stopped before its result falls back to the stream', () => {
  const cut = stream.split('\n').slice(0, 5).join('\n');
  const u = readUsage(cut);
  assert.match(u.source, /no result event/);
  assert.deepEqual(u.counts, { input: 22, output: 69, cache_read: 300, cache_write: 80 });
  assert.equal(u.subtype, null);
});
