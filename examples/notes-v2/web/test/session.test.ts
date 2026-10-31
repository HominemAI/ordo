/**
 * Signing up and in, the token held in memory, and what the page does when
 * the door is a moment behind.
 */

import { describe, expect, it } from 'vitest';
import { OrdoRefusal } from '@hominemai/ordo-sdk/core/refusal.js';
import { NoTenantError, Session, SessionEnded, untilServed } from '../src/session.js';
import { principalOf } from '../src/token.js';
import {
  CREATION_OPTIONS,
  REQUEST_OPTIONS,
  clientOver,
  credentials,
  door,
  refusal,
  signedIn,
  tokenFor,
} from './support.js';

const options = (d: ReturnType<typeof door>, now = () => 1000) => ({
  baseUrl: 'https://door.example/',
  fetch: d.fetch,
  credentials,
  now,
});

describe('the token', () => {
  it('names the person the door wrote into it', () => {
    expect(principalOf(tokenFor('usr_abc123'))).toBe('usr_abc123');
  });

  it('is refused when it is not a token', () => {
    expect(() => principalOf('nonsense')).toThrow();
  });
});

describe('signing up', () => {
  it('registers a passkey and lands in the tenant the door named', async () => {
    const d = door({ json: CREATION_OPTIONS }, { json: signedIn() });
    const session = await Session.signUp(options(d), 'Ada');
    expect(session.space).toBe('spc_ada');
    expect(session.principal).toBe('usr_ada');
    expect(d.sent.map((s) => new URL(s.url).pathname)).toEqual(['/auth/register', '/auth/complete']);
    expect(d.sent[0]?.body).toEqual({ method: 'passkey', display: 'Ada' });
    expect(d.sent[1]?.body).toMatchObject({ kind: 'passkey_registration', display: 'Ada' });
    // The client it makes is bound to that tenant.
    const client = session.client();
    expect(client.space).toBe('spc_ada');
  });

  it('says so when the deployment gave the person no tenant', async () => {
    const d = door({ json: CREATION_OPTIONS }, { json: signedIn('usr_ada', null, { provisioning: 'failed' }) });
    await expect(Session.signUp(options(d), 'Ada')).rejects.toMatchObject({
      name: 'NoTenantError',
      reason: 'failed',
      // The person is real even without a tenant, and a deployment owner
      // signing up first needs this to name themselves.
      principal: 'usr_ada',
    });
  });

  it('waits while a new tenant is set up, showing each step, then lands in it', async () => {
    const d = door(
      { json: CREATION_OPTIONS },
      { json: signedIn('usr_ada', null, { provisioning: 'in progress' }) },
      { json: { state: 'provisioning', space: 'spc_new', step: 'reserved' } },
      { json: { state: 'provisioning', space: 'spc_new', step: 'custody' } },
      { json: { state: 'ready', space: 'spc_new' } },
    );
    const steps: string[] = [];
    const session = await Session.signUp(
      { ...options(d), onSettingUp: (s) => steps.push(s.step ?? s.state), settingUpPause: async () => {} },
      'Ada',
    );
    expect(session.space).toBe('spc_new');
    expect(steps).toEqual(['reserved', 'custody', 'ready']);
    expect(d.sent.slice(2).every((s) => new URL(s.url).pathname === '/auth/provisioning')).toBe(true);
  });

  it('says why when setting up the tenant stopped', async () => {
    const reason = 'setting up your account stopped while its database was being made';
    const d = door(
      { json: CREATION_OPTIONS },
      { json: signedIn('usr_ada', null, { provisioning: 'in progress' }) },
      { json: { state: 'failed', space: 'spc_new', reason } },
    );
    const e = await Session.signUp({ ...options(d), settingUpPause: async () => {} }, 'Ada').catch((x: unknown) => x);
    expect(e).toBeInstanceOf(NoTenantError);
    expect((e as NoTenantError).reason).toBe('failed');
    expect((e as Error).message).toMatch(/^Setting up your account stopped while its database was being made\./);
    expect((e as Error).message).toMatch(/usr_ada/);
  });

  it('signs in with the passkey alone and lands in the same tenant', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: signedIn() });
    const session = await Session.signIn(options(d));
    expect(session.space).toBe('spc_ada');
    expect(d.sent.map((s) => new URL(s.url).pathname)).toEqual(['/auth/begin', '/auth/complete']);
  });
});

