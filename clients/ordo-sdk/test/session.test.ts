/**
 * The session object (`createOrdoAuth`) and the 2 calls it added, against a
 * mock door: renewal before expiry, falling back to the renewal cookie,
 * sign-out, the display name on email sign-up, and the token getter the
 * clients take.
 */

import { describe, expect, it } from 'vitest';
import { PasskeyError } from '../src/auth/passkey.js';
import { completeEmailSignIn } from '../src/auth/email.js';
import { createOrdoAuth, refreshSession, signOut } from '../src/auth/session.js';
import { OrdoClient } from '../src/core/client.js';

interface Sent {
  readonly path: string;
  readonly auth: string | undefined;
  readonly credentials: RequestCredentials | undefined;
  readonly body: Record<string, unknown> | null;
}

type Reply = { status?: number; json?: unknown };

/** A door that answers by path, each path from its own queue. */
function door(routes: Record<string, Reply[]>) {
  const sent: Sent[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const path = new URL(String(input)).pathname;
    const headers = (init?.headers ?? {}) as Record<string, string>;
    const raw = init?.body;
    sent.push({
      path,
      auth: headers['Authorization'],
      credentials: init?.credentials,
      body: typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null,
    });
    const queue = routes[path] ?? [];
    const r = (queue.length > 1 ? queue.shift() : queue[0]) ?? { status: 404 };
    const status = r.status ?? 200;
    return new Response(r.json === undefined || status === 204 ? null : JSON.stringify(r.json), { status });
  };
  return { sent, fetch };
}

const wire = (token: string, expires: number) => ({
  token,
  expires_at: expires,
  session_expires_at: expires + 10_000,
});
const REFUSED = { status: 401, json: { error: { code: 'authentication_failed', message: 'x' } } };
const SENT = { challenge: 'ch_1', binder: 'b'.repeat(64) };

/** Seconds since the epoch, settable. */
function clock(start = 1_000) {
  let s = start;
  return { now: () => s * 1000, at: (t: number) => (s = t) };
}

describe('refreshSession and signOut', () => {
  it('refresh posts the bearer token and answers the new session', async () => {
    const d = door({ '/auth/refresh': [{ json: wire('tok_2', 5000) }] });
    const s = await refreshSession({ baseUrl: 'https://door.example', fetch: d.fetch }, 'tok_1');
    expect(s.token).toBe('tok_2');
    expect(s.expiresAt).toBe(5000);
    expect(d.sent[0]?.auth).toBe('Bearer tok_1');
  });

  it('a refused refresh is a PasskeyError refused', async () => {
    const d = door({ '/auth/refresh': [REFUSED] });
    await expect(refreshSession({ baseUrl: 'https://door.example', fetch: d.fetch }, 't')).rejects.toMatchObject({
      failure: 'refused',
    });
  });

  it('sign-out posts the token with credentials, so the cookie is cleared', async () => {
    const d = door({ '/auth/logout': [{ status: 204 }] });
    await signOut({ baseUrl: 'https://door.example', fetch: d.fetch }, 'tok_1');
    expect(d.sent[0]?.path).toBe('/auth/logout');
    expect(d.sent[0]?.auth).toBe('Bearer tok_1');
    expect(d.sent[0]?.credentials).toBe('include');
  });

  it('a sign-out the door could not record throws unavailable', async () => {
    const d = door({ '/auth/logout': [{ status: 503, json: { error: { code: 'unavailable', message: 'x' } } }] });
    await expect(signOut({ baseUrl: 'https://door.example', fetch: d.fetch }, 't')).rejects.toMatchObject({
      failure: 'unavailable',
    });
  });
});

describe('completeEmailSignIn takes a display name', () => {
  it('sends display, trimmed, and only when given', async () => {
    const d = door({ '/auth/complete': [{ json: wire('tok', 9000) }] });
    const endpoint = { baseUrl: 'https://door.example', fetch: d.fetch };
    await completeEmailSignIn(endpoint, SENT, '123', { display: '  Ana  ' });
    await completeEmailSignIn(endpoint, SENT, '123');
    expect(d.sent[0]?.body?.['display']).toBe('Ana');
    expect(d.sent[1]?.body).not.toHaveProperty('display');
  });
});

