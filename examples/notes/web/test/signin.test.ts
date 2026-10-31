/**
 * The ways in beyond the passkey: which the page offers, the trip to an
 * outside provider and back, an emailed code, a guest, and adding a passkey.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { toBase64Url } from '@hominemai/ordo-sdk/auth/passkey.js';
import { signInMethods } from '../src/config.js';
import { SecondStepNeeded, Session } from '../src/session.js';
import {
  beginOutside,
  completeOutside,
  deviceMarker,
  guestSecret,
  keepGuestSecret,
  returnedFrom,
  type Store,
} from '../src/signin.js';
import { CREATION_OPTIONS, credentials, door, REQUEST_OPTIONS, signedIn, type Reply, type Sent } from './support.js';

function memory(): Store & { readonly data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => void data.set(k, v),
    removeItem: (k) => void data.delete(k),
  };
}

const g = globalThis as Record<string, unknown>;
let local: ReturnType<typeof memory>;
beforeEach(() => {
  local = memory();
  g['localStorage'] = local;
  g['sessionStorage'] = memory();
});
afterEach(() => {
  delete g['localStorage'];
  delete g['sessionStorage'];
});

const paths = (d: ReturnType<typeof door>) => d.sent.map((s) => new URL(s.url).pathname);
const options = (d: ReturnType<typeof door>) => ({
  baseUrl: 'https://door.example/',
  fetch: d.fetch,
  credentials,
  now: () => 1000,
});
const endpoint = (d: ReturnType<typeof door>) => ({ baseUrl: 'https://door.example', fetch: d.fetch });

/** A token as the door writes it today: `v6|usr_x|<exp>|||||sess~...`. */
function tokenV6(principal: string): string {
  return `${toBase64Url(new TextEncoder().encode(`v6|${principal}|9999999999|||||sess~abc`))}.sig`;
}

describe('the ways in the page offers', () => {
  it('offers the passkey alone when nothing is set', () => {
    expect(signInMethods(undefined)).toEqual({ passkey: true, email: false, guest: false, connections: [] });
    expect(signInMethods('')).toEqual({ passkey: true, email: false, guest: false, connections: [] });
  });

  it('reads email, guest and connections from a list', () => {
    const m = signInMethods('passkey, Email,guest,discord,github');
    expect(m.passkey).toBe(true);
    expect(m.email).toBe(true);
    expect(m.guest).toBe(true);
    expect(m.connections).toEqual([
      { name: 'discord', label: 'Discord' },
      { name: 'github', label: 'GitHub' },
    ]);
  });

  it('shows an unknown connection as it was written', () => {
    expect(signInMethods('acme-sso').connections).toEqual([{ name: 'acme-sso', label: 'acme-sso' }]);
  });

  it('drops bad names and counts a repeat once', () => {
    const m = signInMethods('github,GitHub,has space,UPPER_case,,' + 'x'.repeat(33));
    expect(m.connections).toEqual([{ name: 'github', label: 'GitHub' }]);
  });

  it('does not take password for a connection', () => {
    const m = signInMethods('email,password');
    expect(m.connections).toEqual([]);
    expect(m.passkey).toBe(false);
  });
});

