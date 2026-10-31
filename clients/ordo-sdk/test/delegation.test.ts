/**
 * The delegation ceremony against a mock door: the agent's three calls and the
 * person's five platform ops.
 */

import { describe, expect, it } from 'vitest';
import {
  DelegationError,
  approveDelegation,
  declineDelegation,
  myDelegations,
  redeemDelegation,
  requestDelegation,
  reviewDelegation,
  revokeDelegation,
  waitForDelegation,
} from '../src/auth/delegation.js';
import { OrdoRefusal } from '../src/core/refusal.js';
import { MockDoor, q } from './harness.js';

interface Sent {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: Record<string, unknown> | null;
}

function door(...replies: Array<{ status?: number; json?: unknown }>) {
  const sent: Sent[] = [];
  let i = 0;
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const raw = init?.body;
    sent.push({
      url: String(input),
      method: String(init?.method ?? 'GET'),
      headers: { ...((init?.headers ?? {}) as Record<string, string>) },
      body: typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null,
    });
    const r = replies[Math.min(i++, replies.length - 1)] ?? { status: 500 };
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), { status: r.status ?? 200 });
  };
  return { sent, fetch };
}

const BASE = 'https://door.example/api/';
const CLAIM = { request: 'dr_1', binder: 'b'.repeat(64) };
const FAR = 4_000_000_000;
const WAITING = { status: 202, json: { status: 'waiting', expiresAt: FAR } };
const GRANTED = {
  status: 200,
  json: { token: 'tok_agent', expiresAt: 1000, delegation: 'dlg_1', space: 'spc_a', ops: ['notes.list'] },
};
const fail = (status: number, code: string, message = 'no') => ({
  status,
  json: { error: { code, message } },
});
const nap = async () => {};

describe('requestDelegation', () => {
  it('posts the request with no token and answers the id and binder', async () => {
    const d = door({
      status: 201,
      json: { request: 'dr_1', binder: 'b'.repeat(64), expiresAt: 900, review: 'std.delegation.review' },
    });
    const r = await requestDelegation(
      { baseUrl: BASE, fetch: d.fetch },
      { space: 'spc_a', agent: 'agt_1', person: 'mem_1', ops: ['notes.list'], hours: 4 },
    );
    expect(r).toEqual({
      request: 'dr_1',
      binder: 'b'.repeat(64),
      expiresAt: 900,
      review: 'std.delegation.review',
    });
    expect(d.sent[0]?.url).toBe('https://door.example/api/auth/delegation/request');
    expect(d.sent[0]?.method).toBe('POST');
    expect(d.sent[0]?.headers['Authorization']).toBeUndefined();
    expect(d.sent[0]?.body).toEqual({
      space: 'spc_a',
      agent: 'agt_1',
      person: 'mem_1',
      ops: ['notes.list'],
      hours: 4,
    });
  });

  it('leaves hours out when not given', async () => {
    const d = door({
      status: 201,
      json: { request: 'dr_1', binder: 'x', expiresAt: 1, review: 'std.delegation.review' },
    });
    await requestDelegation({ baseUrl: BASE, fetch: d.fetch }, { space: 's', agent: 'a', person: 'p', ops: [] });
    expect(d.sent[0]?.body).not.toHaveProperty('hours');
  });

  it('throws refused with the setting named in the message', async () => {
    const d = door(fail(403, 'delegation_refused', 'ORDO_DELEGATION_MAX_HOURS=8 does not allow 24'));
    const e = await requestDelegation(
      { baseUrl: BASE, fetch: d.fetch },
      { space: 's', agent: 'a', person: 'p', ops: ['x'], hours: 24 },
    ).catch((x: unknown) => x);
    expect(e).toBeInstanceOf(DelegationError);
    expect((e as DelegationError).failure).toBe('refused');
    expect((e as DelegationError).code).toBe('delegation_refused');
    expect((e as DelegationError).message).toContain('ORDO_DELEGATION_MAX_HOURS');
  });

  it('sends the human token and maps the cap and the human check', async () => {
    const d = door({
      status: 201,
      json: { request: 'dr_1', binder: 'x', expiresAt: 1, review: 'std.delegation.review' },
    });
    await requestDelegation(
      { baseUrl: BASE, fetch: d.fetch },
      { space: 's', agent: 'a', person: 'p', ops: ['x'], human: 'tok' },
    );
    expect(d.sent[0]?.body).toHaveProperty('human', 'tok');
    const limit = door(fail(429, 'delegation_limit', 'set ORDO_DELEGATION_OPEN_PER_PERSON=11'));
    const e = await requestDelegation(
      { baseUrl: BASE, fetch: limit.fetch },
      { space: 's', agent: 'a', person: 'p', ops: ['x'] },
    ).catch((x: unknown) => x);
    expect((e as DelegationError).failure).toBe('limit');
    const human = door(fail(401, 'authentication_failed'));
    const h = await requestDelegation(
      { baseUrl: BASE, fetch: human.fetch },
      { space: 's', agent: 'a', person: 'p', ops: ['x'] },
    ).catch((x: unknown) => x);
    expect((h as DelegationError).failure).toBe('human_check');
  });

  it('maps invalid_request and an unconfigured door', async () => {
    const a = door(fail(400, 'invalid_request'));
    const b = door(fail(503, 'delegation_store_unavailable'));
    const ep = (d: ReturnType<typeof door>) => ({ baseUrl: BASE, fetch: d.fetch });
    const req = { space: 's', agent: 'a', person: 'p', ops: [] };
    await expect(requestDelegation(ep(a), req)).rejects.toMatchObject({ failure: 'invalid_request' });
    await expect(requestDelegation(ep(b), req)).rejects.toMatchObject({ failure: 'unavailable' });
  });

  it('says network when the door cannot be reached', async () => {
    const fetch: typeof globalThis.fetch = async () => {
      throw new TypeError('down');
    };
    await expect(
      requestDelegation({ baseUrl: BASE, fetch }, { space: 's', agent: 'a', person: 'p', ops: [] }),
    ).rejects.toMatchObject({ kind: 'unreachable' });
  });
});

