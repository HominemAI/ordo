/**
 * The sign-in methods list, against a mock door.
 *
 * What is proved is what this package owns: the request (a GET, no body, no
 * token, the tenant in the query), the reading of the answer, and that a bad
 * answer or an unreachable door rejects as `PasskeyError`.
 */

import { describe, expect, it } from 'vitest';
import { OrdoNetworkError } from '../src/core/network.js';
import type { PasskeyEndpoint } from '../src/auth/passkey.js';
import { signInMethods } from '../src/auth/methods.js';

interface Sent {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

function door(reply: { status?: number; json?: unknown } | 'down') {
  const sent: Sent[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    sent.push({
      url: String(input),
      method: String(init?.method ?? 'GET'),
      headers: (init?.headers ?? {}) as Record<string, string>,
      body: init?.body ?? null,
    });
    if (reply === 'down') throw new TypeError('offline');
    return new Response(reply.json === undefined ? null : JSON.stringify(reply.json), {
      status: reply.status ?? 200,
    });
  };
  const endpoint: PasskeyEndpoint = { baseUrl: 'https://door.example/', fetch };
  return { sent, endpoint };
}

const PLAIN = {
  passkey: true,
  password: false,
  email: true,
  guest: false,
  humanCheck: null,
  connections: [],
};

describe('signInMethods', () => {
  it('asks GET /auth/methods with no body and no token', async () => {
    const d = door({ json: PLAIN });
    const got = await signInMethods(d.endpoint);
    expect(d.sent).toHaveLength(1);
    expect(d.sent[0]?.url).toBe('https://door.example/auth/methods');
    expect(d.sent[0]?.method).toBe('GET');
    expect(d.sent[0]?.body).toBeNull();
    expect(d.sent[0]?.headers['Authorization']).toBeUndefined();
    expect(got).toEqual(PLAIN);
  });

  it('reads the human check with its site key, and without one', async () => {
    const withKey = await signInMethods(
      door({ json: { ...PLAIN, humanCheck: { kind: 'turnstile', siteKey: '0x4AAA' } } }).endpoint,
    );
    expect(withKey.humanCheck).toEqual({ kind: 'turnstile', siteKey: '0x4AAA' });
    const bare = await signInMethods(door({ json: { ...PLAIN, humanCheck: { kind: 'turnstile' } } }).endpoint);
    expect(bare.humanCheck).toEqual({ kind: 'turnstile' });
  });

  it('reads connections and marks a tenant provider', async () => {
    const d = door({
      json: {
        ...PLAIN,
        connections: [
          { name: 'discord', label: 'Discord', kind: 'oauth2' },
          { name: 'acme-sso', label: 'acme-sso', kind: 'oidc', tenant: true },
        ],
      },
    });
    const got = await signInMethods(d.endpoint, { tenant: 'spc_acme/1' });
    expect(d.sent[0]?.url).toBe('https://door.example/auth/methods?tenant=spc_acme%2F1');
    expect(got.connections).toEqual([
      { name: 'discord', label: 'Discord', kind: 'oauth2', tenant: false },
      { name: 'acme-sso', label: 'acme-sso', kind: 'oidc', tenant: true },
    ]);
  });

  it('rejects an answer that is not the list', async () => {
    for (const json of [
      { ...PLAIN, passkey: 'yes' },
      { ...PLAIN, connections: 'none' },
      { ...PLAIN, connections: [{ name: 'x', label: 'X', kind: 'saml' }] },
      { ...PLAIN, humanCheck: { kind: 'recaptcha' } },
    ]) {
      await expect(signInMethods(door({ json }).endpoint)).rejects.toMatchObject({
        failure: 'malformed',
      });
    }
  });

  it('rejects as unreachable when the door cannot be reached', async () => {
    const err = await signInMethods(door('down').endpoint).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect(err).toMatchObject({ kind: 'unreachable' });
  });
});
