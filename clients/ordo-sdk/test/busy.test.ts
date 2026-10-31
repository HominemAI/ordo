/**
 * A busy deployment answers `503 server_busy` with `Retry-After`. The client
 * waits and sends the call again, `busyRetry.retries` times at most (default
 * 2), with a wait that grows by a factor up to a ceiling.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

import { OrdoClient, OrdoRefusal } from '../src/index.js';
import type { BusyRetryOptions } from '../src/index.js';
import { busyDelayMs, busyRetryOf, DEFAULT_BUSY_RETRY } from '../src/core/busy.js';
import type { OpSpec } from '../src/index.js';
import { MockDoor } from './harness.js';

const read: OpSpec = {
  op: 'activeProducts',
  version: '0123456789abcdef',
  method: 'GET',
  kind: 'read',
  terminal: ['fetch'],
  paged: false,
  singleRow: false,
  casParam: null,
  params: [],
  refusals: [],
  standingError: null,
  sync: null,
  refine: { tier: 'none', fields: [] },
  order: { keys: [], stated: false },
} as unknown as OpSpec;

const write: OpSpec = { ...read, op: 'addProduct', method: 'POST', kind: 'write' };

const busy = (retryAfter = '0') => ({
  status: 503,
  json: { error: { code: 'server_busy', message: 'every connection is in use' } },
  headers: { 'Retry-After': retryAfter },
});
const ok = { json: { rows: [{ id: 'p1' }] } };

// Real-timer tests wait nothing: the first delay is 0 unless a test says so.
function clientOf(
  door: MockDoor,
  extra: { busyRetries?: number; busyRetry?: BusyRetryOptions; newIdempotencyKey?: () => string } = {},
) {
  return new OrdoClient({
    baseUrl: 'https://door.example/api',
    space: 'spc_test',
    token: 'tok',
    fetch: door.fetch,
    ...extra,
    busyRetry: { firstDelayMs: 0, ...extra.busyRetry },
  });
}

afterEach(() => vi.useRealTimers());

describe('retrying server_busy', () => {
  it('says before each wait that it is retrying (L7)', async () => {
    const door = new MockDoor().reply(busy(), busy(), ok);
    const told: number[] = [];
    const res = await clientOf(door).request(read, {}, { onRetry: (n) => told.push(n) });
    expect(res.rows).toEqual([{ id: 'p1' }]);
    expect(told).toEqual([1, 2]);
  });

  it('a busy answer then success returns the success', async () => {
    const door = new MockDoor().reply(busy(), ok);
    const res = await clientOf(door).request(read, {});
    expect(res.rows).toEqual([{ id: 'p1' }]);
    expect(door.calls).toHaveLength(2);
  });

  it('retries stop at the limit and the busy refusal surfaces', async () => {
    const door = new MockDoor().reply(busy());
    await expect(clientOf(door).request(read, {})).rejects.toBeInstanceOf(OrdoRefusal);
    // The first attempt and the default 2 retries.
    expect(door.calls).toHaveLength(3);

    const one = new MockDoor().reply(busy());
    await expect(clientOf(one, { busyRetries: 1 }).request(read, {})).rejects.toBeInstanceOf(OrdoRefusal);
    expect(one.calls).toHaveLength(2);
  });

  it('busyRetries 0 means no retry', async () => {
    const door = new MockDoor().reply(busy(), ok);
    await expect(clientOf(door, { busyRetries: 0 }).request(read, {})).rejects.toBeInstanceOf(OrdoRefusal);
    expect(door.calls).toHaveLength(1);
  });

  it('another 503 class is not retried', async () => {
    for (const code of ['space_unavailable', 'statement_timeout']) {
      const door = new MockDoor().reply(
        { status: 503, json: { error: { code, message: 'no' } }, headers: { 'Retry-After': '0' } },
        ok,
      );
      await expect(clientOf(door).request(read, {})).rejects.toBeInstanceOf(OrdoRefusal);
      expect(door.calls, code).toHaveLength(1);
    }
  });

  it('a retried write carries the same idempotency key', async () => {
    let n = 0;
    const door = new MockDoor().reply(busy(), ok);
    await clientOf(door, { newIdempotencyKey: () => `key-${n++}` }).request(write, {});
    expect(door.calls).toHaveLength(2);
    expect(door.calls.map((c) => c.headers['Idempotency-Key'])).toEqual(['key-0', 'key-0']);
  });

  it('waits what Retry-After says, and never more than the ceiling', async () => {
    vi.useFakeTimers();
    const door = new MockDoor().reply(busy('3'), busy('120'), ok);
    const done = clientOf(door, { busyRetry: { jitter: false } }).request(read, {});
    await vi.advanceTimersByTimeAsync(2_999);
    expect(door.calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(door.calls).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(9_999);
    expect(door.calls).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect((await done).rows).toEqual([{ id: 'p1' }]);
    expect(door.calls).toHaveLength(3);
  });

  it('a value that is not a whole number of retries is refused', () => {
    const door = new MockDoor();
    expect(() => clientOf(door, { busyRetries: -1 })).toThrow(TypeError);
    expect(() => clientOf(door, { busyRetries: 1.5 })).toThrow(TypeError);
  });

  it('delays grow by the factor and stop at the ceiling', () => {
    const retry = busyRetryOf({ firstDelayMs: 100, factor: 3, maxDelayMs: 1_000, jitter: false });
    const waits = [0, 1, 2, 3, 4].map((n) => busyDelayMs(retry, n, null));
    expect(waits).toEqual([100, 300, 900, 1_000, 1_000]);
  });

  it('a Retry-After larger than the computed delay wins', () => {
    const retry = busyRetryOf({ firstDelayMs: 100, factor: 2, maxDelayMs: 10_000, jitter: false });
    expect(busyDelayMs(retry, 0, 3_000)).toBe(3_000);
    // A smaller one does not shorten the computed delay.
    expect(busyDelayMs(retry, 2, 150)).toBe(400);
  });

  it('with jitter off the delays are exact, and with it on they add at most 20%', () => {
    const off = busyRetryOf({ firstDelayMs: 100, jitter: false });
    expect(busyDelayMs(off, 1, null, () => 0.99)).toBe(200);
    const on = busyRetryOf({ firstDelayMs: 100 });
    expect(on.jitter).toBe(true);
    expect(busyDelayMs(on, 1, null, () => 0)).toBe(200);
    expect(busyDelayMs(on, 1, null, () => 0.5)).toBeCloseTo(220);
    expect(busyDelayMs(on, 1, null, () => 0.999999)).toBeLessThanOrEqual(240);
  });

  it('the defaults are the documented ones', () => {
    expect(busyRetryOf(undefined)).toEqual({
      retries: 2,
      firstDelayMs: 500,
      factor: 2,
      maxDelayMs: 10_000,
      jitter: true,
    });
    expect(DEFAULT_BUSY_RETRY.retries).toBe(2);
    expect(busyRetryOf(undefined, 5).retries).toBe(5);
    expect(busyRetryOf({ retries: 4 }, 4).retries).toBe(4);
  });

  it('bad options throw TypeError', () => {
    const bads: [BusyRetryOptions | null | string, number?][] = [
      [{ retries: -1 }],
      [{ retries: 1.5 }],
      [{ firstDelayMs: -1 }],
      [{ firstDelayMs: Number.NaN }],
      [{ factor: 0.5 }],
      [{ factor: Number.POSITIVE_INFINITY }],
      [{ maxDelayMs: -5 }],
      [{ jitter: 'yes' as unknown as boolean }],
      [{ retreis: 3 } as unknown as BusyRetryOptions],
      [{ retries: 1 }, 2],
      ['often'],
      [null],
    ];
    for (const [opts, shorthand] of bads) {
      expect(() => busyRetryOf(opts as BusyRetryOptions, shorthand), JSON.stringify(opts)).toThrow(TypeError);
    }
  });

  it('delays grow across real retries', async () => {
    vi.useFakeTimers();
    const door = new MockDoor().reply(busy(), busy(), busy(), ok);
    const done = clientOf(door, {
      busyRetry: { retries: 3, firstDelayMs: 100, factor: 3, jitter: false },
    }).request(read, {});
    await vi.advanceTimersByTimeAsync(99);
    expect(door.calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(door.calls).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(299);
    expect(door.calls).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(door.calls).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(899);
    expect(door.calls).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(1);
    expect((await done).rows).toEqual([{ id: 'p1' }]);
  });

  it('an exhausted retry surfaces server_busy, not internal', async () => {
    const door = new MockDoor().reply(busy());
    const err = await clientOf(door).request(read, {}).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoRefusal);
    expect((err as OrdoRefusal).detail.class).toBe('server_busy');
  });
});

describe('retrying tenant_not_ready (owner, 2026-10-21)', () => {
  const notReady = (retryAfter = '0') => ({
    status: 503,
    json: { error: { code: 'tenant_not_ready', message: 'your tenant is still being set up' } },
    headers: { 'Retry-After': retryAfter },
  });

  it('a write into a tenant still being set up is sent again with the same key, and lands', async () => {
    const door = new MockDoor().reply(notReady(), notReady(), ok);
    const reasons: string[] = [];
    const res = await clientOf(door).request(write, {}, { onRetry: (_n, _d, why) => reasons.push(why) });
    expect(res.rows).toEqual([{ id: 'p1' }]);
    expect(reasons).toEqual(['tenant_not_ready', 'tenant_not_ready']);
    const keys = door.calls.map((c) => c.headers['Idempotency-Key']);
    expect(new Set(keys).size).toBe(1);
  });

  it('when the retries run out the refusal keeps its class', async () => {
    const door = new MockDoor().reply(notReady());
    const err = await clientOf(door).request(read, {}).catch((e) => e);
    expect(err).toBeInstanceOf(OrdoRefusal);
    expect((err as OrdoRefusal).detail.class).toBe('tenant_not_ready');
    expect(door.calls).toHaveLength(3);
  });
});
