/**
 * The passkey ceremony's encoding, with `navigator.credentials` stubbed.
 *
 * No test here fakes a user gesture (checklist 5h). What is proved is what
 * this package owns: which bytes are decoded before they reach the browser,
 * which are encoded on the way back, and that `clientDataJSON` survives the
 * trip byte for byte. A person clicks through the first real one.
 */

import { describe, expect, it } from 'vitest';
import {
  PasskeyError,
  fromBase64Url,
  registerPasskey,
  signInWithPasskey,
  stepUp,
  toBase64Url,
  type PasskeyEndpoint,
} from '../src/auth/passkey.js';

interface Sent {
  readonly url: string;
  readonly headers: Record<string, string>;
  readonly body: Record<string, unknown> | null;
}

/** A door that answers what it is told, in order, and records what it was sent. */
function door(...replies: Array<{ status?: number; json?: unknown }>) {
  const sent: Sent[] = [];
  let i = 0;
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const raw = init?.body;
    sent.push({
      url: String(input),
      headers: Object.fromEntries(Object.entries((init?.headers ?? {}) as Record<string, string>)),
      body: typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null,
    });
    const r = replies[Math.min(i++, replies.length - 1)] ?? { status: 500 };
    const status = r.status ?? 200;
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), { status });
  };
  return { sent, fetch };
}

const bytes = (...n: number[]) => Uint8Array.from(n);
const buf = (u: Uint8Array): ArrayBuffer =>
  u.buffer.slice(u.byteOffset, u.byteOffset + u.byteLength) as ArrayBuffer;

/** The 32 bytes the "server" minted for every challenge in these tests. */
const CHALLENGE_BYTES = Uint8Array.from({ length: 32 }, (_, i) => i * 7 + 1);
const CHALLENGE = toBase64Url(CHALLENGE_BYTES);

const TOKEN_ANSWER = { token: 'tok', expires_at: 4000, session_expires_at: 90000 };

const REQUEST_OPTIONS = {
  challenge: CHALLENGE,
  method: 'passkey',
  rp_id: 'example.com',
  user_verification: 'required',
  allow_credentials: [] as string[],
  timeout_ms: 300000,
};

const CREATION_OPTIONS = {
  challenge: CHALLENGE,
  method: 'passkey',
  rp_id: 'example.com',
  rp_name: 'Example',
  user: { id: toBase64Url(bytes(9, 9, 9)), name: 'Ana', display_name: 'Ana' },
  algorithms: [-7, -8],
  user_verification: 'required',
  resident_key: 'required',
  attestation: 'none',
  exclude_credentials: [toBase64Url(bytes(200, 201))],
  timeout_ms: 300000,
};

/** The four byte strings a browser hands back, with bytes that are not text. */
const ASSERTION = {
  rawId: buf(bytes(251, 252, 253, 254, 255)),
  response: {
    // Deliberately not canonical JSON: spaces, a key order a serialiser
    // would change, and a trailing newline. These bytes are what is signed.
    clientDataJSON: buf(
      new TextEncoder().encode(
        '{ "type":"webauthn.get",\n  "challenge":"x","origin":"https://app.example.com" }\n',
      ),
    ),
    authenticatorData: buf(bytes(0, 1, 2, 250, 251)),
    signature: buf(bytes(48, 69, 2, 33, 0, 255)),
  },
};

const ATTESTATION = {
  rawId: buf(bytes(1)),
  response: {
    clientDataJSON: buf(new TextEncoder().encode('{"type":"webauthn.create" ,"x":1}')),
    attestationObject: buf(bytes(163, 99, 102, 109, 116, 100, 110, 111, 110, 101, 255)),
  },
};

function endpointOver(
  d: ReturnType<typeof door>,
  credentials: NonNullable<PasskeyEndpoint['credentials']>,
): PasskeyEndpoint {
  return { baseUrl: 'https://door.example.com/', fetch: d.fetch, credentials };
}

describe('base64url', () => {
  it('round-trips every length without padding', () => {
    for (let n = 0; n < 9; n++) {
      const b = Uint8Array.from({ length: n }, (_, i) => 250 - i * 31);
      const s = toBase64Url(b);
      expect(s).not.toContain('=');
      expect(s).not.toMatch(/[+/]/);
      expect(Array.from(fromBase64Url(s))).toEqual(Array.from(b));
    }
    expect(toBase64Url(bytes(251, 255))).toBe('-_8');
  });

  it('refuses a character outside the alphabet', () => {
    expect(() => fromBase64Url('ab+c')).toThrow(PasskeyError);
  });
});

