// Token usage from one `claude -p --output-format stream-json --verbose`
// transcript.
//
// Two sources, kept apart in the record:
//
// 1. The final `result` event. `modelUsage` is per model, camelCase
//    (inputTokens, outputTokens, cacheReadInputTokens,
//    cacheCreationInputTokens, costUSD), and includes subagents. Its
//    top-level `usage` (snake_case) covers the main loop only. This is the
//    number the bench reports.
// 2. A sum over `assistant` events, deduplicated by `message.id` (one API
//    response can arrive as several events), split into the main thread and
//    subagents (`parent_tool_use_id` set). A cross-check, and the fallback
//    when the run was stopped before a `result` event. Per-event
//    `output_tokens` can be a placeholder, so this sum's output count is a
//    floor.

/** @typedef {{ input: number, output: number, cache_read: number, cache_write: number }} Counts */

/** @returns {Counts} */
export const zero = () => ({ input: 0, output: 0, cache_read: 0, cache_write: 0 });

/** @param {Counts} a @param {Counts} b @returns {Counts} */
export function add(a, b) {
  return {
    input: a.input + b.input,
    output: a.output + b.output,
    cache_read: a.cache_read + b.cache_read,
    cache_write: a.cache_write + b.cache_write,
  };
}

/** Every token, all 4 counts together. @param {Counts} c */
export const total = (c) => c.input + c.output + c.cache_read + c.cache_write;

const n = (v) => (typeof v === 'number' && Number.isFinite(v) ? v : 0);

/** @param {Record<string, unknown> | undefined} u snake_case API usage */
function fromApi(u) {
  return {
    input: n(u?.input_tokens),
    output: n(u?.output_tokens),
    cache_read: n(u?.cache_read_input_tokens),
    cache_write: n(u?.cache_creation_input_tokens),
  };
}

/** @param {Record<string, unknown> | undefined} u camelCase modelUsage entry */
function fromModelUsage(u) {
  return {
    input: n(u?.inputTokens),
    output: n(u?.outputTokens),
    cache_read: n(u?.cacheReadInputTokens),
    cache_write: n(u?.cacheCreationInputTokens),
  };
}

/**
 * Read a transcript's text (one JSON object per line).
 * @param {string} text
 */
export function readUsage(text) {
  /** @type {Map<string, { usage: Counts, sub: boolean, model: string }>} */
  const messages = new Map();
  let init = null;
  let result = null;
  let unparsed = 0;
  let mainTurns = 0;

  for (const line of text.split('\n')) {
    if (!line.trim()) continue;
    let ev;
    try {
      ev = JSON.parse(line);
    } catch {
      unparsed++;
      continue;
    }
    if (ev.type === 'system' && ev.subtype === 'init') init = ev;
    else if (ev.type === 'assistant' && ev.message) {
      const id = ev.message.id ?? `anon-${messages.size}`;
      const sub = ev.parent_tool_use_id != null;
      if (!messages.has(id) && !sub) mainTurns++;
      // A later event for the same message carries the fuller usage.
      messages.set(id, { usage: fromApi(ev.message.usage), sub, model: String(ev.message.model ?? '') });
    } else if (ev.type === 'result') result = ev;
  }

  let main = zero();
  let subagents = zero();
  for (const m of messages.values()) {
    if (m.sub) subagents = add(subagents, m.usage);
    else main = add(main, m.usage);
  }

  /** @type {Record<string, Counts & { cost_usd: number }>} */
  const byModel = {};
  let reported = null;
  if (result?.modelUsage && typeof result.modelUsage === 'object') {
    reported = zero();
    for (const [model, u] of Object.entries(result.modelUsage)) {
      const c = fromModelUsage(u);
      byModel[model] = { ...c, cost_usd: n(u?.costUSD) };
      reported = add(reported, c);
    }
  }

  const stream = add(main, subagents);
  return {
    // What the bench counts: the result's modelUsage, or the stream when the
    // run never reached a result.
    counts: reported ?? stream,
    source: reported ? 'result.modelUsage' : 'assistant events (no result event: the run was stopped)',
    by_model: byModel,
    stream: { main, subagents, messages: messages.size, main_turns: mainTurns },
    result_usage_main_loop: result ? fromApi(result.usage) : null,
    cost_usd_list_price: result ? n(result.total_cost_usd) : null,
    num_turns: result ? n(result.num_turns) : mainTurns,
    subtype: result?.subtype ?? null,
    is_error: result?.is_error ?? null,
    session_id: result?.session_id ?? init?.session_id ?? null,
    duration_ms: result ? n(result.duration_ms) : null,
    duration_api_ms: result ? n(result.duration_api_ms) : null,
    init: init
      ? {
          model: init.model ?? null,
          claude_code_version: init.claude_code_version ?? null,
          tools: init.tools ?? null,
          mcp_servers: init.mcp_servers ?? null,
          permission_mode: init.permissionMode ?? null,
          cwd: init.cwd ?? null,
        }
      : null,
    unparsed_lines: unparsed,
  };
}
