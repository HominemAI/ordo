/**
 * A door that keeps streams open, for the live and device tests.
 *
 * `harness.ts`'s `MockDoor` answers one request and is done. Live and sync
 * are the opposite: the door holds the response open and the test pushes
 * frames into it, so what a hook does between frames can be asserted. One
 * `fetch` serves both clients (this package's and the SDK's), and routes on
 * the URL:
 *
 * - `?live=1&sync=1` is a sync feed (the SDK manager's), and a bare
 *   `?live=1` a single-read stream,
 * - `GET /live` is a shared live stream (this package's hub), and
 *   `POST /live/{stream}` / `DELETE /live/{stream}/{key}` add and drop its
 *   reads (`api-engine-design.md` §6.1),
 * - anything else is an ordinary call, answered from a queue.
 */

import { createElement, type ReactNode } from 'react';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { DescriptorClient, type DescriptorClientOptions } from '../src/client.js';
import { OrdoProvider, type OrdoProviderProps } from '../src/hooks/context.js';
import { iface } from './harness.js';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export class Stream {
  readonly body: ReadableStream<Uint8Array>;
  #controller!: ReadableStreamDefaultController<Uint8Array>;
  #done = false;
  readonly url: string;
  /** The `cache` mode the request asked for, if any. */
  cache: RequestCache | undefined;
  /** The id a shared stream names itself by in its `ready` frame. */
  readonly id: string;

  constructor(url: string, signal?: AbortSignal | null, id = '') {
    this.url = url;
    this.id = id;
    this.body = new ReadableStream<Uint8Array>({
      start: (c) => {
        this.#controller = c;
      },
    });
    signal?.addEventListener('abort', () => {
      if (this.#done) return;
      this.#done = true;
      this.#controller.error(signal.reason ?? new DOMException('aborted', 'AbortError'));
    });
  }

  get done(): boolean {
    return this.#done;
  }

  push(text: string): void {
    if (!this.#done) this.#controller.enqueue(new TextEncoder().encode(text));
  }

  close(): void {
    if (this.#done) return;
    this.#done = true;
    this.#controller.close();
  }
}

export const sse = (event: string | null, data: unknown): string =>
  `${event ? `event: ${event}\n` : ''}data: ${JSON.stringify(data)}\n\n`;

/** One read the hub added to a shared stream. */
export interface LiveRead {
  readonly stream: string;
  readonly key: string;
  readonly op: string;
  readonly params: Record<string, string>;
  /** The read's refinement, as the hub sent it (R6). */
  readonly refine?: unknown;
  readonly headers: Record<string, string>;
  dropped: boolean;
}

export class LiveDoor {
  /** Ordinary (non-stream) requests, in order. */
  readonly calls: { url: string; method: string; body: unknown }[] = [];
  /** Shared streams, `GET /live`, in the order they were opened. */
  readonly live: Stream[] = [];
  /** The request headers of each `GET /live`, in order. */
  readonly liveHeaders: Record<string, string>[] = [];
  readonly sync: Stream[] = [];
  /** Single-read streams, `GET /op/{name}?live=1`. The hub opens none. */
  readonly single: Stream[] = [];
  /** Reads added to shared streams, refused ones excluded. */
  readonly reads: LiveRead[] = [];
  /** `POST /live/{stream}` requests, refused ones included. */
  readonly adds: { stream: string; key: string; op: string }[] = [];
  /** When true `fetch` rejects the way a browser does with no network. */
  offline = false;
  /** When set, answers every ordinary call from its URL instead of the reply queue. */
  answer: ((url: URL) => unknown) | null = null;
  /** Answers for ordinary calls, consumed in order; the last one repeats. */
  #replies: { status?: number; body: unknown }[] = [];
  /** Refusals for the next stream opens, consumed in order. */
  #streamRefusals: { status: number; code: string }[] = [];
  /** Refusals for added reads, by op, consumed in order. */
  #readRefusals: { op: string; status: number; code: string }[] = [];
  /**
   * Streams the door no longer answers for, although their feed is still
   * open: what a client sees after its token was replaced, since a stream
   * belongs to the token that opened it.
   */
  readonly #forgotten = new Set<string>();

  /** Stop answering adds and drops for stream `id`, leaving its feed open. */
  forget(id: string): this {
    this.#forgotten.add(id);
    return this;
  }

  reply(...replies: { status?: number; body: unknown }[]): this {
    this.#replies.push(...replies);
    return this;
  }

  refuseNextStream(status: number, code: string): this {
    this.#streamRefusals.push({ status, code });
    return this;
  }

  /** Refuse the next read of `op` added to a shared stream. */
  refuseRead(op: string, status: number, code: string): this {
    this.#readRefusals.push({ op, status, code });
    return this;
  }

  /** Send the `ready` frame on shared stream `i` (default: the newest). */
  ready(i = this.live.length - 1): void {
    const stream = this.live[i]!;
    stream.push(sse('ready', { stream: stream.id }));
  }

  /** End shared stream `i` (default: the newest) the way the door does on a refusal: a last `error` frame, then close. */
  refuse(code: string, key?: string, i = this.live.length - 1): void {
    const stream = this.live[i]!;
    stream.push(sse('error', { error: { code, message: `${code} at the door` }, ...(key ? { key } : {}) }));
    stream.close();
  }

  /** Signal every live read of `op` on shared stream `i` (default: the newest). */
  invalidate(op: string, i = this.live.length - 1, times = 1): void {
    const stream = this.live[i]!;
    const keys = this.reads.filter((r) => r.stream === stream.id && r.op === op && !r.dropped).map((r) => r.key);
    for (let n = 0; n < times; n++) for (const key of keys) stream.push(sse('invalidate', { key }));
  }

  /** Reads of `op` currently on the newest stream. */
  readsOf(op: string): LiveRead[] {
    const id = this.live[this.live.length - 1]?.id;
    return this.reads.filter((r) => r.stream === id && r.op === op && !r.dropped);
  }

  readonly fetch: typeof globalThis.fetch = async (input, init) => {
    const url = String(input);
    if (this.offline) throw new TypeError('Failed to fetch');
    const parsed = new URL(url, 'http://door.test');
    const method = init?.method ?? 'GET';
    const headers = (init?.headers ?? {}) as Record<string, string>;
    const json = (status: number, body: unknown) =>
      new Response(body === null ? null : JSON.stringify(body), {
        status,
        headers: { 'Content-Type': 'application/json' },
      });
    const refusal = (status: number, code: string) => json(status, { error: { code, message: `${code} at the door` } });

    if (parsed.pathname === '/live' && method === 'GET') {
      const r = this.#streamRefusals.shift();
      if (r) return refusal(r.status, r.code);
      const stream = new Stream(url, init?.signal, `s${this.live.length + 1}`);
      stream.cache = init?.cache;
      this.live.push(stream);
      this.liveHeaders.push(headers);
      return new Response(stream.body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
    }
    const add = /^\/live\/([^/]+)$/.exec(parsed.pathname);
    if (add && method === 'POST') {
      const id = decodeURIComponent(add[1]!);
      const body = JSON.parse(String(init?.body)) as {
        key: string;
        op: string;
        params: Record<string, string>;
        refine?: unknown;
      };
      this.adds.push({ stream: id, key: body.key, op: body.op });
      const open = this.live.find((s) => s.id === id && !s.done);
      if (!open || this.#forgotten.has(id)) return refusal(404, 'not_found');
      const at = this.#readRefusals.findIndex((r) => r.op === body.op);
      if (at >= 0) {
        const [r] = this.#readRefusals.splice(at, 1);
        return refusal(r!.status, r!.code);
      }
      this.reads.push({
        stream: id,
        key: body.key,
        op: body.op,
        params: body.params,
        ...(body.refine === undefined ? {} : { refine: body.refine }),
        headers,
        dropped: false,
      });
      return json(200, { op: body.op, rows: [] });
    }
    const drop = /^\/live\/([^/]+)\/([^/]+)$/.exec(parsed.pathname);
    if (drop && method === 'DELETE') {
      const id = decodeURIComponent(drop[1]!);
      const key = decodeURIComponent(drop[2]!);
      for (const r of this.reads) if (r.stream === id && r.key === key) r.dropped = true;
      return new Response(null, { status: 204 });
    }
    if (parsed.searchParams.get('live') === '1') {
      const r = this.#streamRefusals.shift();
      if (r) return refusal(r.status, r.code);
      const stream = new Stream(url, init?.signal);
      stream.cache = init?.cache;
      (parsed.searchParams.get('sync') === '1' ? this.sync : this.single).push(stream);
      return new Response(stream.body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
    }
    this.calls.push({
      url,
      method,
      body: typeof init?.body === 'string' ? (JSON.parse(init.body) as unknown) : undefined,
    });
    if (this.answer) return json(200, this.answer(parsed));
    const next = this.#replies.length > 1 ? this.#replies.shift()! : (this.#replies[0] ?? { body: { rows: [] } });
    return json(next.status ?? 200, next.body);
  };

  client(extra: Partial<DescriptorClientOptions> = {}): DescriptorClient {
    let n = 0;
    return new DescriptorClient({
      baseUrl: 'http://door.test',
      iface: iface(),
      space: 'spc_test',
      token: 'tok',
      fetch: this.fetch,
      newIdempotencyKey: () => `idem-${n++}`,
      ...extra,
    });
  }
}

export async function settle(ms = 0): Promise<void> {
  await act(async () => {
    await new Promise((r) => setTimeout(r, ms));
  });
}

/** Wait until `check` holds, flushing React between polls. */
export async function until(check: () => boolean, what = 'condition'): Promise<void> {
  for (let i = 0; i < 200; i++) {
    if (check()) return;
    await settle(0);
  }
  throw new Error(`timed out waiting for ${what}`);
}

export interface Mounted<T> {
  readonly result: { current: T };
  readonly unmount: () => void;
  /** Render again with the same props, as a parent's state change would. */
  readonly rerender: () => Promise<void>;
}

/** Render `hook` under an `OrdoProvider` and hand back its latest value. */
export async function mount<T>(hook: () => T, props: Omit<OrdoProviderProps, 'children'>): Promise<Mounted<T>> {
  const result = { current: undefined as unknown as T };
  function Probe(): ReactNode {
    result.current = hook();
    return null;
  }
  let renderer!: ReactTestRenderer;
  await act(async () => {
    renderer = create(createElement(OrdoProvider, props, createElement(Probe)));
  });
  return {
    result,
    unmount: () => {
      void act(() => renderer.unmount());
    },
    rerender: async () => {
      await act(async () => {
        renderer.update(createElement(OrdoProvider, props, createElement(Probe)));
      });
    },
  };
}
