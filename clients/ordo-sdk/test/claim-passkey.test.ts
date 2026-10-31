/**
 * Adding a passkey by claim, against a mock door and a stubbed
 * `navigator.credentials`: the options are asked for with the bearer and the
 * claim, and the attestation is posted as the claimed proof.
 */

import { describe, expect, it } from 'vitest';
import { toBase64Url, type PasskeyEndpoint } from '../src/auth/passkey.js';
import { claimPasskey } from '../src/auth/claim.js';

const bytes = (...b: number[]) => Uint8Array.from(b);
const buf = (u: Uint8Array) => u.buffer.slice(u.byteOffset, u.byteOffset + u.byteLength);

const CHALLENGE = toBase64Url(Uint8Array.from({ length: 32 }, (_, i) => i * 3 + 2));

const CREATION_OPTIONS = {
  challenge: CHALLENGE,
  method: 'passkey',
  rp_id: 'example.com',
  rp_name: 'Example',
  user: { id: toBase64Url(bytes(7, 7)), name: 'Nelly', display_name: 'Nelly' },
  algorithms: [-7, -8],
  user_verification: 'required',
  resident_key: 'required',
  attestation: 'none',
  exclude_credentials: [] as string[],
  timeout_ms: 300000,
};

const ATTESTATION = {
  rawId: buf(bytes(1)),
  response: {
    clientDataJSON: buf(new TextEncoder().encode('{"type":"webauthn.create"}')),
    attestationObject: buf(bytes(163, 99, 255)),
  },
};

interface Sent {
  readonly url: string;
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

function endpoint(d: ReturnType<typeof door>): PasskeyEndpoint {
  return {
    baseUrl: 'https://door.example',
    fetch: d.fetch,
    credentials: {
      get: async () => null,
      create: async () => ATTESTATION as unknown as Credential,
    },
  };
}

describe('claimPasskey', () => {
  it('asks for options with the claim, then posts the attestation as the claimed proof', async () => {
    const d = door(
      { json: CREATION_OPTIONS },
      { json: { linked: 'passkey:AQ', via: 'passkey', authenticators: 2 } },
    );
    const linked = await claimPasskey(endpoint(d), 'tok_session', 'clm_1');
    expect(linked).toEqual({ linked: 'passkey:AQ', via: 'passkey', authenticators: 2, already: false });
    expect(d.sent[0]?.url).toBe('https://door.example/auth/claim/passkey');
    expect(d.sent[0]?.body).toEqual({ claim: 'clm_1' });
    expect(d.sent[1]?.url).toBe('https://door.example/auth/claim/complete');
    expect(d.sent[1]?.body).toMatchObject({
      claim: 'clm_1',
      method: 'passkey',
      kind: 'passkey_registration',
      challenge: CHALLENGE,
      attestation_object: toBase64Url(bytes(163, 99, 255)),
    });
    for (const s of d.sent) {
      const auth = Object.entries(s.headers).find(([k]) => k.toLowerCase() === 'authorization');
      expect(auth?.[1]).toBe('Bearer tok_session');
    }
  });

  it('surfaces linking_off with its setting and creates nothing', async () => {
    const d = door({
      status: 403,
      json: { error: { code: 'linking_off', setting: 'ORDO_AUTH_LINKING', value: 'on' } },
    });
    await expect(claimPasskey(endpoint(d), 't', 'clm_1')).rejects.toMatchObject({
      failure: 'linking_off',
      setting: 'ORDO_AUTH_LINKING',
    });
    expect(d.sent).toHaveLength(1);
  });

  it('a closed claim is the one refusal', async () => {
    const d = door({ status: 401, json: { error: { code: 'authentication_failed' } } });
    await expect(claimPasskey(endpoint(d), 't', 'clm_gone')).rejects.toMatchObject({
      failure: 'refused',
    });
  });
});