describe('redeemDelegation', () => {
  it('answers waiting on a 202, then granted', async () => {
    const d = door(WAITING, GRANTED);
    const ep = { baseUrl: BASE, fetch: d.fetch };
    expect(await redeemDelegation(ep, CLAIM)).toEqual({ status: 'waiting', expiresAt: FAR });
    expect(await redeemDelegation(ep, CLAIM)).toEqual({
      status: 'granted',
      token: 'tok_agent',
      expiresAt: 1000,
      delegation: 'dlg_1',
      space: 'spc_a',
      ops: ['notes.list'],
    });
    expect(d.sent[0]?.url).toBe('https://door.example/api/auth/delegation/redeem');
    expect(d.sent[0]?.body).toEqual(CLAIM);
  });

  it('throws declined, expired, redeemed and not_found', async () => {
    const cases: Array<[ReturnType<typeof fail>, string]> = [
      [fail(403, 'delegation_declined'), 'declined'],
      [fail(410, 'delegation_expired'), 'expired'],
      [fail(410, 'delegation_redeemed'), 'redeemed'],
      [fail(404, 'not_found'), 'not_found'],
    ];
    for (const [reply, failure] of cases) {
      const d = door(reply);
      await expect(redeemDelegation({ baseUrl: BASE, fetch: d.fetch }, CLAIM)).rejects.toMatchObject({
        failure,
        status: reply.status,
      });
    }
  });

  it('gives a wrong binder the same not_found as an unknown request', async () => {
    const d = door(fail(404, 'not_found', 'not found'));
    const e = (await redeemDelegation({ baseUrl: BASE, fetch: d.fetch }, { request: 'dr_1', binder: 'wrong' }).catch(
      (x: unknown) => x,
    )) as DelegationError;
    expect(e.failure).toBe('not_found');
    expect(d.sent[0]?.body).toEqual({ request: 'dr_1', binder: 'wrong' });
  });
});

describe('waitForDelegation', () => {
  it('polls until the person approves', async () => {
    const d = door(WAITING, WAITING, GRANTED);
    const sleeps: number[] = [];
    const seen: number[] = [];
    const r = await waitForDelegation({ baseUrl: BASE, fetch: d.fetch }, CLAIM, {
      intervalMs: 50,
      sleep: async (ms) => void sleeps.push(ms),
      onWaiting: (s) => void seen.push(s.expiresAt),
    });
    expect(r.token).toBe('tok_agent');
    expect(d.sent).toHaveLength(3);
    expect(sleeps).toEqual([50, 50]);
    expect(seen).toEqual([FAR, FAR]);
  });

  it('defaults the interval to 2000 ms', async () => {
    const d = door(WAITING, GRANTED);
    const sleeps: number[] = [];
    await waitForDelegation({ baseUrl: BASE, fetch: d.fetch }, CLAIM, { sleep: async (ms) => void sleeps.push(ms) });
    expect(sleeps).toEqual([2000]);
  });

  it('stops at a final refusal', async () => {
    const d = door(WAITING, fail(403, 'delegation_declined'));
    await expect(
      waitForDelegation({ baseUrl: BASE, fetch: d.fetch }, CLAIM, { sleep: nap }),
    ).rejects.toMatchObject({ failure: 'declined' });
    expect(d.sent).toHaveLength(2);
  });

  it('stops at the delegation expiresAt when no timeoutMs is given', async () => {
    const expiresAt = 1000;
    const d = door({ status: 202, json: { status: 'waiting', expiresAt } });
    let clock = 990_000;
    await expect(
      waitForDelegation({ baseUrl: BASE, fetch: d.fetch }, CLAIM, {
        intervalMs: 4000,
        now: () => clock,
        sleep: async (ms) => void (clock += ms),
      }),
    ).rejects.toMatchObject({ failure: 'timeout' });
    // Tries at 990 s, 994 s and 998 s; at 998 s the next sleep would pass 1000 s.
    expect(d.sent).toHaveLength(3);
  });

  it('gives up at timeoutMs', async () => {
    const d = door(WAITING);
    await expect(
      waitForDelegation({ baseUrl: BASE, fetch: d.fetch }, CLAIM, { intervalMs: 100, timeoutMs: 50, sleep: nap }),
    ).rejects.toMatchObject({ failure: 'timeout' });
    expect(d.sent).toHaveLength(1);
  });

  it('stops when aborted between tries', async () => {
    const d = door(WAITING);
    const ac = new AbortController();
    const p = waitForDelegation({ baseUrl: BASE, fetch: d.fetch }, CLAIM, {
      intervalMs: 60_000,
      signal: ac.signal,
    });
    await new Promise((r) => setTimeout(r, 10));
    ac.abort();
    await expect(p).rejects.toMatchObject({ failure: 'aborted' });
    expect(d.sent).toHaveLength(1);
  });

  it('does not call the door when already aborted', async () => {
    const d = door(WAITING);
    const ac = new AbortController();
    ac.abort();
    await expect(
      waitForDelegation({ baseUrl: BASE, fetch: d.fetch }, CLAIM, { signal: ac.signal }),
    ).rejects.toMatchObject({ failure: 'aborted' });
    expect(d.sent).toHaveLength(0);
  });
});

