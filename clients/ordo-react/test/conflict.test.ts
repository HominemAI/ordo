/**
 * A write that lost a race with other writes to the same rows answers
 * `409 serialization_conflict`, and the client sends it again with the same
 * `Idempotency-Key`, `conflictRetry.retries` times at most (default 3), as
 * `@hominemai/ordo-sdk` does (owner ruling, 2026-10-19).
 */

import { afterEach, describe, expect, it, vi } from 'vitest';
import { OrdoRefusal } from '../src/refusal.js';
import { conflictDelayMs, conflictRetryOf } from '../src/busy.js';
import type { ConflictRetryOptions, RetryReason } from '../src/busy.js';
import { MockDoor, client as plainClient, refusal } from './harness.js';

// Real-timer tests wait nothing: the first delay is 0 unless a test says so.
const client = (door: MockDoor, o: { conflictRetries?: number; conflictRetry?: ConflictRetryOptions } = {}) =>
  plainClient(door, {
    ...o,
    busyRetry: { firstDelayMs: 0 },
    conflictRetry: { firstDelayMs: 0, ...o.conflictRetry },
  });

const lost = { status: 409, body: refusal('serialization_conflict', 'other writes to the same rows won') };
const busy = { status: 503, body: refusal('server_busy', 'every connection is in use'), headers: { 'Retry-After': '0' } };
const updated = { body: { op: 'std.tasks.update', rows: [{ id: 'task_1', rev: 4 }] } };
const update = {
  params: { id: 'task_1', rev: 3, project: 'proj_1', assignee: 'mem_1', title: 'Ship', status: 'open' },
};

afterEach(() => vi.useRealTimers());

describe('retrying serialization_conflict', () => {
  it('is on by default, and every try carries the same idempotency key', async () => {
    const door = new MockDoor().program(lost, lost, updated);
    await client(door).commit('std.tasks.update', update);
    expect(door.requests.map((r) => r.headers['Idempotency-Key'])).toEqual(['idem-0', 'idem-0', 'idem-0']);
  });

  it('retries stop at the limit and the conflict surfaces', async () => {
    const door = new MockDoor().always(lost);
    const e = await client(door).commit('std.tasks.update', update).catch((x: unknown) => x);
    expect(e).toBeInstanceOf(OrdoRefusal);
    // The first attempt and the default 3 retries.
    expect(door.requests).toHaveLength(4);
  });

  it('conflictRetries 0 turns it off', async () => {
    const door = new MockDoor().program(lost, updated);
    await expect(client(door, { conflictRetries: 0 }).commit('std.tasks.update', update)).rejects.toBeInstanceOf(
      OrdoRefusal,
    );
    expect(door.requests).toHaveLength(1);
  });

  it('another 409 class is not retried', async () => {
    const door = new MockDoor().program({ status: 409, body: refusal('cas_conflict') }, updated);
    await expect(client(door).commit('std.tasks.update', update)).rejects.toBeInstanceOf(OrdoRefusal);
    expect(door.requests).toHaveLength(1);
  });

  it('busy and conflict retries are counted apart, and onRetry names the reason', async () => {
    const door = new MockDoor().program(busy, lost, lost, updated);
    const told: [number, RetryReason][] = [];
    await client(door).commit('std.tasks.update', { ...update, onRetry: (n, _ms, why) => told.push([n, why]) });
    expect(told).toEqual([
      [1, 'server_busy'],
      [1, 'serialization_conflict'],
      [2, 'serialization_conflict'],
    ]);
  });

  it('the defaults and the jitter match the SDK', () => {
    expect(conflictRetryOf(undefined)).toEqual({
      retries: 3,
      firstDelayMs: 50,
      factor: 2,
      maxDelayMs: 1_000,
      jitter: true,
    });
    const on = conflictRetryOf({ firstDelayMs: 100 });
    expect(conflictDelayMs(on, 1, null, () => 0)).toBe(100);
    expect(conflictDelayMs(on, 1, null, () => 0.5)).toBe(150);
    expect(() => conflictRetryOf({ retries: -1 })).toThrow(/conflictRetry\.retries/);
  });
});
