/**
 * Live: one shared stream per client, and what a hook does with it.
 *
 * `ui-gen.md` §2 settles the *meaning*: invalidation is a staleness signal,
 * never a data push, and the answer is a refetch through the op under the
 * caller's own scope.
 *
 * This file is pure. `LiveHub` owns the connection, `createPacer` owns the
 * pacing, and `hooks/useLive.ts` is the React adapter over both.
 *
 * ## One stream per client (checklist 27s)
 *
 * The owner, 2026-10-01: a stream per entity *"seems like it would be
 * pricey among tons of clients"*. So the hub opens one stream, `GET /live`
 * (`api-engine-design.md` §6.1), and every hook shares it:
 *
 * - the stream's first frame, `ready`, names it;
 * - each distinct (op, params, refinement) is one read on it, added with
 *   `POST /live/{stream}` under a key of the hub's own (`r1`, `r2`, ...).
 *   The refinement rides with the read, so the door checks it as it would a
 *   one-shot read (R6). Signals are still per op: two refinements of one op
 *   hear the same writes;
 * - each `invalidate` frame names the key it is for;
 * - the last hook on a read leaving drops it, `DELETE /live/{stream}/{key}`,
 *   and the last read leaving closes the stream.
 *
 * The door checks each read as if it were called alone. A read it refuses
 * is refused for that read's hooks only; the stream and its other reads go on.
 *
 * ## The first answer
 *
 * The door subscribes a read to its bus *before* it answers the `POST`, so
 * nothing committed after that answer can be missed. A hook that fetched
 * before its read was added could miss a change made between its fetch and
 * the subscription. So a hook fetches at once, beside the subscribe, and
 * fetches once more when its read opens if the first fetch started before
 * that (a stream that never opens must not hide rows). The rows in the
 * `POST`'s answer are not used: the fetch goes through the op, which is what
 * carries paging, refinement and the version pin. A **re**connect is
 * different. Signals may have been missed while the stream was down, so the
 * hub reports a read opened again as an invalidation.
 */

import type { DescriptorClient, Params } from './client.js';
import type { Refine } from './refine.js';
import { OrdoRefusal, decodeRefusal, type Refusal } from './refusal.js';
import { OrdoNetworkError } from '@hominemai/ordo-sdk/core/network.js';
import { frames } from './sse.js';
import { RECONNECT_CLASSES, reconnectDelayMs } from '@hominemai/ordo-sdk/core/busy.js';

export type LiveStatus =
  | { readonly state: 'connecting' }
  | { readonly state: 'open' }
  /** The stream ended or failed; the hub is waiting to open it again. */
  | { readonly state: 'reconnecting' }
  /** The op is not live-capable here. Reads still work; they just do not follow. */
  | { readonly state: 'unsupported'; readonly refusal: Refusal | null }
  /** The door refused the subscription for good. */
  | { readonly state: 'failed'; readonly refusal: Refusal };

export interface LiveSubscriber {
  readonly invalidate: () => void;
  readonly status: (status: LiveStatus) => void;
}

export interface LiveHubOptions {
  /** Milliseconds to wait before reconnect number `attempts`. */
  readonly backoff?: (attempts: number) => number;
  /** How the hub waits. Replaced in tests. */
  readonly wait?: (ms: number, signal: AbortSignal) => Promise<void>;
  /**
   * Milliseconds a stream may stay `connecting`, from the request to its
   * `ready` frame. Past it the stream is treated as failed: reads move to
   * `reconnecting` and it is opened again on the backoff. A read shows its
   * rows meanwhile, because a hook does not wait for the stream. Unset,
   * `DEFAULT_LIVE_CONNECT_MS`: 3 seconds. Set it through the provider:
   * `live={{ connectMs }}`.
   */
  readonly connectMs?: number;
}

/** The shipped deadline on a stream that has not said `ready`: 3 seconds. */
export const DEFAULT_LIVE_CONNECT_MS = 3000;