describe('sign in', () => {
  it('a_challenge_is_decoded_to_the_bytes_the_server_minted', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: TOKEN_ANSWER });
    let seen: PublicKeyCredentialRequestOptions | undefined;
    await signInWithPasskey(
      endpointOver(d, {
        create: async () => null,
        get: async (o) => {
          seen = o?.publicKey;
          return ASSERTION as unknown as Credential;
        },
      }),
    );
    expect(seen).toBeDefined();
    // What the authenticator signs over is the decoded bytes, not the text.
    expect(Array.from(new Uint8Array(seen?.challenge as ArrayBuffer))).toEqual(
      Array.from(CHALLENGE_BYTES),
    );
    expect(seen?.rpId).toBe('example.com');
    expect(seen?.userVerification).toBe('required');
    expect(seen?.allowCredentials).toEqual([]);
    expect(seen?.timeout).toBe(300000);
    // And what goes back to the server is the string it sent.
    expect(d.sent[1]?.body?.['challenge']).toBe(CHALLENGE);
  });

  it('the_four_byte_strings_reach_the_server_as_unpadded_base64url', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: TOKEN_ANSWER });
    const s = await signInWithPasskey(
      endpointOver(d, { create: async () => null, get: async () => ASSERTION as unknown as Credential }),
    );
    expect(s.token).toBe('tok');
    expect(s.expiresAt).toBe(4000);
    expect(s.sessionExpiresAt).toBe(90000);
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/begin');
    expect(d.sent[0]?.body).toEqual({ method: 'passkey' });
    const body = d.sent[1]?.body as Record<string, string>;
    expect(d.sent[1]?.url).toBe('https://door.example.com/auth/complete');
    expect(body['method']).toBe('passkey');
    expect(body['kind']).toBe('passkey_assertion');
    for (const key of ['credential_id', 'client_data_json', 'authenticator_data', 'signature']) {
      expect(body[key], key).toMatch(/^[A-Za-z0-9_-]+$/);
    }
    expect(Array.from(fromBase64Url(body['credential_id'] as string))).toEqual([251, 252, 253, 254, 255]);
    expect(Array.from(fromBase64Url(body['authenticator_data'] as string))).toEqual([0, 1, 2, 250, 251]);
    expect(Array.from(fromBase64Url(body['signature'] as string))).toEqual([48, 69, 2, 33, 0, 255]);
  });

  it('a_turnstile_token_travels_in_the_begin_body_as_human', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: TOKEN_ANSWER });
    await signInWithPasskey(
      endpointOver(d, { create: async () => null, get: async () => ASSERTION as unknown as Credential }),
      { human: 'cf-token' },
    );
    expect(d.sent[0]?.body).toEqual({ method: 'passkey', human: 'cf-token' });
    expect(d.sent[1]?.body?.['human']).toBeUndefined();
  });

  it('client_data_json_is_posted_byte_for_byte', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: TOKEN_ANSWER });
    await signInWithPasskey(
      endpointOver(d, { create: async () => null, get: async () => ASSERTION as unknown as Credential }),
    );
    const posted = fromBase64Url((d.sent[1]?.body as Record<string, string>)['client_data_json'] as string);
    expect(Array.from(posted)).toEqual(Array.from(new Uint8Array(ASSERTION.response.clientDataJSON)));
    // A parse and re-serialise would have changed these bytes.
    expect(new TextDecoder().decode(posted)).not.toBe(
      JSON.stringify(JSON.parse(new TextDecoder().decode(posted))),
    );
  });

  it('names a tenant and an invitation when asked', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: { ...TOKEN_ANSWER, admission: { space: 'acme', outcome: 'admitted' } } });
    const s = await signInWithPasskey(
      endpointOver(d, { create: async () => null, get: async () => ASSERTION as unknown as Credential }),
      { space: 'acme', invitation: 'tkt_x' },
    );
    expect(d.sent[1]?.body).toMatchObject({ space: 'acme', invitation: 'tkt_x' });
    expect(s.admission).toEqual({ space: 'acme', outcome: 'admitted' });
  });

  it('turns a dismissed prompt into a cancellation and a refusal into one message', async () => {
    const d = door({ json: REQUEST_OPTIONS });
    await expect(
      signInWithPasskey(
        endpointOver(d, {
          create: async () => null,
          get: async () => {
            throw Object.assign(new Error('x'), { name: 'NotAllowedError' });
          },
        }),
      ),
    ).rejects.toMatchObject({ failure: 'cancelled' });

    const refused = door({ json: REQUEST_OPTIONS }, { status: 401, json: { error: { code: 'authentication_failed' } } });
    await expect(
      signInWithPasskey(
        endpointOver(refused, { create: async () => null, get: async () => ASSERTION as unknown as Credential }),
      ),
    ).rejects.toMatchObject({ failure: 'refused' });

    const down = door({ status: 503, json: { error: { code: 'credential_store_unavailable' } } });
    await expect(
      signInWithPasskey(endpointOver(down, { create: async () => null, get: async () => null })),
    ).rejects.toMatchObject({ failure: 'unavailable' });
  });

  it('is unsupported where there is no credentials container', async () => {
    const d = door({ json: REQUEST_OPTIONS });
    await expect(signInWithPasskey({ baseUrl: 'https://x', fetch: d.fetch })).rejects.toMatchObject({
      failure: 'unsupported',
    });
  });
});