describe('coming back from an outside provider', () => {
  it('says there is nothing when the address carries no hand-off', () => {
    expect(returnedFrom('', memory())).toEqual({ kind: 'none' });
    expect(returnedFrom('?other=1', memory())).toEqual({ kind: 'none' });
  });

  it('answers the method, hand-off, binder and kept hash, and forgets the binder', async () => {
    const store = memory();
    const d = door({ json: { redirect: 'https://github.example/auth', binder: 'bnd_1' } });
    await beginOutside(endpoint(d), 'github', 'https://notes.example/', undefined, '#share=abc', store);
    const first = returnedFrom('?handoff=code_9', store);
    expect(first).toEqual({ kind: 'handoff', method: 'github', handoff: 'code_9', binder: 'bnd_1', hash: '#share=abc' });
    // A hand-off is good once: the second read has no binder.
    expect(returnedFrom('?handoff=code_9', store)).toMatchObject({ kind: 'error' });
  });

  it('is an error when the hand-off has no binder kept', () => {
    expect(returnedFrom('?handoff=code_9', memory())).toEqual({ kind: 'error', error: 'authentication_failed', hash: '' });
  });

  it('carries the word the provider sent back with an error', () => {
    expect(returnedFrom('?error=identity_provider_unavailable', memory())).toEqual({
      kind: 'error',
      error: 'identity_provider_unavailable',
      hash: '',
    });
  });

  it('posts the method, return address and human token to begin, and keeps the binder', async () => {
    const store = memory();
    const d = door({ json: { redirect: 'https://discord.example/go', binder: 'bnd_2' } });
    const redirect = await beginOutside(endpoint(d), 'discord', 'https://notes.example/', 'turn_tok', '', store);
    expect(redirect).toBe('https://discord.example/go');
    expect(paths(d)).toEqual(['/auth/begin']);
    expect(d.sent[0]?.body).toEqual({ method: 'discord', return_to: 'https://notes.example/', human: 'turn_tok' });
    expect(JSON.parse(store.data.get('ordo-notes.outside') ?? 'null')).toMatchObject({ method: 'discord', binder: 'bnd_2' });
  });

  it('leaves the human token out when there is none', async () => {
    const d = door({ json: { redirect: 'https://x.example', binder: 'b' } });
    await beginOutside(endpoint(d), 'github', 'https://notes.example/', undefined, '', memory());
    expect(d.sent[0]?.body).toEqual({ method: 'github', return_to: 'https://notes.example/' });
  });

  it('swaps the hand-off and binder for a session at complete', async () => {
    const d = door({ json: signedIn() });
    const r = await completeOutside(endpoint(d), { method: 'github', handoff: 'code_9', binder: 'bnd_1' });
    expect(r.kind).toBe('session');
    expect(paths(d)).toEqual(['/auth/complete']);
    expect(d.sent[0]?.body).toEqual({ method: 'github', kind: 'handoff', handoff: 'code_9', binder: 'bnd_1' });
  });

  it('lands in a session through Session.fromOutside', async () => {
    const d = door({ json: signedIn('usr_ada', 'spc_ada', { via: 'oauth2:github' }) });
    const s = await Session.fromOutside(options(d), { method: 'github', handoff: 'h', binder: 'b' });
    expect(s.via).toBe('oauth2:github');
    expect(s.claimProof).toBe('outside');
    expect(s.outsideProved).toBe(false);
  });
});

describe('an emailed code', () => {
  const sent = { challenge: 'chal_1', binder: 'bnd_1' };
  const answer = (extra = {}) => signedIn('usr_ada', 'spc_ada', { via: 'email-otp', device: 'dev_marker', ...extra });

  it('posts the code, keeps the device marker, and knows it can prove by email', async () => {
    const d = door({ json: answer({ token: tokenV6('usr_ada') }) });
    const s = await Session.withEmail(options(d), sent, ' 123456 ', 'ada@example.com');
    expect(d.sent[0]?.body).toEqual({
      method: 'email',
      kind: 'email_code',
      challenge: 'chal_1',
      binder: 'bnd_1',
      code: '123456',
    });
    expect(deviceMarker()).toBe('dev_marker');
    expect(local.data.get('ordo-notes.device')).toBe('dev_marker');
    expect(s.via).toBe('email-otp');
    expect(s.principal).toBe('usr_ada');
    expect(s.claimProof).toBe('email');
  });

  it('throws SecondStepNeeded when the door asks for a second step', async () => {
    const d = door({
      json: { second_step: 'hdl_1', binder: 'b', expires_at: 2000, methods: ['totp'], enrol: false },
    });
    await expect(Session.withEmail(options(d), sent, '123456', 'ada@example.com')).rejects.toBeInstanceOf(
      SecondStepNeeded,
    );
    expect(deviceMarker()).toBeUndefined();
  });

  it('mails a fresh code to the same address, sending the device marker', async () => {
    const d = door({ json: answer() }, { json: { challenge: 'c2', binder: 'b2' } });
    const s = await Session.withEmail(options(d), sent, '123456', 'ada@example.com');
    const next = await s.sendClaimCode('turn_tok');
    expect(next).toEqual({ challenge: 'c2', binder: 'b2' });
    expect(d.sent[1]?.body).toEqual({
      method: 'email',
      identifier: 'ada@example.com',
      human: 'turn_tok',
      device: 'dev_marker',
    });
  });
});

