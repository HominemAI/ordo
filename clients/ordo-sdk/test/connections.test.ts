/**
 * Tenant-owned sign-in providers, against a mock door.
 *
 * What is proved is what this package owns: the shape of each op request, that
 * the secret is sent and never read back, the typed failures, and the tenant
 * sign-in requests.
 */

import { describe, expect, it } from 'vitest';
import { OrdoNetworkError } from '../src/core/network.js';
import { PasskeyError, type PasskeyEndpoint } from '../src/auth/passkey.js';
import {
  ConnectionError,
  addConnection,
  beginTenantSignIn,
  completeTenantSignIn,
  listConnections,
  retireConnection,
  updateConnection,
  type ConnectionEndpoint,
} from '../src/auth/connections.js';

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
      headers: { ...(init?.headers as Record<string, string>) },
      body: typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null,
    });
    const r = replies[Math.min(i++, replies.length - 1)] ?? { status: 500 };
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), {
      status: r.status ?? 200,
    });
  };
  return { sent, fetch };
}

const ROW = {
  name: 'acme',
  key: 'k1',
  kind: 'oidc',
  state: 'active',
  issuer: 'https://idp.example',
  clientId: 'cid',
  secret: 'set',
  role: 'viewer',
  callback: 'https://door.example/auth/callback/t/sp1/acme',
  outcome: 'added',
};

const opsOf = (fetch: typeof globalThis.fetch): ConnectionEndpoint => ({
  baseUrl: 'https://door.example/',
  space: 'sp1',
  token: 'tok_owner',
  fetch,
});

const endpointOf = (fetch: typeof globalThis.fetch): PasskeyEndpoint => ({
  baseUrl: 'https://door.example',
  fetch,
});

describe('addConnection', () => {
  it('posts the op in the tenant space with the secret, and reads the row', async () => {
    const d = door({ json: { rows: [ROW] } });
    const got = await addConnection(opsOf(d.fetch), {
      name: 'acme',
      kind: 'oidc',
      issuer: 'https://idp.example',
      clientId: 'cid',
      clientSecret: 's3cret',
      role: 'editor',
    });
    const s = d.sent[0];
    expect(s?.url).toBe('https://door.example/op/std.space.addConnection');
    expect(s?.method).toBe('POST');
    expect(s?.headers['X-Ordo-Space']).toBe('sp1');
    expect(s?.headers['Authorization']).toBe('Bearer tok_owner');
    expect(s?.headers['Idempotency-Key']).toBeTruthy();
    expect(s?.body).toEqual({
      name: 'acme',
      kind: 'oidc',
      issuer: 'https://idp.example',
      clientId: 'cid',
      clientSecret: 's3cret',
      role: 'editor',
    });
    expect(got.callback).toBe('https://door.example/auth/callback/t/sp1/acme');
    expect(got.secret).toBe('set');
    expect(got.outcome).toBe('added');
  });
});

describe('updateConnection', () => {
  it('sends the name and only the changes', async () => {
    const d = door({ json: { rows: [{ ...ROW, outcome: 'updated' }] } });
    const got = await updateConnection(opsOf(d.fetch), 'acme', { clientSecret: 'new', scope: 'openid' });
    expect(d.sent[0]?.url).toBe('https://door.example/op/std.space.updateConnection');
    expect(d.sent[0]?.body).toEqual({ name: 'acme', clientSecret: 'new', scope: 'openid' });
    expect(got.outcome).toBe('updated');
  });
});

describe('retireConnection', () => {
  it('sends the name', async () => {
    const d = door({ json: { rows: [{ ...ROW, state: 'retired', outcome: 'retired' }] } });
    const got = await retireConnection(opsOf(d.fetch), 'acme');
    expect(d.sent[0]?.url).toBe('https://door.example/op/std.space.retireConnection');
    expect(d.sent[0]?.body).toEqual({ name: 'acme' });
    expect(got.state).toBe('retired');
  });
});

describe('listConnections', () => {
  it('reads with GET and no body', async () => {
    const d = door({ json: { rows: [ROW, { ...ROW, name: 'b' }] } });
    const rows = await listConnections(opsOf(d.fetch));
    expect(d.sent[0]?.method).toBe('GET');
    expect(d.sent[0]?.url).toBe('https://door.example/op/std.space.connections');
    expect(d.sent[0]?.body).toBeNull();
    expect(d.sent[0]?.headers['Idempotency-Key']).toBeUndefined();
    expect(rows.map((r) => r.name)).toEqual(['acme', 'b']);
  });
});