describe('holding the session', () => {
  it('keeps the token until a minute before it ends, then renews it once for every caller', async () => {
    let now = 1000;
    const renewed = { token: tokenFor('usr_ada', 2), expires_at: 8000, session_expires_at: 90000 };
    const d = door({ json: REQUEST_OPTIONS }, { json: signedIn() }, { json: renewed });
    const session = await Session.signIn(options(d, () => now));
    const first = await session.token();
    expect(first).toBe(signedIn().token);
    expect(d.sent).toHaveLength(2);

    now = 3950; // 50 s before the end
    const [a, b, c] = await Promise.all([session.token(), session.token(), session.token()]);
    expect([a, b, c]).toEqual([renewed.token, renewed.token, renewed.token]);
    expect(d.sent.filter((s) => s.url.endsWith('/auth/refresh'))).toHaveLength(1);
    expect(d.sent.at(-1)?.headers['Authorization']).toBe(`Bearer ${first}`);
  });

  it('is over when the door refuses the renewal, and says it once', async () => {
    let now = 1000;
    const d = door({ json: REQUEST_OPTIONS }, { json: signedIn() }, { status: 401 });
    const session = await Session.signIn(options(d, () => now));
    let ended = 0;
    session.onEnd(() => ended++);
    now = 3990;
    await expect(session.token()).rejects.toBeInstanceOf(SessionEnded);
    await expect(session.token()).rejects.toBeInstanceOf(SessionEnded);
    expect(ended).toBe(1);
    expect(session.ended).toBe(true);
  });

  it('is not over when there is only no network', async () => {
    let now = 1000;
    let reachable = false;
    const d = door({ json: REQUEST_OPTIONS }, { json: signedIn() }, () => {
      reachable = true;
      return { status: 200, json: { token: tokenFor('usr_ada', 3), expires_at: 9000, session_expires_at: 90000 } };
    });
    const session = await Session.signIn({
      ...options(d, () => now),
      fetch: async (input, init) => {
        if (String(input).endsWith('/auth/refresh') && !reachable) {
          reachable = true;
          throw new TypeError('Failed to fetch');
        }
        return d.fetch(input, init);
      },
    });
    now = 3990;
    await expect(session.token()).rejects.toThrow(/could not be reached/);
    expect(session.ended).toBe(false);
    expect(await session.token()).toBe(tokenFor('usr_ada', 3));
  });

  it('signs out at the door, and only then lets go of the token', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: signedIn() }, { status: 204 });
    const session = await Session.signIn(options(d));
    await session.signOut();
    const out = d.sent.at(-1);
    expect(out?.url).toBe('https://door.example/auth/logout');
    expect(out?.headers['Authorization']).toBe(`Bearer ${signedIn().token}`);
    expect(session.ended).toBe(true);
    await expect(session.token()).rejects.toBeInstanceOf(SessionEnded);
  });

  it('stays signed in, and says so, when the door could not end the session', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: signedIn() }, { status: 503 });
    const session = await Session.signIn(options(d));
    await expect(session.signOut()).rejects.toThrow(/still signed in/);
    expect(session.ended).toBe(false);
  });

  it('holds a token pasted from `ordo dev` without asking the door to renew or end it', async () => {
    const d = door({ status: 404 });
    const session = Session.fromToken(options(d), tokenFor('mem_dev'), 'dev');
    expect(session.space).toBe('dev');
    expect(session.principal).toBe('mem_dev');
    expect(await session.token()).toBe(tokenFor('mem_dev'));
    await session.signOut();
    expect(session.ended).toBe(true);
    expect(d.sent).toEqual([]);
  });

  it('never writes the token to storage', async () => {
    const seen: string[] = [];
    const store = {
      setItem: (k: string, v: string) => seen.push(`${k}=${v}`),
      getItem: () => null,
    };
    const g = globalThis as Record<string, unknown>;
    g['localStorage'] = store;
    g['sessionStorage'] = store;
    try {
      const d = door({ json: REQUEST_OPTIONS }, { json: signedIn() });
      await Session.signIn(options(d));
    } finally {
      delete g['localStorage'];
      delete g['sessionStorage'];
    }
    expect(seen).toEqual([]);
  });
});

