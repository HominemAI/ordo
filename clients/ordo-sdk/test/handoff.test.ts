/**
 * The subdomain hand-off, against a mock door.
 *
 * What is proved is what this package owns: the binder and its hash, the
 * address the subdomain sends the person to, the shape of both requests, that
 * the binder is cleared and the code stripped, and that a refusal or a missing
 * binder rejects.
 */

import { createHash } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import { PasskeyError } from '../src/auth/passkey.js';
import {
  HANDOFF_BINDER_KEY,
  binderHash,
  finishSubdomainSignIn,
  issueHandoff,
  makeBinder,
  readHandoffRequest,
  startSubdomainSignIn,
} from '../src/auth/handoff.js';

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

function memory() {
  const m = new Map<string, string>();
  return {
    m,
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    removeItem: (k: string) => void m.delete(k),
  };
}

const SESSION = {
  token: 'tok_sub',
  via: 'handoff',
  expires_at: 1000,
  session_expires_at: 2000,
};
const REFUSED = {
  status: 401,
  json: { error: { code: 'authentication_failed', message: 'x' } },
};
const DOOR = 'https://door.example';

describe('binder', () => {
  it('is 43 base64url characters, hashed over its text', async () => {
    const b = makeBinder();
    expect(b).toMatch(/^[A-Za-z0-9_-]{43}$/);
    const want = createHash('sha256').update(b, 'utf8').digest('base64url');
    expect(await binderHash(b)).toBe(want);
  });
});

describe('startSubdomainSignIn', () => {
  it('stores the binder and builds the core address', async () => {
    const s = memory();
    const url = await startSubdomainSignIn({
      coreUrl: 'https://login.example.com/signin',
      returnTo: 'https://app.example.com/notes?x=1&handoff_code=old',
      storage: s,
    });
    const binder = s.m.get(HANDOFF_BINDER_KEY)!;
    expect(binder).toMatch(/^[A-Za-z0-9_-]{43}$/);
    const u = new URL(url);
    expect(u.origin + u.pathname).toBe('https://login.example.com/signin');
    expect(u.searchParams.get('return_to')).toBe('https://app.example.com/notes?x=1');
    expect(u.searchParams.get('binder_hash')).toBe(
      createHash('sha256').update(binder, 'utf8').digest('base64url'),
    );
    expect(url).not.toContain(binder);
  });
});

describe('readHandoffRequest', () => {
  it('reads both parameters, or null', () => {
    expect(
      readHandoffRequest(
        'https://login.example.com/?return_to=https%3A%2F%2Fa.example.com%2F&binder_hash=h',
      ),
    ).toEqual({ returnTo: 'https://a.example.com/', binderHash: 'h' });
    expect(readHandoffRequest('https://login.example.com/?return_to=x')).toBeNull();
    expect(readHandoffRequest('https://login.example.com/')).toBeNull();
  });
});

describe('issueHandoff', () => {
  it('posts the body with the bearer token and returns the redirect', async () => {
    const d = door({
      json: { redirect: 'https://app.example.com/?handoff_code=c1', expires_at: 77 },
    });
    const out = await issueHandoff({
      baseUrl: DOOR,
      fetch: d.fetch,
      token: 'tok_core',
      returnTo: 'https://app.example.com/',
      binderHash: 'hash1',
    });
    expect(out).toEqual({ redirect: 'https://app.example.com/?handoff_code=c1', expiresAt: 77 });
    expect(d.sent).toHaveLength(1);
    expect(d.sent[0]!.url).toBe(`${DOOR}/auth/handoff`);
    expect(d.sent[0]!.method).toBe('POST');
    expect(d.sent[0]!.headers['Authorization']).toBe('Bearer tok_core');
    expect(d.sent[0]!.body).toEqual({
      return_to: 'https://app.example.com/',
      binder_hash: 'hash1',
    });
  });

  it('rejects a 401 as refused and a 503 as unavailable', async () => {
    const args = { baseUrl: DOOR, token: 't', returnTo: 'r', binderHash: 'h' };
    await expect(issueHandoff({ ...args, fetch: door(REFUSED).fetch })).rejects.toMatchObject({
      failure: 'refused',
    });
    const down = door({ status: 503, json: { error: { code: 'credential_store_unavailable' } } });
    await expect(issueHandoff({ ...args, fetch: down.fetch })).rejects.toMatchObject({
      failure: 'unavailable',
    });
  });
});

describe('finishSubdomainSignIn', () => {
  it('redeems with the stored binder, clears it and strips the code', async () => {
    const s = memory();
    s.m.set(HANDOFF_BINDER_KEY, 'binder-text');
    const d = door({ json: SESSION });
    const replaced: unknown[][] = [];
    const got = await finishSubdomainSignIn({
      baseUrl: DOOR,
      fetch: d.fetch,
      url: 'https://app.example.com/notes?x=1&handoff_code=c9#top',
      storage: s,
      history: { replaceState: (...a) => void replaced.push(a) },
    });
    expect(got?.token).toBe('tok_sub');
    expect(got?.expiresAt).toBe(1000);
    expect(got?.sessionExpiresAt).toBe(2000);
    expect(d.sent[0]!.url).toBe(`${DOOR}/auth/handoff/redeem`);
    expect(d.sent[0]!.body).toEqual({ code: 'c9', binder: 'binder-text' });
    expect(s.m.has(HANDOFF_BINDER_KEY)).toBe(false);
    expect(replaced).toEqual([[null, '', '/notes?x=1#top']]);
  });

  it('returns null without a code and does not call the door', async () => {
    const d = door({ json: SESSION });
    const got = await finishSubdomainSignIn({
      baseUrl: DOOR,
      fetch: d.fetch,
      url: 'https://app.example.com/',
      storage: memory(),
    });
    expect(got).toBeNull();
    expect(d.sent).toHaveLength(0);
  });

  it('throws on a refusal', async () => {
    const s = memory();
    s.m.set(HANDOFF_BINDER_KEY, 'b');
    const d = door(REFUSED);
    await expect(
      finishSubdomainSignIn({
        baseUrl: DOOR,
        fetch: d.fetch,
        url: 'https://app.example.com/?handoff_code=c',
        storage: s,
      }),
    ).rejects.toBeInstanceOf(PasskeyError);
  });

  it('throws without calling the door when no binder is kept', async () => {
    const d = door({ json: SESSION });
    await expect(
      finishSubdomainSignIn({
        baseUrl: DOOR,
        fetch: d.fetch,
        url: 'https://app.example.com/?handoff_code=c',
        storage: memory(),
      }),
    ).rejects.toMatchObject({ failure: 'refused' });
    expect(d.sent).toHaveLength(0);
  });
});