/**
 * Refusals worth another try; every other one is a standing answer. The
 * SDK's list, the one its sync manager reconnects on: `internal` and
 * `RETRY_CLASSES` (`server_busy`, `tenant_not_ready`,
 * `serialization_conflict`).
 */
const RETRYABLE: ReadonlySet<string> = RECONNECT_CLASSES;

/** 0, then 250 ms doubling to 30 s: the SDK's one reconnect backoff. */
function defaultBackoff(attempts: number): number {
  return reconnectDelayMs(attempts);
}

/** The door's own `Retry-After` on a refusal, a floor under the backoff. */
function askedOf(error: unknown): number {
  return error instanceof OrdoRefusal ? (error.retryAfterMs ?? 0) : 0;
}

function defaultWait(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) return resolve();
    const timer = setTimeout(resolve, ms);
    signal.addEventListener(
      'abort',
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
  });
}

/** One read on the shared stream: a distinct (op, params, refinement). */
interface Entry {
  readonly key: string;
  /** The key the door tags this read's signals with. */
  readonly wire: string;
  readonly op: string;
  readonly params: Params;
  readonly refine: Refine | undefined;
  readonly subscribers: Set<LiveSubscriber>;
  status: LiveStatus;
  /** The stream the door accepted this read on, if any. */
  on: string | null;
  /** Opened on some stream before; opening again is an invalidation. */
  opened: boolean;
  /** Refused for good. Not added again while its hooks stay. */
  settled: boolean;
  /** A pending retry of the add, cancelled when the read or stream goes. */
  retry: AbortController | null;
}

/** The one connection, while any read wants it. */
interface Connection {
  readonly controller: AbortController;
  /** The open stream's id, from its `ready` frame. */
  stream: string | null;
  /**
   * The token was renewed after a refusal and nothing has been accepted
   * since. A second refusal then stands, so a door that refuses every token
   * is not asked in a loop (the SDK sync manager's rule).
   */
  renewed: boolean;
}

export class LiveHub {
  readonly #client: DescriptorClient;
  readonly #backoff: (attempts: number) => number;
  readonly #wait: (ms: number, signal: AbortSignal) => Promise<void>;
  readonly #connectMs: number;
  readonly #entries = new Map<string, Entry>();
  readonly #byWire = new Map<string, Entry>();
  #seq = 0;
  #conn: Connection | null = null;

  constructor(client: DescriptorClient, options: LiveHubOptions = {}) {
    this.#client = client;
    this.#backoff = options.backoff ?? defaultBackoff;
    this.#wait = options.wait ?? defaultWait;
    this.#connectMs = options.connectMs ?? DEFAULT_LIVE_CONNECT_MS;
  }

  /** Reads currently on the stream or waiting for it. For tests and diagnostics. */
  get size(): number {
    return this.#entries.size;
  }

  /** Whether the shared stream is open or opening. For tests and diagnostics. */
  get connected(): boolean {
    return this.#conn !== null;
  }

  subscribe(op: string, params: Params, subscriber: LiveSubscriber, refine?: Refine): () => void {
    const key = keyOf(op, params, refine);
    let entry = this.#entries.get(key);
    if (!entry) {
      this.#seq += 1;
      entry = {
        key,
        wire: `r${this.#seq}`,
        op,
        params,
        refine,
        subscribers: new Set(),
        status: { state: 'connecting' },
        on: null,
        opened: false,
        settled: false,
        retry: null,
      };
      this.#entries.set(key, entry);
      this.#byWire.set(entry.wire, entry);
      const conn = this.#connection();
      if (conn.stream !== null) void this.#add(entry, conn, conn.stream, 0);
    }
    entry.subscribers.add(subscriber);
    subscriber.status(entry.status);
    const joined = entry;
    return () => {
      joined.subscribers.delete(subscriber);
      if (joined.subscribers.size > 0) return;
      this.#leave(joined);
    };
  }

