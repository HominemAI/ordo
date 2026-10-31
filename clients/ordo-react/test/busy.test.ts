import { afterEach, describe, expect, it, vi } from 'vitest';
import { OrdoRefusal } from '../src/refusal.js';
import { busyDelayMs, busyRetryOf, DEFAULT_BUSY_RETRY } from '../src/busy.js';
import type { BusyRetryOptions } from '../src/busy.js';
import { MockDoor, client as plainClient, refusal } from './harness.js';

// Real-timer tests wait nothing: the first delay is 0 unless a test says so.
const client = (door: MockDoor, o: { busyRetries?: number; busyRetry?: BusyRetryOptions } = {}) =>
  plainClient(door, { ...o, busyRetry: { firstDelayMs: 0, ...o.busyRetry } });

const busy = { status: 503, body: refusal('server_busy', 'every connection is in use'), headers: { 'Retry-After': '0' } };
const ok = { body: { rows: [{ id: 'task_1' }] } };
const get = { params: { id: 'task_1' } };

afterEach(() => vi.useRealTimers());

describe('retrying server_busy', () => {
  it('a busy answer then success returns the success', async () => {
    const door = new MockDoor().program(busy, ok);
    const res = await client(door).fetchOp('std.tasks.get', get);
    expect(res.rows).toHaveLength(1);
    expect(door.requests).toHaveLength(2);
  });

  it('says before each wait that it is retrying (L7)', async () => {
    const door = new MockDoor().program(busy, busy, ok);
    const told: number[] = [];
    const res = await client(door).fetchOp('std.tasks.get', { ...get, onRetry: (n) => told.push(n) });
    expect(res.rows).toHaveLength(1);
    expect(told).toEqual([1, 2]);
  });

  it('retries stop at the limit and the busy refusal surfaces', async () => {
    const door = new MockDoor().always(busy);
    await expect(client(door).fetchOp('std.tasks.get', get)).rejects.toBeInstanceOf(OrdoRefusal);
    // The first attempt and the default 2 retries.
    expect(door.requests).toHaveLength(3);

    const one = new MockDoor().always(busy);
    await expect(client(one, { busyRetries: 1 }).fetchOp('std.tasks.get', get)).rejects.toBeInstanceOf(
      OrdoRefusal,
    );
    expect(one.requests).toHaveLength(2);
  });

  it('busyRetries 0 means no retry', async () => {
    const door = new MockDoor().program(busy, ok);
    await expect(client(door, { busyRetries: 0 }).fetchOp('std.tasks.get', get)).rejects.toBeInstanceOf(
      OrdoRefusal,
    );
    expect(door.requests).toHaveLength(1);
  });

  it('another 503 class is not retried', async () => {
    const door = new MockDoor().program(
      { status: 503, body: refusal('space_unavailable'), headers: { 'Retry-After': '0' } },
      ok,
    );
    await expect(client(door).fetchOp('std.tasks.get', get)).rejects.toBeInstanceOf(OrdoRefusal);
    expect(door.requests).toHaveLength(1);
  });

  it('a retried write carries the same idempotency key', async () => {
    const door = new MockDoor().program(busy, { body: { op: 'std.tasks.update', rows: [{ id: 'task_1', rev: 4 }] } });
    await client(door).commit('std.tasks.update', {
      params: { id: 'task_1', rev: 3, project: 'proj_1', assignee: 'mem_1', title: 'Ship', status: 'open' },
    });
    expect(door.requests).toHaveLength(2);
    expect(door.requests.map((r) => r.headers['Idempotency-Key'])).toEqual(['idem-0', 'idem-0']);
  });

  it('waits what Retry-After says, capped at the ceiling', async () => {
    vi.useFakeTimers();
    const door = new MockDoor().program({ ...busy, headers: { 'Retry-After': '120' } }, ok);
    const done = client(door, { busyRetry: { jitter: false } }).fetchOp('std.tasks.get', get);
    await vi.advanceTimersByTimeAsync(9_999);
    expect(door.requests).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect((await done).rows).toHaveLength(1);
  });

  it('delays grow by the factor and stop at the ceiling', () => {
    const retry = busyRetryOf({ firstDelayMs: 100, factor: 3, maxDelayMs: 1_000, jitter: false });
    expect([0, 1, 2, 3, 4].map((n) => busyDelayMs(retry, n, null))).toEqual([100, 300, 900, 1_000, 1_000]);
  });

  it('a Retry-After larger than the computed delay wins', () => {
    const retry = busyRetryOf({ firstDelayMs: 100, factor: 2, maxDelayMs: 10_000, jitter: false });
    expect(busyDelayMs(retry, 0, 3_000)).toBe(3_000);
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
  });

  it('bad options throw TypeError', () => {
    const bads: [unknown, number?][] = [
      [{ retries: -1 }],
      [{ retries: 1.5 }],
      [{ firstDelayMs: -1 }],
      [{ firstDelayMs: Number.NaN }],
      [{ factor: 0.5 }],
      [{ factor: Number.POSITIVE_INFINITY }],
      [{ maxDelayMs: -5 }],
      [{ jitter: 'yes' }],
      [{ retreis: 3 }],
      [{ retries: 1 }, 2],
      ['often'],
      [null],
    ];
    for (const [opts, shorthand] of bads) {
      expect(() => busyRetryOf(opts as BusyRetryOptions, shorthand), JSON.stringify(opts)).toThrow(TypeError);
    }
    const door = new MockDoor();
    expect(() => client(door, { busyRetries: -1 })).toThrow(TypeError);
    expect(() => client(door, { busyRetry: { factor: 0 } })).toThrow(TypeError);
  });

  it('delays grow across real retries', async () => {
    vi.useFakeTimers();
    const door = new MockDoor().program(busy, busy, busy, ok);
    const done = client(door, {
      busyRetry: { retries: 3, firstDelayMs: 100, factor: 3, jitter: false },
    }).fetchOp('std.tasks.get', get);
    await vi.advanceTimersByTimeAsync(99);
    expect(door.requests).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(door.requests).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(299);
    expect(door.requests).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(door.requests).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(899);
    expect(door.requests).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(1);
    expect((await done).rows).toHaveLength(1);
  });

  it('an exhausted retry surfaces server_busy, not internal', async () => {
    const door = new MockDoor().always(busy);
    const err = await client(door).fetchOp('std.tasks.get', get).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoRefusal);
    expect((err as OrdoRefusal).class).toBe('server_busy');
  });
});
