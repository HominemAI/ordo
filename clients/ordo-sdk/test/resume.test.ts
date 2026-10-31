/**
 * Resuming a sign-in on load (owner ruling 2026-10-21), against a mock door.
 *
 * What this package owns: the call is `POST /auth/refresh` with credentials
 * and no token, a session comes back as any sign-in's does, nothing to resume
 * is `null` (with the reason when the door gave one), and a door that cannot
 * answer is an error rather than a signed-out person.
 */

import { describe, expect, it } from 'vitest';
import { PasskeyError, call } from '../src/auth/passkey.js';
import { resumeSession } from '../src/auth/resume.js';

function door(status: number, json?: unknown) {
  const sent: Array<{ url: string; init: RequestInit | undefined }> = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    sent.push({ url: String(input), init });
    return new Response(json === undefined ? null : JSON.stringify(json), { status });
  };
  return { sent, fetch };
}

const SESSION = {
  token: 'tok_back',
  via: 'passkey',
  space: 'spc_ana',
  expires_at: 1000,
  session_expires_at: 2000,
  resumed: true,
};

describe('resumeSession', () => {
  it('posts to /auth/refresh with credentials and no token', async () => {
    const d = door(200, SESSION);
    const done = await resumeSession({ baseUrl: 'https://door.example/', fetch: d.fetch });
    expect(d.sent).toHaveLength(1);
    expect(d.sent[0]?.url).toBe('https://door.example/auth/refresh');
    expect(d.sent[0]?.init?.method).toBe('POST');
    expect(d.sent[0]?.init?.credentials).toBe('include');
    const headers = (d.sent[0]?.init?.headers ?? {}) as Record<string, string>;
    expect(headers['Authorization']).toBeUndefined();
    expect(d.sent[0]?.init?.body).toBeUndefined();
    expect(done?.token).toBe('tok_back');
    expect(done?.expiresAt).toBe(1000);
    expect(done?.sessionExpiresAt).toBe(2000);
    expect(done?.raw['space']).toBe('spc_ana');
  });

  it('answers null when there is nothing to resume', async () => {
    const d = door(401, { error: { code: 'authentication_failed', message: 'that did not authenticate' } });
    const misses: string[] = [];
    const done = await resumeSession({ baseUrl: 'https://door.example', fetch: d.fetch }, {
      onMiss: (why) => misses.push(why),
    });
    expect(done).toBeNull();
    expect(misses).toEqual(['none']);
  });

  it('passes on the setting when the deployment turned it off', async () => {
    const message = 'the deployment owner sets ORDO_REMEMBER=on to allow it';
    const d = door(401, { error: { code: 'remember_off', message } });
    const said: Array<[string, string | undefined]> = [];
    const done = await resumeSession({ baseUrl: 'https://door.example', fetch: d.fetch }, {
      onMiss: (why, m) => said.push([why, m]),
    });
    expect(done).toBeNull();
    expect(said).toEqual([['remember_off', message]]);
  });

  it('throws when the door cannot check, rather than signing the person out', async () => {
    const d = door(503, { error: { code: 'session_store_unavailable' } });
    await expect(resumeSession({ baseUrl: 'https://door.example', fetch: d.fetch })).rejects.toBeInstanceOf(
      PasskeyError,
    );
  });

  it('takes turns with other tabs through a browser lock', async () => {
    const held: string[] = [];
    const nav = globalThis as { navigator?: unknown };
    const before = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
    Object.defineProperty(globalThis, 'navigator', {
      configurable: true,
      value: {
        locks: {
          request: async (name: string, fn: () => Promise<unknown>) => {
            held.push(name);
            return fn();
          },
        },
      },
    });
    try {
      const d = door(200, SESSION);
      const done = await resumeSession({ baseUrl: 'https://door.example', fetch: d.fetch });
      expect(done?.token).toBe('tok_back');
      expect(held).toEqual(['ordo-resume']);
    } finally {
      if (before) Object.defineProperty(globalThis, 'navigator', before);
      else delete nav.navigator;
    }
  });
});

describe('sign-in calls', () => {
  it('ask for credentials, so the renewal cookie a sign-in sets is kept', async () => {
    const d = door(200, SESSION);
    await call({ baseUrl: 'https://door.example', fetch: d.fetch }, '/auth/complete', { kind: 'x' });
    expect(d.sent[0]?.init?.credentials).toBe('include');
  });
});