  /** The last hook on a read left: drop it at the door, and close an empty stream. */
  #leave(entry: Entry): void {
    this.#entries.delete(entry.key);
    this.#byWire.delete(entry.wire);
    entry.retry?.abort();
    const conn = this.#conn;
    if (!conn) return;
    if (this.#entries.size === 0) {
      // Aborting closes the stream at the door, and every read on it.
      this.#conn = null;
      conn.controller.abort();
      return;
    }
    if (entry.on !== null && entry.on === conn.stream) this.#drop(conn.stream, entry.wire);
  }

  #drop(stream: string, wire: string): void {
    void this.#client.dropLiveRead(stream, wire).catch(() => undefined);
  }

  #connection(attempts = 0): Connection {
    if (this.#conn) return this.#conn;
    const conn: Connection = { controller: new AbortController(), stream: null, renewed: false };
    this.#conn = conn;
    void this.#run(conn, attempts);
    return conn;
  }

  /**
   * Drop `conn` and open a fresh stream for the reads that remain. Counted
   * as one failed attempt, so a door that refuses every add on every new
   * stream is retried on the backoff and not in a tight loop.
   */
  #reopen(conn: Connection): void {
    if (this.#conn !== conn) return;
    this.#conn = null;
    conn.controller.abort();
    for (const entry of this.#entries.values()) {
      if (entry.settled) continue;
      entry.on = null;
      entry.retry?.abort();
      entry.retry = null;
      this.#set(entry, { state: 'reconnecting' });
    }
    if (this.#entries.size > 0) this.#connection(1);
  }

  #set(entry: Entry, status: LiveStatus): void {
    entry.status = status;
    for (const s of [...entry.subscribers]) s.status(status);
  }

  #invalidate(entry: Entry): void {
    for (const s of [...entry.subscribers]) s.invalidate();
  }

  /** Whether an add begun for `entry` on `stream` still matters. */
  #current(entry: Entry, conn: Connection, stream: string): boolean {
    return (
      this.#entries.get(entry.key) === entry &&
      this.#conn === conn &&
      conn.stream === stream &&
      !conn.controller.signal.aborted
    );
  }

  async #add(entry: Entry, conn: Connection, stream: string, attempt: number): Promise<void> {
    try {
      await this.#client.addLiveRead(stream, entry.wire, entry.op, {
        params: entry.params,
        ...(entry.refine ? { refine: entry.refine } : {}),
        signal: conn.controller.signal,
      });
    } catch (error) {
      if (!this.#current(entry, conn, stream)) return;
      // The door no longer answers for this stream: it closed, or it was
      // opened under a token this client has since replaced (a stream
      // belongs to the token that opened it). Either way the feed may still
      // be open on our side, so waiting for it to end would wait forever.
      // Close it here; the loop reopens one under the current token and
      // adds every read again.
      if (error instanceof OrdoRefusal && error.refusal.class === 'not_found') {
        this.#reopen(conn);
        return;
      }
      if (error instanceof OrdoRefusal && !RETRYABLE.has(error.refusal.class)) {
        const refusal = error.refusal;
        entry.settled = true;
        this.#set(
          entry,
          refusal.class === 'not_live' || refusal.class === 'method_mismatch'
            ? { state: 'unsupported', refusal }
            : { state: 'failed', refusal },
        );
        return;
      }
      // Worth another try on the same stream.
      this.#set(entry, { state: 'reconnecting' });
      const retry = new AbortController();
      entry.retry = retry;
      await this.#wait(Math.max(this.#backoff(attempt + 1), askedOf(error)), retry.signal);
      if (retry.signal.aborted || !this.#current(entry, conn, stream)) return;
      entry.retry = null;
      return this.#add(entry, conn, stream, attempt + 1);
    }
    if (!this.#current(entry, conn, stream)) {
      // The hooks left while the add was in flight; take it back off.
      if (this.#conn === conn && conn.stream === stream && this.#entries.get(entry.key) !== entry) {
        this.#drop(stream, entry.wire);
      }
      return;
    }
    entry.on = stream;
    conn.renewed = false;
    this.#set(entry, { state: 'open' });
    // A read opened again may have missed signals while it was off.
    if (entry.opened) this.#invalidate(entry);
    entry.opened = true;
  }

  async #run(conn: Connection, attempts = 0): Promise<void> {
    const { signal } = conn.controller;
    if (attempts > 0) await this.#wait(this.#backoff(attempts), signal);
    while (!signal.aborted) {
      let asked = 0;
      let skipWait = false;
      // One controller per try, so the deadline can end this try alone.
      const attempt = new AbortController();
      const follow = () => attempt.abort();
      signal.addEventListener('abort', follow, { once: true });
      const deadline = setTimeout(() => attempt.abort(), this.#connectMs);
      try {
        const response = await this.#client.openLiveStream({ signal: attempt.signal });
        let ready = false;
        let last: OrdoRefusal | null = null;
        let lastKey: string | undefined;
        for await (const frame of frames(response)) {
          if (signal.aborted) return;
          if (frame.event === 'ready') {
            const id = (frame.data as { stream?: unknown } | null)?.stream;
            if (typeof id !== 'string') continue;
            ready = true;
            clearTimeout(deadline);
            attempts = 0;
            conn.stream = id;
            for (const entry of [...this.#entries.values()]) {
              if (!entry.settled) void this.#add(entry, conn, id, 0);
            }
            continue;
          }
          if (frame.event === 'error') {
            // The door re-checked the feed, refused, and closes it. The
            // frame is the last; `key` names the read that was refused.
            last = new OrdoRefusal(decodeRefusal(frame.data, 500, '', []));
            const key = (frame.data as { key?: unknown } | null)?.key;
            lastKey = typeof key === 'string' ? key : undefined;
            break;
          }
          if (frame.event === 'invalidate') {
            conn.renewed = false;
            const wire = (frame.data as { key?: unknown } | null)?.key;
            const entry = typeof wire === 'string' ? this.#byWire.get(wire) : undefined;
            if (entry && entry.on === conn.stream) this.#invalidate(entry);
          }
        }
        // The door closed the stream. One that never said `ready` counts as
        // a failure, or a door that closes every stream at once would be
        // reopened in a tight loop.
        attempts = ready ? 0 : attempts + 1;
        if (last) {
          // Same handling as a refusal at the door.
          const verdict = await this.#refused(conn, last, lastKey);
          if (verdict === 'stop') return;
          if (verdict === 'again') {
            attempts = 0;
            skipWait = true;
          } else {
            attempts += 1;
          }
        }
      } catch (error) {
        if (signal.aborted) return;
        if (error instanceof OrdoRefusal && !RETRYABLE.has(error.refusal.class)) {
          const verdict = await this.#refused(conn, error, undefined);
          if (verdict === 'stop') return;
          if (verdict === 'again') {
            attempts = 0;
            skipWait = true;
          } else {
            attempts += 1;
          }
        } else {
          asked = askedOf(error);
          attempts += 1;
        }
      } finally {
        clearTimeout(deadline);
        signal.removeEventListener('abort', follow);
      }
      if (signal.aborted) return;
      conn.stream = null;
      for (const entry of this.#entries.values()) {
        if (entry.settled) continue;
        entry.on = null;
        entry.retry?.abort();
        entry.retry = null;
        this.#set(entry, { state: 'reconnecting' });
      }
      // A new token reconnects at once.
      if (!skipWait) await this.#wait(Math.max(this.#backoff(attempts), asked), signal);
    }
  }

  /**
   * The door refused the feed, at the open or in a last `error` frame. An
   * `access_denied` may only mean a stale token (a guest's window token names
   * one grant, and a second accept replaces it), so the client is asked once
   * for a new one, the SDK's `renewToken`. Answers:
   *
   * - `again`: renewed, reconnect at once;
   * - `stop`: the refusal stands. The refused read (the frame's `key`), or
   *   every read when it names none, shows `failed` with the typed refusal
   *   and is not added again. Stops when no read is left to follow;
   * - `wait`: the door could not be asked about a new token, or other reads
   *   remain; reconnect on the backoff.
   */
  async #refused(
    conn: Connection,
    error: OrdoRefusal,
    wire: string | undefined,
  ): Promise<'again' | 'stop' | 'wait'> {
    const signal = conn.controller.signal;
    if (RETRYABLE.has(error.refusal.class)) return 'wait';
    if (error.refusal.class === 'access_denied' && !conn.renewed) {
      let renewed = false;
      try {
        renewed = await this.#client.renewToken();
      } catch (e) {
        if (e instanceof OrdoNetworkError) return signal.aborted ? 'stop' : 'wait';
      }
      if (signal.aborted) return 'stop';
      if (renewed) {
        conn.renewed = true;
        return 'again';
      }
    }
    const refusal = error.refusal;
    const keyed = wire === undefined ? undefined : this.#byWire.get(wire);
    for (const entry of this.#entries.values()) {
      if (entry.settled || (keyed && entry !== keyed)) continue;
      entry.settled = true;
      this.#set(entry, { state: 'failed', refusal });
    }
    if ([...this.#entries.values()].every((e) => e.settled)) {
      if (this.#conn === conn) this.#conn = null;
      conn.controller.abort();
      return 'stop';
    }
    // Other reads are unaffected: they reconnect without the refused one.
    return 'wait';
  }
}

function keyOf(op: string, params: Params, refine: Refine | undefined): string {
  const names = Object.keys(params).sort();
  const base = `${op}?${names.map((n) => `${n}=${String(params[n])}`).join('&')}`;
  return refine && (refine.where || refine.pick) ? `${base}#${JSON.stringify(refine)}` : base;
}

// ── pacing ───────────────────────────────────────────────────────────────

/**
 * The shipped minimum gap between live refetches, in milliseconds: a quarter
 * second. Ruled 2026-10-01 (checklist 27n, board `react-live-device`):
 * *group bursts and wait at least a quarter second between refetches, as a
 * setting.* The provider's `live.paceMs` sets it for an app; a hook's
 * `livePaceMs` may lengthen it for itself. This constant is the one place
 * the number lives.
 */
export const DEFAULT_LIVE_PACE_MS = 250;

/**
 * The gap one hook refetches at: the provider's pace is a floor, and a hook
 * may wait longer, never less (`plan-syntax-additions.md` §9b: *a hook may
 * slow down, never speed up past the floor*). A hook that asks for nothing
 * gets the floor.
 */
export function effectivePaceMs(floorMs: number, hookMs: number | undefined): number {
  return hookMs === undefined ? floorMs : Math.max(floorMs, hookMs);
}

export interface PacerTimers {
  readonly now: () => number;
  readonly set: (fn: () => void, ms: number) => unknown;
  readonly clear: (handle: unknown) => void;
}

const REAL_TIMERS: PacerTimers = {
  now: () => Date.now(),
  set: (fn, ms) => setTimeout(fn, ms),
  clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
};

export interface Pacer {
  /** Ask for a fire. Asks made while one is pending join it. */
  readonly request: () => void;
  readonly cancel: () => void;
}

/**
 * Coalesce a burst of invalidations into one refetch, and space refetches at
 * least `minIntervalMs` apart.
 *
 * The door already coalesces (a lagged subscriber gets one `invalidate`), but
 * it cannot know how expensive this client's refetch is. A table that is
 * paged through and refined can cost far more than the signal did. Pacing is
 * trailing-edge: the fire happens once the interval since the last one has
 * passed, and a request that arrives while one is pending is absorbed. So a
 * burst of invalidations is one refetch, and a stream of them is one refetch
 * per `minIntervalMs`.
 *
 * `minIntervalMs` is the setting (`DEFAULT_LIVE_PACE_MS` when the app sets
 * nothing); this function never supplies a number of its own.
 */
export function createPacer(fire: () => void, minIntervalMs: number, timers: PacerTimers = REAL_TIMERS): Pacer {
  let pending: unknown;
  let last = -Infinity;
  return {
    request() {
      if (pending !== undefined) return;
      const wait = Math.max(0, last + minIntervalMs - timers.now());
      pending = timers.set(() => {
        pending = undefined;
        last = timers.now();
        fire();
      }, wait);
    },
    cancel() {
      if (pending !== undefined) timers.clear(pending);
      pending = undefined;
    },
  };
}