describe('register', () => {
  it('sends the human-check token with the first leg, register', async () => {
    const d = door({ json: CREATION_OPTIONS }, { json: TOKEN_ANSWER });
    await registerPasskey(
      endpointOver(d, {
        get: async () => null,
        create: async () => ATTESTATION as unknown as Credential,
      }),
      { human: 'ts_token' },
    );
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/register');
    expect(d.sent[0]?.body).toEqual({ method: 'passkey', human: 'ts_token' });
  });

  it('builds the creation options from the door answer and posts the attestation unpadded', async () => {
    const d = door({ json: CREATION_OPTIONS }, { json: TOKEN_ANSWER });
    let seen: PublicKeyCredentialCreationOptions | undefined;
    const s = await registerPasskey(
      endpointOver(d, {
        get: async () => null,
        create: async (o) => {
          seen = o?.publicKey;
          return ATTESTATION as unknown as Credential;
        },
      }),
      { display: 'Ana', ticket: 'tkt_abc' },
    );
    expect(s.token).toBe('tok');
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/register');
    expect(d.sent[0]?.body).toEqual({ method: 'passkey', display: 'Ana', ticket: 'tkt_abc' });
    expect(Array.from(new Uint8Array(seen?.challenge as ArrayBuffer))).toEqual(Array.from(CHALLENGE_BYTES));
    expect(Array.from(new Uint8Array(seen?.user.id as ArrayBuffer))).toEqual([9, 9, 9]);
    expect(seen?.user.name).toBe('Ana');
    expect(seen?.user.displayName).toBe('Ana');
    expect(seen?.rp).toEqual({ id: 'example.com', name: 'Example' });
    expect(seen?.pubKeyCredParams).toEqual([
      { type: 'public-key', alg: -7 },
      { type: 'public-key', alg: -8 },
    ]);
    expect(seen?.authenticatorSelection).toEqual({
      residentKey: 'required',
      userVerification: 'required',
    });
    expect(seen?.attestation).toBe('none');
    expect(seen?.excludeCredentials?.length).toBe(1);
    expect(Array.from(new Uint8Array(seen?.excludeCredentials?.[0]?.id as ArrayBuffer))).toEqual([200, 201]);

    const body = d.sent[1]?.body as Record<string, string>;
    expect(body['kind']).toBe('passkey_registration');
    expect(body['challenge']).toBe(CHALLENGE);
    expect(body['ticket']).toBe('tkt_abc');
    expect(Array.from(fromBase64Url(body['attestation_object'] as string))).toEqual([
      163, 99, 102, 109, 116, 100, 110, 111, 110, 101, 255,
    ]);
    expect(new TextDecoder().decode(fromBase64Url(body['client_data_json'] as string))).toBe(
      '{"type":"webauthn.create" ,"x":1}',
    );
  });
});

describe('step up', () => {
  it('carries the bearer token on both calls and answers the re-minted session', async () => {
    const d = door(
      { json: { ...REQUEST_OPTIONS, allow_credentials: [toBase64Url(bytes(251, 252, 253, 254, 255))] } },
      { json: { ...TOKEN_ANSWER, token: 'stepped' } },
    );
    let seen: PublicKeyCredentialRequestOptions | undefined;
    const s = await stepUp(
      endpointOver(d, {
        create: async () => null,
        get: async (o) => {
          seen = o?.publicKey;
          return ASSERTION as unknown as Credential;
        },
      }),
      'the-session-token',
    );
    expect(s.token).toBe('stepped');
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/step-up/begin');
    expect(d.sent[1]?.url).toBe('https://door.example.com/auth/step-up');
    expect(d.sent[0]?.headers['Authorization']).toBe('Bearer the-session-token');
    expect(d.sent[1]?.headers['Authorization']).toBe('Bearer the-session-token');
    expect(d.sent[0]?.body).toBeNull();
    expect(Array.from(new Uint8Array(seen?.allowCredentials?.[0]?.id as ArrayBuffer))).toEqual([
      251, 252, 253, 254, 255,
    ]);
  });
});
