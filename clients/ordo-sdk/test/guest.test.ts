/**
 * Guest sign-in, against a mock door.
 *
 * What is proved is what this package owns: the shape of the start and resume
 * requests, that the secret is returned and never in a URL, the guest proof for
 * a claim, and that the refusals reject the way the other sign-ins do.
 */

import { describe, expect, it } from 'vitest';
import { PasskeyError, type PasskeyEndpoint } from '../src/auth/passkey.js';
import { beginClaim } from '../src/auth/claim.js';
import { guestCeremony, resumeGuest, startGuest } from '../src/auth/guest.js';

interface Sent {
  readonly url: string;
  readonly method: string;
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
      body: typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null,
    });
    const r = replies[Math.min(i++, replies.length - 1)] ?? { status: 500 };
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), {
      status: r.status ?? 200,
    });
  };
  return { sent, fetch };
}

const SECRET = 'S'.repeat(43);
const SESSION = {
  token: 'tok_guest',
  via: 'guest',
  expires_at: 1000,
  session_expires_at: 2000,
};

const endpointOf = (fetch: typeof globalThis.fetch): PasskeyEndpoint => ({
  baseUrl: 'https://door.example',
  fetch,
});

describe('startGuest', () => {
  it('posts method guest and kind new, with nothing else by default', async () => {
    const d = door({ json: { ...SESSION, guest_secret: SECRET } });
    await startGuest(endpointOf(d.fetch));
    expect(d.sent[0]?.url).toBe('https://door.example/auth/complete');
    expect(d.sent[0]?.method).toBe('POST');
    expect(d.sent[0]?.body).toEqual({ method: 'guest', kind: 'new' });
  });

  it('passes human, display, space and invitation when given', async () => {
    const d = door({ json: { ...SESSION, guest_secret: SECRET } });
    await startGuest(endpointOf(d.fetch), {
      human: 'ts_1',
      display: 'Visitor',
      space: 'acme',
      invitation: 'inv_1',
    });
    expect(d.sent[0]?.body).toEqual({
      method: 'guest',
      kind: 'new',
      human: 'ts_1',
      display: 'Visitor',
      space: 'acme',
      invitation: 'inv_1',
    });
  });

  it('returns the secret and the session, and the secret is in no URL', async () => {
    const d = door({ json: { ...SESSION, guest_secret: SECRET } });
    const started = await startGuest(endpointOf(d.fetch));
    expect(started.secret).toBe(SECRET);
    expect(started.result.kind).toBe('session');
    if (started.result.kind === 'session') {
      expect(started.result.session.token).toBe('tok_guest');
    }
    for (const s of d.sent) expect(s.url).not.toContain(SECRET);
  });

  it('rejects a session answer with no guest_secret as malformed', async () => {
    const d = door({ json: SESSION });
    await expect(startGuest(endpointOf(d.fetch))).rejects.toMatchObject({ failure: 'malformed' });
  });

  it('rejects a 403 guests_off with the setting and value', async () => {
    const d = door({
      status: 403,
      json: { error: { code: 'guests_off', setting: 'ORDO_AUTH_GUEST', value: 'on' } },
    });
    const err = await startGuest(endpointOf(d.fetch)).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(PasskeyError);
    expect(err).toMatchObject({
      failure: 'guests_off',
      setting: 'ORDO_AUTH_GUEST',
      value: 'on',
    });
  });

  it('rejects a 401 as refused', async () => {
    const d = door({
      status: 401,
      json: { error: { code: 'authentication_failed', message: 'x' } },
    });
    await expect(startGuest(endpointOf(d.fetch))).rejects.toMatchObject({ failure: 'refused' });
  });

  it('rejects a 429 as refused', async () => {
    const d = door({ status: 429, json: { error: { code: 'quota_exceeded' } } });
    await expect(startGuest(endpointOf(d.fetch))).rejects.toMatchObject({ failure: 'refused' });
  });
});

describe('resumeGuest', () => {
  it('posts method guest, kind resume and the secret, and returns a session', async () => {
    const d = door({ json: SESSION });
    const out = await resumeGuest(endpointOf(d.fetch), SECRET);
    expect(d.sent[0]?.url).toBe('https://door.example/auth/complete');
    expect(d.sent[0]?.body).toEqual({ method: 'guest', kind: 'resume', secret: SECRET });
    expect(out.kind).toBe('session');
  });

  it('carries space and invitation from the options', async () => {
    const d = door({ json: SESSION });
    await resumeGuest(endpointOf(d.fetch), SECRET, { space: 'acme', invitation: 'inv_1' });
    expect(d.sent[0]?.body).toEqual({
      method: 'guest',
      kind: 'resume',
      secret: SECRET,
      space: 'acme',
      invitation: 'inv_1',
    });
  });

  it('rejects a 401 as refused', async () => {
    const d = door({ status: 401, json: { error: { code: 'authentication_failed' } } });
    await expect(resumeGuest(endpointOf(d.fetch), SECRET)).rejects.toMatchObject({
      failure: 'refused',
    });
  });
});

describe('guestCeremony', () => {
  it('builds the resume body', () => {
    expect(guestCeremony(SECRET)).toEqual({ method: 'guest', kind: 'resume', secret: SECRET });
  });

  it('is what beginClaim posts to /auth/claim/begin', async () => {
    const d = door({ json: { claim: 'clm_1', expires_at: 900 } });
    const opened = await beginClaim(endpointOf(d.fetch), 'tok_guest', guestCeremony(SECRET));
    expect(opened).toEqual({ claim: 'clm_1', expiresAt: 900 });
    expect(d.sent[0]?.url).toBe('https://door.example/auth/claim/begin');
    expect(d.sent[0]?.body).toEqual({ method: 'guest', kind: 'resume', secret: SECRET });
  });
});