describe('continuing as a guest', () => {
  it('starts a new guest when no secret is kept, and keeps the secret', async () => {
    const d = door({ json: signedIn('usr_g', 'spc_g', { guest_secret: 'gs_secret', via: 'guest' }) });
    const s = await Session.asGuest(options(d), 'turn_tok');
    expect(d.sent[0]?.body).toEqual({ method: 'guest', kind: 'new', human: 'turn_tok' });
    expect(guestSecret()).toBe('gs_secret');
    expect(s.via).toBe('guest');
    expect(s.claimProof).toBe('guest');
  });

  it('resumes the guest when a secret is kept, and starts nothing new', async () => {
    keepGuestSecret('gs_kept');
    const d = door({ json: signedIn('usr_g', 'spc_g', { via: 'guest' }) });
    await Session.asGuest(options(d));
    expect(paths(d)).toEqual(['/auth/complete']);
    expect(d.sent[0]?.body).toEqual({ method: 'guest', kind: 'resume', secret: 'gs_kept' });
    expect(guestSecret()).toBe('gs_kept');
  });
});

describe('adding a passkey', () => {
  /** Answers each route as the door does. */
  const byPath =
    (routes: Record<string, Reply>) =>
    (s: Sent): Reply =>
      routes[new URL(s.url).pathname] ?? { status: 404 };

  it('links a guest by claim and forgets the guest secret afterwards', async () => {
    keepGuestSecret('gs_kept');
    const d = door(
      { json: signedIn('usr_g', 'spc_g', { via: 'guest' }) },
      byPath({
        '/auth/claim/begin': { json: { claim: 'clm_1', expires_at: 5000 } },
        '/auth/claim/passkey': { json: CREATION_OPTIONS },
        '/auth/claim/complete': { json: { linked: 'pk_1', via: 'passkey' } },
      }),
    );
    const s = await Session.asGuest(options(d));
    await s.addPasskey();
    expect(paths(d).slice(1)).toEqual(['/auth/claim/begin', '/auth/claim/passkey', '/auth/claim/complete']);
    expect(d.sent[1]?.body).toEqual({ method: 'guest', kind: 'resume', secret: 'gs_kept' });
    expect(d.sent[2]?.body).toEqual({ claim: 'clm_1' });
    expect(d.sent[3]?.body).toMatchObject({ method: 'passkey', kind: 'passkey_registration', claim: 'clm_1' });
    expect(d.sent[1]?.headers['Authorization']).toMatch(/^Bearer /);
    expect(guestSecret()).toBeNull();
  });

  it('refuses when the session has no proof to give', async () => {
    const d = door({ json: signedIn('usr_ada', 'spc_ada', { via: 'oauth2:github' }) });
    const s = await Session.fromOutside(options(d), { method: 'github', handoff: 'h', binder: 'b' });
    await expect(s.addPasskey()).rejects.toThrow(/cannot add a passkey/);
    expect(d.sent).toHaveLength(1);
  });
});

