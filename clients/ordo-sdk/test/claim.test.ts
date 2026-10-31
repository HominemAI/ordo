/**
 * Linking by claim, against a mock door.
 *
 * What is proved is what this package owns: the bearer and the bodies of both
 * calls, the shape of the answers, the `claim` passed to an emailed code, and
 * that the named refusals surface with their code.
 */

import { describe, expect, it } from 'vitest';
import { PasskeyError, type PasskeyEndpoint } from '../src/auth/passkey.js';
import { beginEmailSignIn } from '../src/auth/email.js';
import {
  beginClaim,
  completeClaim,
  emailCodeCeremony,
  handoffCeremony,
  passwordCeremony,
} from '../src/auth/claim.js';

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
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), {
      status: r.status ?? 200,
    });
  };
  return { sent, fetch };
}

const endpointOf = (fetch: typeof globalThis.fetch): PasskeyEndpoint => ({
  baseUrl: 'https://door.example',
  fetch,
});

const HELD = emailCodeCeremony({ challenge: 'ch_1', binder: 'b1' }, ' 123456 ');
const CLAIMED = handoffCeremony('discord', 'ho_1', 'b2');

describe('beginClaim', () => {
  it('sends the bearer and the ceremony body, and returns the claim and expiry', async () => {
    const d = door({ json: { claim: 'clm_1', expires_at: 900 } });
    const opened = await beginClaim(endpointOf(d.fetch), 'tok_session', HELD);
    expect(opened).toEqual({ claim: 'clm_1', expiresAt: 900 });
    expect(d.sent[0]?.url).toBe('https://door.example/auth/claim/begin');
    expect(d.sent[0]?.method).toBe('POST');
    expect(d.sent[0]?.headers['Authorization']).toBe('Bearer tok_session');
    expect(d.sent[0]?.body).toEqual({
      method: 'email',
      kind: 'email_code',
      challenge: 'ch_1',
      binder: 'b1',
      code: '123456',
    });
  });

  it('builds password and hand-off proofs in the complete shape', () => {
    expect(passwordCeremony('c', 'a@b.c', 'pw')).toEqual({
      method: 'password',
      kind: 'password',
      challenge: 'c',
      identifier: 'a@b.c',
      password: 'pw',
    });
    expect(CLAIMED).toEqual({ method: 'discord', kind: 'handoff', handoff: 'ho_1', binder: 'b2' });
  });

  it('rejects malformed when the answer has no claim', async () => {
    const d = door({ json: { expires_at: 1 } });
    await expect(beginClaim(endpointOf(d.fetch), 't', HELD)).rejects.toMatchObject({
      failure: 'malformed',
    });
  });

  it('surfaces linking_off with the setting and value', async () => {
    const d = door({
      status: 403,
      json: {
        error: { code: 'linking_off', message: 'off', setting: 'ORDO_AUTH_LINKING', value: 'on' },
      },
    });
    const e = await beginClaim(endpointOf(d.fetch), 't', HELD).catch((x: unknown) => x);
    expect(e).toBeInstanceOf(PasskeyError);
    expect(e).toMatchObject({
      failure: 'linking_off',
      setting: 'ORDO_AUTH_LINKING',
      value: 'on',
    });
  });

  it('answers the one refusal as refused', async () => {
    const d = door({ status: 401, json: { error: { code: 'authentication_failed' } } });
    await expect(beginClaim(endpointOf(d.fetch), 't', HELD)).rejects.toMatchObject({
      failure: 'refused',
    });
  });

  it('answers a store outage as unavailable', async () => {
    const d = door({ status: 503, json: { error: { code: 'credential_store_unavailable' } } });
    await expect(beginClaim(endpointOf(d.fetch), 't', HELD)).rejects.toMatchObject({
      failure: 'unavailable',
    });
  });
});

describe('completeClaim', () => {
  it('sends the bearer, the claim and the ceremony, and returns what was linked', async () => {
    const d = door({ json: { linked: 'auth_9', via: 'discord', authenticators: 3 } });
    const linked = await completeClaim(endpointOf(d.fetch), 'tok_session', 'clm_1', CLAIMED);
    expect(linked).toEqual({ linked: 'auth_9', via: 'discord', authenticators: 3, already: false });
    expect(d.sent[0]?.url).toBe('https://door.example/auth/claim/complete');
    expect(d.sent[0]?.headers['Authorization']).toBe('Bearer tok_session');
    expect(d.sent[0]?.body).toEqual({
      method: 'discord',
      kind: 'handoff',
      handoff: 'ho_1',
      binder: 'b2',
      claim: 'clm_1',
    });
  });

  it("reports already when the sign-in was this account's", async () => {
    const d = door({ json: { linked: 'auth_9', via: 'discord', already: true } });
    const linked = await completeClaim(endpointOf(d.fetch), 't', 'clm_1', CLAIMED);
    expect(linked).toEqual({ linked: 'auth_9', via: 'discord', already: true });
  });

  it('surfaces identifier_taken', async () => {
    const d = door({ status: 409, json: { error: { code: 'identifier_taken', message: 'x' } } });
    await expect(completeClaim(endpointOf(d.fetch), 't', 'clm_1', CLAIMED)).rejects.toMatchObject({
      failure: 'identifier_taken',
    });
  });

  it('surfaces linking_off', async () => {
    const d = door({
      status: 403,
      json: { error: { code: 'linking_off', setting: 'ORDO_AUTH_LINKING', value: 'on' } },
    });
    await expect(completeClaim(endpointOf(d.fetch), 't', 'clm_1', CLAIMED)).rejects.toMatchObject({
      failure: 'linking_off',
      setting: 'ORDO_AUTH_LINKING',
    });
  });
});

describe('beginEmailSignIn with a claim', () => {
  it('passes claim through to /auth/begin', async () => {
    const d = door({ json: { challenge: 'ch_2', binder: 'b3' } });
    await beginEmailSignIn(endpointOf(d.fetch), { address: 'new@example.com', claim: 'clm_1' });
    expect(d.sent[0]?.url).toBe('https://door.example/auth/begin');
    expect(d.sent[0]?.body).toEqual({
      method: 'email',
      identifier: 'new@example.com',
      claim: 'clm_1',
    });
  });

  it('omits claim when not given', async () => {
    const d = door({ json: { challenge: 'ch_2', binder: 'b3' } });
    await beginEmailSignIn(endpointOf(d.fetch), { address: 'a@example.com' });
    expect(d.sent[0]?.body).not.toHaveProperty('claim');
  });
});
