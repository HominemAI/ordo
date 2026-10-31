/**
 * A server-sent-event reader. The door's live feeds are `text/event-stream`.
 * The shared stream (`GET /live`) sends `ready` first, then one `invalidate`
 * per signal naming its read's key. The single-read stream (`?live=1`) sends
 * a default `data:` event first (the ordinary envelope), then `invalidate`
 * events. Both put `:hb` comments between.
 *
 * Kept to what the feed uses: `event:` and `data:` fields, blocks split on a
 * blank line, comments skipped. No `id:` or `retry:`, because the door sends
 * neither and reconnect policy lives in `live.ts`.
 */

export interface SseFrame {
  /** `message` for a block with no `event:` field. */
  readonly event: string;
  readonly data: unknown;
}

export async function* frames(response: Response): AsyncGenerator<SseFrame, void, void> {
  if (!response.body) return;
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  /** How far into `buffer` has been searched for a blank line. */
  let scanned = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      buffer += done ? decoder.decode() : decoder.decode(value, { stream: true });
      // Search only what arrived since the last look (L2): a frame split over
      // many chunks costs one pass over its bytes, not one per chunk. One
      // character back, since a chunk may end between the two newlines.
      let start = 0;
      let end: number;
      while ((end = buffer.indexOf('\n\n', Math.max(start, scanned - 1))) >= 0) {
        const frame = parse(buffer.slice(start, end));
        start = end + 2;
        scanned = start;
        if (frame) yield frame;
      }
      if (start > 0) buffer = buffer.slice(start);
      scanned = buffer.length;
      if (done) return;
    }
  } finally {
    // Leaving the loop early (an abort, an unsubscribe) must release the
    // connection, or the door keeps a subscription open for a hook that is gone.
    void reader.cancel().catch(() => undefined);
  }
}

function parse(block: string): SseFrame | null {
  let event = 'message';
  const data: string[] = [];
  for (const line of block.split('\n')) {
    if (line.startsWith(':')) continue;
    const colon = line.indexOf(':');
    const field = colon < 0 ? line : line.slice(0, colon);
    let value = colon < 0 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') event = value;
    else if (field === 'data') data.push(value);
  }
  if (data.length === 0) return null;
  try {
    return { event, data: JSON.parse(data.join('\n')) as unknown };
  } catch {
    return null;
  }
}