describe('a door that is a moment behind', () => {
  it('waits out access_denied on a tenant made a moment ago, then goes on', async () => {
    const d = door(refusal(403, 'access_denied'), refusal(403, 'access_denied'), { json: { rows: [] } });
    const waits: number[] = [];
    await untilServed(clientOver(d), { wait: async (ms) => void waits.push(ms) });
    expect(d.sent).toHaveLength(3);
    expect(waits).toEqual([150, 150]);
  });

  it('believes a refusal that goes on', async () => {
    const d = door(refusal(403, 'access_denied'));
    await expect(untilServed(clientOver(d), { attempts: 3, wait: async () => {} })).rejects.toBeInstanceOf(
      OrdoRefusal,
    );
    expect(d.sent).toHaveLength(3);
  });

  it('does not wait on any other refusal', async () => {
    const d = door(refusal(500, 'internal'));
    await expect(untilServed(clientOver(d), { wait: async () => {} })).rejects.toBeInstanceOf(OrdoRefusal);
    expect(d.sent).toHaveLength(1);
  });
});

describe('a platform op that wants a fresh touch', () => {
  // The provider's `stepUp` is the session's: `useCommit` calls it when the
  // door refuses a platform write, and tries the write once more
  // (`clients/ordo-react/test/platform.test.ts`).
  it('asks for a passkey touch and takes the token the door re-mints', async () => {
    const d = door(
      { json: REQUEST_OPTIONS },
      { json: signedIn() },
      // step-up begin, then the step-up itself
      { json: REQUEST_OPTIONS },
      { json: { token: tokenFor('usr_ada', 5), expires_at: 4500, session_expires_at: 90000 } },
    );
    const session = await Session.signIn(options(d));
    await session.stepUp();
    expect(d.sent.map((s) => new URL(s.url).pathname).slice(2)).toEqual(['/auth/step-up/begin', '/auth/step-up']);
    expect(await session.token()).toBe(tokenFor('usr_ada', 5));
  });
});

describe('a reload', () => {
  it('resumes the sign-in the browser holds, in the same tenant', async () => {
    const d = door({ json: signedIn('usr_ada', 'spc_ada', { resumed: true, via: 'passkey' }) });
    const session = await Session.resume(options(d));
    expect(session?.space).toBe('spc_ada');
    expect(session?.principal).toBe('usr_ada');
    expect(d.sent.map((s) => new URL(s.url).pathname)).toEqual(['/auth/refresh']);
    expect(d.sent[0]?.headers['Authorization']).toBeUndefined();
  });

  it('answers null when there is nothing to resume, so the sign-in screen shows', async () => {
    const d = door(refusal(401, 'authentication_failed'));
    expect(await Session.resume(options(d))).toBeNull();
  });

  it('asks which tenant when the resumed sign-in named none', async () => {
    const d = door(
      { json: signedIn('usr_ada', null, { resumed: true }) },
      { json: { state: 'ready', space: 'spc_ada' } },
    );
    const session = await Session.resume(options(d));
    expect(session?.space).toBe('spc_ada');
  });
});