describe('adding a passkey to an account that came in through an outside connection', () => {
  const byPath =
    (routes: Record<string, Reply>) =>
    (s: Sent): Reply =>
      routes[new URL(s.url).pathname] ?? { status: 404 };

  it('keeps the session for the trip, takes the code as the first proof, and links the passkey', async () => {
    // Trip out: signed in through Discord, the person presses the button.
    const d = door(
      { json: signedIn('usr_ada', 'spc_ada', { via: 'oauth2:discord' }) },
      byPath({
        '/auth/begin': { json: { redirect: 'https://discord.example/authorize', binder: 'bnd_7' } },
        '/auth/claim/begin': { json: { claim: 'clm_2', expires_at: 5000 } },
        '/auth/claim/passkey': { json: CREATION_OPTIONS },
        '/auth/claim/complete': { json: { linked: 'pk_2', via: 'passkey' } },
      }),
    );
    const before = await Session.fromOutside(options(d), { method: 'discord', handoff: 'h0', binder: 'b0' });
    expect(before.claimProof).toBe('outside');
    const redirect = await before.beginOutsideClaim('https://notes.example/', 'tok_h', '#x');
    expect(redirect).toBe('https://discord.example/authorize');
    expect(d.sent[1]?.body).toEqual({
      method: 'discord',
      return_to: 'https://notes.example/',
      human: 'tok_h',
    });
    // What waits in sessionStorage is the session and the binder, never in an address.
    const kept = (g['sessionStorage'] as Store).getItem('ordo-notes.outside') ?? '';
    expect(kept).toContain('bnd_7');
    expect(JSON.parse(kept).carried).toMatchObject({ space: 'spc_ada', via: 'oauth2:discord' });
    expect((g['localStorage'] as Store).getItem('ordo-notes.outside')).toBeNull();

    // Trip back: the address carries the code. The page must not sign in with it.
    const back = returnedFrom('?handoff=code_3');
    if (back.kind !== 'handoff' || !back.carried) throw new Error('expected a carried hand-off');
    expect((g['sessionStorage'] as Store).getItem('ordo-notes.outside')).toBeNull();
    const after = Session.resumeForClaim(options(d), back.carried, back);
    expect(after.principal).toBe('usr_ada');
    expect(after.space).toBe('spc_ada');
    expect(after.outsideProved).toBe(true);
    const sent = d.sent.length;
    await after.addPasskey();
    const paths2 = d.sent.slice(sent).map((s) => new URL(s.url).pathname);
    expect(paths2).toEqual(['/auth/claim/begin', '/auth/claim/passkey', '/auth/claim/complete']);
    expect(d.sent[sent]?.body).toEqual({ method: 'discord', kind: 'handoff', handoff: 'code_3', binder: 'bnd_7' });
    expect(d.sent[sent]?.headers['Authorization']).toMatch(/^Bearer /);
    expect(paths2).not.toContain('/auth/complete');
    // The code is good once.
    expect(after.outsideProved).toBe(false);
  });

  it('comes back signed in, with no proof, when the provider sent an error', async () => {
    const d = door({ json: signedIn('usr_ada', 'spc_ada', { via: 'oauth2:discord' }) }, {
      json: { redirect: 'https://discord.example/authorize', binder: 'bnd_8' },
    });
    const before = await Session.fromOutside(options(d), { method: 'discord', handoff: 'h0', binder: 'b0' });
    await before.beginOutsideClaim('https://notes.example/');
    const back = returnedFrom('?error=access_denied');
    if (back.kind !== 'error' || !back.carried) throw new Error('expected a carried error');
    const after = Session.resumeForClaim(options(d), back.carried, null);
    expect(after.space).toBe('spc_ada');
    expect(after.outsideProved).toBe(false);
    await expect(after.addPasskey()).rejects.toThrow(/cannot add a passkey/);
  });

  it('writes nothing for the trip when the person only signs in', async () => {
    const d = door({ json: { redirect: 'https://discord.example/authorize', binder: 'bnd_9' } });
    await beginOutside(endpoint(d), 'discord', 'https://notes.example/');
    const kept = (g['sessionStorage'] as Store).getItem('ordo-notes.outside') ?? '';
    expect(JSON.parse(kept).carried).toBeUndefined();
    const back = returnedFrom('?handoff=c');
    expect(back.kind === 'handoff' && back.carried).toBeFalsy();
  });

  it('does not offer the trip to a session that did not come in through a connection', async () => {
    const d = door({ json: {} });
    const s = Session.fromToken(options(d), tokenV6('usr_ada'), 'spc_ada');
    expect(s.outsideConnection).toBeNull();
    await expect(s.beginOutsideClaim('https://notes.example/')).rejects.toThrow(/outside connection/);
  });
});

describe('whether the account holds a passkey', () => {
  const rows = (...r: Array<Record<string, unknown>>): Reply => ({ json: { authenticators: r } });

  it('is true with an active passkey', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: signedIn() }, rows({ id: 'a1', kind: 'passkey', state: 'active' }));
    const s = await Session.signIn(options(d));
    expect(await s.hasPasskey()).toBe(true);
    const asked = d.sent.at(-1);
    expect(asked?.method).toBe('GET');
    expect(new URL(asked?.url ?? '').pathname).toBe('/auth/authenticators');
  });

  it('is false with only other kinds of rows', async () => {
    const d = door(
      { json: REQUEST_OPTIONS },
      { json: signedIn() },
      rows({ id: 'a1', kind: 'other', state: 'active' }, { id: 'a2', kind: 'passkey', state: 'pending' }),
    );
    const s = await Session.signIn(options(d));
    expect(await s.hasPasskey()).toBe(false);
  });
});
