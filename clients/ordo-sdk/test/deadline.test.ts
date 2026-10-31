/** The deadline on sign-in calls. */

import { describe, expect, it } from 'vitest';
import { profile } from '../src/auth/access.js';
import { listConnections } from '../src/auth/connections.js';
import { DEFAULT_CALL_TIMEOUT_MS, fetchWithDeadline } from '../src/auth/deadline.js';
import { DelegationError, requestDelegation, waitForDelegation } from '../src/auth/delegation.js';
import { listSpaces } from '../src/auth/share.js';
import { call } from '../src/auth/passkey.js';
import { OrdoNetworkError } from '../src/core/network.js';

const never: typeof globalThis.fetch = () => new Promise<Response>(() => {});
const BASE = 'https://door.example/api/';
const TINY = 20;

describe('a call to the door has a deadline', () => {
  it('defaults to 15 seconds', () => {
    expect(DEFAULT_CALL_TIMEOUT_MS).toBe(15_000);
  });

  it('passkey call rejects as a timeout when the door never answers', async () => {
    const err = await call({ baseUrl: BASE, fetch: never, callTimeoutMs: TINY }, '/auth/x', {}).catch((e) => e);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect(err.kind).toBe('timeout');
    expect(err.option).toBe('callTimeoutMs');
    expect(err.message).toMatch(/callTimeoutMs/);
  });

  it('access rejects as a timeout when the door never answers', async () => {
    const err = await profile({ baseUrl: BASE, fetch: never, callTimeoutMs: TINY, space: 's', token: 't' }).catch((e) => e);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect(err.kind).toBe('timeout');
    expect(err.option).toBe('callTimeoutMs');
    expect(err.message).toMatch(/callTimeoutMs/);
  });

  it('connections rejects as a timeout when the door never answers', async () => {
    const err = await listConnections({ baseUrl: BASE, fetch: never, callTimeoutMs: TINY, space: 's', token: 't' }).catch((e) => e);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect(err.kind).toBe('timeout');
    expect(err.option).toBe('callTimeoutMs');
    expect(err.message).toMatch(/callTimeoutMs/);
  });

  it('listSpaces rejects as a timeout when the door never answers', async () => {
    const err = await listSpaces({ baseUrl: BASE, fetch: never, callTimeoutMs: TINY }, 'tok').catch((e) => e);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect(err.kind).toBe('timeout');
    expect(err.option).toBe('callTimeoutMs');
    expect(err.message).toMatch(/callTimeoutMs/);
  });

  it('delegation rejects as a timeout when the door never answers', async () => {
    const err = await requestDelegation(
      { baseUrl: BASE, fetch: never, callTimeoutMs: TINY },
      { space: 's', agent: 'a', person: 'p', ops: ['a'] },
    ).catch((e) => e);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect(err.kind).toBe('timeout');
    expect(err.option).toBe('callTimeoutMs');
    expect(err.message).toMatch(/callTimeoutMs/);
  });

  it('a delegation wait aborted by the caller is aborted, not unavailable', async () => {
    const ctl = new AbortController();
    const p = waitForDelegation({ baseUrl: BASE, fetch: never, callTimeoutMs: 60_000 }, { handle: 'h' } as never, { signal: ctl.signal }).catch((e) => e);
    ctl.abort();
    const err = await p;
    expect(err).toBeInstanceOf(DelegationError);
    expect(err.failure).toBe('aborted');
  });

  it("the caller's own signal still aborts, and is not called a timeout", async () => {
    const ctl = new AbortController();
    const p = fetchWithDeadline(never, BASE, {}, 60_000, ctl.signal);
    ctl.abort(new Error('mine'));
    await expect(p).rejects.toThrow('mine');
  });
});
