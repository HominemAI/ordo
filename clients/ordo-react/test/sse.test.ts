/**
 * The server-sent-event reader (L2 of `docs/audit-client-blocking-2026-10-10.md`):
 * a frame split across many chunks arrives whole, and the reader searches
 * only what arrived since its last look.
 */

import { describe, expect, it } from 'vitest';
import { frames, type SseFrame } from '../src/sse.js';

function response(chunks: readonly string[]): Response {
  const enc = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(c) {
      for (const chunk of chunks) c.enqueue(enc.encode(chunk));
      c.close();
    },
  });
  return new Response(body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
}

async function all(chunks: readonly string[]): Promise<SseFrame[]> {
  const out: SseFrame[] = [];
  for await (const f of frames(response(chunks))) out.push(f);
  return out;
}

describe('the event-stream reader', () => {
  const text = 'event: ready\ndata: {"a":1}\n\n:hb\n\ndata: {"b":[1,2,3]}\n\nevent: invalidate\ndata: {"key":"k"}\n\n';
  const want = [
    { event: 'ready', data: { a: 1 } },
    { event: 'message', data: { b: [1, 2, 3] } },
    { event: 'invalidate', data: { key: 'k' } },
  ];

  it('reads frames that arrive in one chunk', async () => {
    expect(await all([text])).toEqual(want);
  });

  it('reads frames split one character per chunk, the blank line split too (L2)', async () => {
    expect(await all([...text])).toEqual(want);
  });

  it('reads frames split at every possible point', async () => {
    for (let at = 1; at < text.length; at++) {
      expect(await all([text.slice(0, at), text.slice(at)])).toEqual(want);
    }
  });
});