describe('createOrdoAuth', () => {
  it('resumes on load and hands out the token while it is fresh', async () => {
    const c = clock();
    const d = door({ '/auth/refresh': [{ json: wire('tok_r', 4600) }] });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch, now: c.now, autoRenew: false });
    const seen: Array<string | null> = [];
    auth.subscribe((s) => seen.push(s?.token ?? null));
    expect((await auth.resume())?.token).toBe('tok_r');
    expect(await auth.token()).toBe('tok_r');
    expect(d.sent).toHaveLength(1);
    expect(d.sent[0]?.auth).toBeUndefined();
    expect(seen).toEqual(['tok_r']);
  });

  it('renews with the bearer token near expiry, once for many callers', async () => {
    const c = clock();
    const d = door({
      '/auth/complete': [{ json: wire('tok_1', 1_100) }],
      '/auth/refresh': [{ json: wire('tok_2', 5_000) }],
    });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch, now: c.now, autoRenew: false });
    const r = await auth.completeEmail(SENT, '123', { display: 'Ana' });
    expect(r.kind).toBe('session');
    // 100 s left, under the 60 s margin only after 41 s.
    c.at(1_050);
    const [a, b] = await Promise.all([auth.token(), auth.token()]);
    expect([a, b]).toEqual(['tok_2', 'tok_2']);
    const refreshes = d.sent.filter((s) => s.path === '/auth/refresh');
    expect(refreshes).toHaveLength(1);
    expect(refreshes[0]?.auth).toBe('Bearer tok_1');
    expect(auth.session()?.sessionExpiresAt).toBe(15_000);
  });

  it('falls back to the renewal cookie when the token expired, and signs out when that is gone too', async () => {
    const c = clock();
    const d = door({
      '/auth/complete': [{ json: wire('tok_1', 1_100) }],
      // The cookie continues the sign-in once, then nothing is left.
      '/auth/refresh': [{ json: wire('tok_c', 2_000) }, REFUSED],
    });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch, now: c.now, autoRenew: false });
    await auth.completeEmail(SENT, '123');
    c.at(1_200); // expired: no bearer refresh, the cookie straight away
    expect(await auth.token()).toBe('tok_c');
    expect(d.sent.at(-1)?.auth).toBeUndefined();
    c.at(2_500);
    await expect(auth.token()).rejects.toBeInstanceOf(PasskeyError);
    expect(auth.session()).toBeNull();
  });

  it('a bearer refresh refused tries the cookie before signing out', async () => {
    const c = clock();
    const d = door({
      '/auth/complete': [{ json: wire('tok_1', 1_030) }],
      '/auth/refresh': [REFUSED, { json: wire('tok_c', 9_000) }],
    });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch, now: c.now, autoRenew: false });
    await auth.completeEmail(SENT, '123');
    expect(await auth.renew()).toBe(true);
    expect(await auth.token()).toBe('tok_c');
  });

  it('a door that cannot be reached throws and keeps the session', async () => {
    const c = clock();
    let down = false;
    const d = door({ '/auth/complete': [{ json: wire('tok_1', 1_030) }] });
    const fetch: typeof globalThis.fetch = (input, init) =>
      down ? Promise.reject(new TypeError('Failed to fetch')) : d.fetch(input, init);
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch, now: c.now, autoRenew: false });
    await auth.completeEmail(SENT, '123');
    down = true;
    await expect(auth.token()).rejects.toThrow();
    expect(auth.session()?.token).toBe('tok_1');
  });

  it('signs out at the door and here', async () => {
    const d = door({ '/auth/complete': [{ json: wire('tok_1', 9_000) }], '/auth/logout': [{ status: 204 }] });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch, now: clock().now, autoRenew: false });
    await auth.completeEmail(SENT, '123');
    const seen: Array<string | null> = [];
    auth.subscribe((s) => seen.push(s?.token ?? null));
    await auth.signOut();
    expect(d.sent.at(-1)?.path).toBe('/auth/logout');
    expect(auth.session()).toBeNull();
    expect(seen).toEqual([null]);
    await expect(auth.token()).rejects.toMatchObject({ failure: 'refused' });
  });

  it('keeps the session when the sign-out was not recorded', async () => {
    const d = door({
      '/auth/complete': [{ json: wire('tok_1', 9_000) }],
      '/auth/logout': [{ status: 503, json: { error: { code: 'unavailable', message: 'x' } } }],
    });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch, now: clock().now, autoRenew: false });
    await auth.completeEmail(SENT, '123');
    await expect(auth.signOut()).rejects.toMatchObject({ failure: 'unavailable' });
    expect(auth.session()?.token).toBe('tok_1');
  });

  it('a second step is left to the page and keeps nobody signed in', async () => {
    const step = { second_step: 'h', binder: 'b', expires_at: 1, methods: ['totp'] };
    const d = door({ '/auth/complete': [{ json: step }] });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch, autoRenew: false });
    const r = await auth.completeEmail(SENT, '123');
    expect(r.kind).toBe('second-step');
    expect(auth.session()).toBeNull();
  });

  it('lists tenants and plugs its token into OrdoClient', async () => {
    const d = door({
      '/auth/complete': [{ json: wire('tok_1', 9_000) }],
      '/auth/spaces': [{ json: [{ space: 'spc_a', name: 'A', role: 'owner', via: 'member' }] }],
      '/auth/refresh': [{ json: wire('tok_2', 9_500) }],
    });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch, now: clock().now, autoRenew: false });
    await auth.completeEmail(SENT, '123');
    expect(await auth.spaces()).toEqual([{ space: 'spc_a', name: 'A', role: 'owner', via: 'member' }]);
    expect(d.sent.at(-1)?.auth).toBe('Bearer tok_1');
    const client = new OrdoClient({ ...auth.clientOptions(), space: 'spc_a', fetch: d.fetch });
    expect(await client.renewToken()).toBe(true);
  });

  it('renews on a timer ahead of expiry', async () => {
    const d = door({
      '/auth/complete': [{ json: wire('tok_1', Date.now() / 1000 + 60.05) }],
      '/auth/refresh': [{ json: wire('tok_2', Date.now() / 1000 + 3_600) }],
    });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch });
    await auth.completeEmail(SENT, '123');
    await new Promise((r) => setTimeout(r, 300));
    expect(auth.session()?.token).toBe('tok_2');
    auth.use(null); // stops the timer
  });
});