describe('the person’s calls', () => {
  it('reviews with a GET carrying the request, the space and the token', async () => {
    const disclosure = { request: 'dr_1', tenant: 'spc_a', ops: [], ceiling: 'c', revoke: 'r', ownerSees: null };
    const m = new MockDoor().reply({ json: { rows: [disclosure] } });
    const r = await reviewDelegation(m.client(), 'dr_1');
    expect(r.request).toBe('dr_1');
    const c = m.calls[0]!;
    expect(c.method).toBe('GET');
    expect(c.url).toContain('/op/std.delegation.review');
    expect(q(c.url).get('request')).toBe('dr_1');
    expect(c.headers['X-Ordo-Space']).toBe('spc_test');
    expect(c.headers['Authorization']).toBe('Bearer tok');
    expect(c.headers['X-Ordo-Version']).toBe('platform');
  });

  it('approves with a POST body of the request and an idempotency key', async () => {
    const row = { request: 'dr_1', delegation: 'dlg_1', agent: 'agt_1', ops: ['x'], expiresAt: 5, replaced: [], outcome: 'approved' };
    const m = new MockDoor().reply({ json: { rows: [row] } });
    const r = await approveDelegation(m.client(), 'dr_1');
    expect(r.outcome).toBe('approved');
    const c = m.calls[0]!;
    expect(c.method).toBe('POST');
    expect(c.url).toContain('/op/std.delegation.approve');
    expect(c.body).toEqual({ request: 'dr_1' });
    expect(c.headers['Idempotency-Key']).toBe('idem-fixed');
  });

  it('shows an approve without a fresh touch as the door’s refusal', async () => {
    const m = new MockDoor().reply({
      status: 403,
      json: { error: { code: 'access_denied', message: 'needs a fresh passkey touch' } },
    });
    await expect(approveDelegation(m.client(), 'dr_1')).rejects.toBeInstanceOf(OrdoRefusal);
  });

  it('declines with a POST', async () => {
    const m = new MockDoor().reply({ json: { rows: [{ request: 'dr_1', outcome: 'declined' }] } });
    expect((await declineDelegation(m.client(), 'dr_1')).outcome).toBe('declined');
    expect(m.calls[0]?.method).toBe('POST');
    expect(m.calls[0]?.url).toContain('/op/std.delegation.decline');
    expect(m.calls[0]?.body).toEqual({ request: 'dr_1' });
  });

  it('lists the person’s delegations with a GET and no params', async () => {
    const rows = [
      { request: 'dr_1', delegation: null, agent: 'agt_1', agentName: 'Bot', state: 'waiting', ops: ['x'], grantedAt: null, expiresAt: 5, revokedAt: null },
    ];
    const m = new MockDoor().reply({ json: { rows } });
    const mine = await myDelegations(m.client());
    expect(mine).toHaveLength(1);
    expect(mine[0]?.state).toBe('waiting');
    expect(m.calls[0]?.method).toBe('GET');
    expect(m.calls[0]?.url).toContain('/op/std.delegation.mine');
    expect(m.calls[0]?.url).not.toContain('?');
  });

  it('revokes with a POST naming the delegation', async () => {
    const m = new MockDoor().reply({ json: { rows: [{ delegation: 'dlg_1', outcome: 'already ended' }] } });
    expect((await revokeDelegation(m.client(), 'dlg_1')).outcome).toBe('already ended');
    expect(m.calls[0]?.method).toBe('POST');
    expect(m.calls[0]?.url).toContain('/op/std.delegation.revoke');
    expect(m.calls[0]?.body).toEqual({ delegation: 'dlg_1' });
  });

  it('throws when the door answers no row', async () => {
    const m = new MockDoor().reply({ json: { rows: [] } });
    await expect(reviewDelegation(m.client(), 'dr_1')).rejects.toBeInstanceOf(DelegationError);
  });
});