describe('connection failures', () => {
  it('names the setting when tenant providers are off', async () => {
    const d = door({
      status: 400,
      json: { error: { code: 'tenant_providers_off', message: 'off', setting: 'ORDO_AUTH_TENANT_PROVIDERS' } },
    });
    const err = await listConnections(opsOf(d.fetch)).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ConnectionError);
    expect((err as ConnectionError).failure).toBe('tenant_providers_off');
    expect((err as ConnectionError).setting).toBe('ORDO_AUTH_TENANT_PROVIDERS');
  });

  it.each(['tenant_refused', 'duplicate', 'not_found', 'identity_provider_unavailable'] as const)(
    'types %s',
    async (code) => {
      const d = door({ status: 409, json: { error: { code, message: 'no' } } });
      const err = await retireConnection(opsOf(d.fetch), 'x').catch((e: unknown) => e);
      expect((err as ConnectionError).failure).toBe(code);
      expect((err as ConnectionError).code).toBe(code);
    },
  );

  it('keeps an unknown code as refused', async () => {
    const d = door({ status: 403, json: { error: { code: 'access_denied', message: 'no' } } });
    const err = await retireConnection(opsOf(d.fetch), 'x').catch((e: unknown) => e);
    expect((err as ConnectionError).failure).toBe('refused');
    expect((err as ConnectionError).code).toBe('access_denied');
  });

  it('is unavailable when the door cannot be reached', async () => {
    const fetch: typeof globalThis.fetch = async () => {
      throw new Error('down');
    };
    const err = await listConnections(opsOf(fetch)).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect((err as OrdoNetworkError).kind).toBe('unreachable');
  });
});

describe('tenant sign-in', () => {
  it('begins with method, tenant and return_to, and presence only when asked', async () => {
    const d = door({ json: { redirect: 'https://idp.example/a', binder: 'B' } });
    const got = await beginTenantSignIn(endpointOf(d.fetch), {
      method: 'acme',
      tenant: 'sp1',
      returnTo: 'https://app.example/back',
    });
    expect(d.sent[0]?.url).toBe('https://door.example/auth/begin');
    expect(d.sent[0]?.body).toEqual({
      method: 'acme',
      tenant: 'sp1',
      return_to: 'https://app.example/back',
    });
    expect(got).toEqual({ redirect: 'https://idp.example/a', binder: 'B' });
    await beginTenantSignIn(endpointOf(d.fetch), {
      method: 'acme',
      tenant: 'sp1',
      returnTo: 'https://app.example/back',
      presence: true,
    });
    expect(d.sent[1]?.body?.['presence']).toBe(true);
  });

  it('completes with kind handoff and yields a session', async () => {
    const d = door({
      json: { token: 'tok', via: 'acme', expires_at: 1000, session_expires_at: 2000 },
    });
    const out = await completeTenantSignIn(endpointOf(d.fetch), {
      method: 'acme',
      tenant: 'sp1',
      handoff: 'H',
      binder: 'B',
    });
    expect(d.sent[0]?.url).toBe('https://door.example/auth/complete');
    expect(d.sent[0]?.body).toEqual({
      method: 'acme',
      tenant: 'sp1',
      kind: 'handoff',
      handoff: 'H',
      binder: 'B',
    });
    expect(out.kind).toBe('session');
  });

  it('refuses a missing binder before the wire', async () => {
    const d = door({ json: {} });
    await expect(
      completeTenantSignIn(endpointOf(d.fetch), { method: 'a', tenant: 's', handoff: 'H', binder: '' }),
    ).rejects.toBeInstanceOf(PasskeyError);
    expect(d.sent).toHaveLength(0);
  });

  it('surfaces tenant_providers_off with its setting', async () => {
    const d = door({
      status: 400,
      json: { error: { code: 'tenant_providers_off', setting: 'ORDO_AUTH_TENANT_PROVIDERS' } },
    });
    const err = await beginTenantSignIn(endpointOf(d.fetch), {
      method: 'a',
      tenant: 's',
      returnTo: 'https://x.example',
    }).catch((e: unknown) => e);
    expect((err as PasskeyError).failure).toBe('tenant_providers_off');
    expect((err as PasskeyError).setting).toBe('ORDO_AUTH_TENANT_PROVIDERS');
  });
});
