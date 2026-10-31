/**
 * A write that lost a race with other writes to the same rows, through the
 * server's own retries, answers `409 serialization_conflict`. Nothing was
 * applied, so the client sends it again (owner ruling, 2026-10-19),
 * `conflictRetry.retries` times at most (default 3), with the same
 * `Idempotency-Key`.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

import { OrdoClient, OrdoRefusal } from '../src/index.js';
import type { ConflictRetryOptions, OpSpec, RetryReason } from '../src/index.js';
import { conflictDelayMs, conflictRetryOf, DEFAULT_CONFLICT_RETRY } from '../src/core/busy.js';
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

const write: OpSpec = { ...read, op: 'claimSeat', method: 'POST', kind: 'write' };

const lost = {
  status: 409,
  json: {
    error: {
      code: 'serialization_conflict',
      message: 'other writes to the same rows won 6 times in a row. The retries are ORDO_WRITE_RETRIES=5',
    },
  },
};
const busy = {
  status: 503,
  json: { error: { code: 'server_busy', message: 'every connection is in use' } },
  headers: { 'Retry-After': '0' },
};
const ok = { json: { rows: [{ id: 'p1' }] } };

// Real-timer tests wait nothing: the first delay is 0 unless a test says so.
function clientOf(
  door: MockDoor,
  extra: {
    conflictRetries?: number;
    conflictRetry?: ConflictRetryOptions;
    busyRetries?: number;
    newIdempotencyKey?: () => string;
  } = {},
) {
  return new OrdoClient({
    baseUrl: 'https://door.example/api',
    space: 'spc_test',
    token: 'tok',
    fetch: door.fetch,
    ...extra,
    busyRetry: { firstDelayMs: 0 },
    conflictRetry: { firstDelayMs: 0, ...extra.conflictRetry },
  });
}

afterEach(() => vi.useRealTimers());

describe('retrying serialization_conflict', () => {
  it('is on by default: a lost race then success returns the success', async () => {
    const door = new MockDoor().reply(lost, ok);
    const res = await clientOf(door).request(write, {});
    expect(res.rows).toEqual([{ id: 'p1' }]);
    expect(door.calls).toHaveLength(2);
  });

  it('a retried write carries the same idempotency key on every try', async () => {
    let n = 0;
    const door = new MockDoor().reply(lost, lost, lost, ok);
    await clientOf(door, { newIdempotencyKey: () => `key-${n++}` }).request(write, {});
    expect(door.calls.map((c) => c.headers['Idempotency-Key'])).toEqual(['key-0', 'key-0', 'key-0', 'key-0']);
    // A second call is a second write, with its own key.
    const again = new MockDoor().reply(ok);
    const c = clientOf(again, { newIdempotencyKey: () => `key-${n++}` });
    await c.request(write, {});
    expect(again.calls[0]!.headers['Idempotency-Key']).toBe('key-1');
  });

  it('retries stop at the limit and the conflict surfaces as the refusal', async () => {
    const door = new MockDoor().reply(lost);
    const e = await clientOf(door).request(write, {}).catch((x: unknown) => x);
    expect(e).toBeInstanceOf(OrdoRefusal);
    expect((e as OrdoRefusal).class).toBe('serialization_conflict');
    // The first attempt and the default 3 retries.
    expect(door.calls).toHaveLength(4);

    const one = new MockDoor().reply(lost);
    await expect(clientOf(one, { conflictRetries: 1 }).request(write, {})).rejects.toBeInstanceOf(OrdoRefusal);
    expect(one.calls).toHaveLength(2);
  });

  it('conflictRetries 0 turns it off', async () => {
    const door = new MockDoor().reply(lost, ok);
    await expect(clientOf(door, { conflictRetries: 0 }).request(write, {})).rejects.toBeInstanceOf(OrdoRefusal);
    expect(door.calls).toHaveLength(1);
  });

  it('another 409 class is not retried', async () => {
    for (const code of ['cas_conflict', 'duplicate', 'lease_fenced']) {
      const door = new MockDoor().reply({ status: 409, json: { error: { code, message: 'no' } } }, ok);
      await expect(clientOf(door).request(write, {})).rejects.toBeInstanceOf(OrdoRefusal);
      expect(door.calls, code).toHaveLength(1);
    }
  });

  it('busy and conflict retries are counted apart, and onRetry names the reason', async () => {
    const door = new MockDoor().reply(busy, lost, busy, lost, ok);
    const told: [number, RetryReason][] = [];
    const res = await clientOf(door).request(write, {}, { onRetry: (n, _ms, why) => told.push([n, why]) });
    expect(res.rows).toEqual([{ id: 'p1' }]);
    expect(told).toEqual([
      [1, 'server_busy'],
      [1, 'serialization_conflict'],
      [2, 'server_busy'],
      [2, 'serialization_conflict'],
    ]);
  });

  it('waits grow by the factor and stop at the ceiling', async () => {
    vi.useFakeTimers();
    const door = new MockDoor().reply(lost, lost, lost, ok);
    const done = clientOf(door, {
      conflictRetry: { firstDelayMs: 100, factor: 3, maxDelayMs: 500, jitter: false },
    }).request(write, {});
    await vi.advanceTimersByTimeAsync(99);
    expect(door.calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(door.calls).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(299);
    expect(door.calls).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(door.calls).toHaveLength(3);
    // 900 is held to the ceiling of 500.
    await vi.advanceTimersByTimeAsync(499);
    expect(door.calls).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(1);
    expect((await done).rows).toEqual([{ id: 'p1' }]);
  });

  it('jitter is a random time from half the wait to all of it', () => {
    const on = conflictRetryOf({ firstDelayMs: 100 });
    expect(on.jitter).toBe(true);
    expect(conflictDelayMs(on, 1, null, () => 0)).toBe(100);
    expect(conflictDelayMs(on, 1, null, () => 0.5)).toBe(150);
    expect(conflictDelayMs(on, 1, null, () => 0.999999)).toBeLessThanOrEqual(200);
    const off = conflictRetryOf({ firstDelayMs: 100, jitter: false });
    expect(conflictDelayMs(off, 1, null, () => 0.3)).toBe(200);
  });

  it('the defaults are the documented ones', () => {
    expect(conflictRetryOf(undefined)).toEqual({
      retries: 3,
      firstDelayMs: 50,
      factor: 2,
      maxDelayMs: 1_000,
      jitter: true,
    });
    expect(DEFAULT_CONFLICT_RETRY.retries).toBe(3);
    expect(conflictRetryOf(undefined, 5).retries).toBe(5);
  });

  it('bad options throw TypeError naming the option', () => {
    const bads: [ConflictRetryOptions | null | string, number?][] = [
      [{ retries: -1 }],
      [{ retries: 1.5 }],
      [{ firstDelayMs: -1 }],
      [{ factor: 0.5 }],
      [{ maxDelayMs: Number.NaN }],
      [{ jitter: 'yes' as unknown as boolean }],
      [{ retreis: 3 } as unknown as ConflictRetryOptions],
      [{ retries: 1 }, 2],
      ['often'],
      [null],
    ];
    for (const [opts, shorthand] of bads) {
      expect(() => conflictRetryOf(opts as ConflictRetryOptions, shorthand), JSON.stringify(opts)).toThrow(
        /conflictRetr/,
      );
    }
    expect(() => clientOf(new MockDoor(), { conflictRetries: -1 })).toThrow(TypeError);
  });
});
