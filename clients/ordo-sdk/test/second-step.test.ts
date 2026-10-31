/**
 * The second-step loop, with `navigator.credentials` stubbed.
 *
 * Nothing here fakes a user gesture. What is proved is what this package owns:
 * that a sign-in answering no token is a second step and not a failure, that
 * the handle and its binder travel together, that a code is one request and a
 * security key is two, and that a key's assertion is encoded by the passkey
 * module's code and reaches the door byte for byte.
 */

import { describe, expect, it } from 'vitest';
import {
  PasskeyError,
  fromBase64Url,
  toBase64Url,
  type PasskeyEndpoint,
} from '../src/auth/passkey.js';
import {
  beginTotpEnrolment,
  completeSecondStep,
  confirmTotpEnrolment,
  enrolSecurityKey,
  listFactors,
  reauthenticateWithPassword,
  removeFactor,
  setSecondStepLevel,
  signInWithPasskeyStep,
  signInWithPassword,
  type SecondStep,
} from '../src/auth/second-step.js';

interface Sent {
  readonly url: string;
  readonly method: string;
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
      method: String(init?.method ?? 'GET'),
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

const CHALLENGE_BYTES = Uint8Array.from({ length: 32 }, (_, i) => i * 7 + 1);
const CHALLENGE = toBase64Url(CHALLENGE_BYTES);
const PASSWORD_CHALLENGE = 'pw-challenge-1';

const STEP_ANSWER = {
  second_step: 'ss_abc',
  binder: 'b'.repeat(64),
  methods: ['totp', 'security-key', 'backup-code'],
  expires_at: 1300,
};
const TOKEN_ANSWER = { token: 'tok', expires_at: 4000, session_expires_at: 90000 };
const KEY_OPTIONS = {
  challenge: CHALLENGE,
  method: 'passkey',
  rp_id: 'example.com',
  user_verification: 'preferred',
  allow_credentials: [toBase64Url(bytes(251, 252, 253, 254, 255))],
  timeout_ms: 300000,
};
const ASSERTION = {
  rawId: buf(bytes(251, 252, 253, 254, 255)),
  response: {
    // Not canonical JSON: these bytes are what is signed.
    clientDataJSON: buf(
      new TextEncoder().encode(
        '{ "type":"webauthn.get",\n  "challenge":"x","origin":"https://app.example.com" }\n',
      ),
    ),
    authenticatorData: buf(bytes(0, 1, 2, 250, 251)),
    signature: buf(bytes(48, 69, 2, 33, 0, 255)),
  },
};
const CREATION_OPTIONS = {
  challenge: CHALLENGE,
  method: 'passkey',
  rp_id: 'example.com',
  rp_name: 'Example',
  user: { id: toBase64Url(bytes(9, 9, 9)), name: 'my key', display_name: 'my key' },
  algorithms: [-7, -8],
  user_verification: 'preferred',
  resident_key: 'discouraged',
  attestation: 'none',
  exclude_credentials: [] as string[],
  timeout_ms: 300000,
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
  credentials?: NonNullable<PasskeyEndpoint['credentials']>,
): PasskeyEndpoint {
  return {
    baseUrl: 'https://door.example.com/',
    fetch: d.fetch,
    ...(credentials ? { credentials } : {}),
  };
}

const STEP: SecondStep = {
  handle: STEP_ANSWER.second_step,
  binder: STEP_ANSWER.binder,
  expiresAt: 1300,
  methods: ['totp', 'security-key', 'backup-code'],
  enrol: false,
};

describe('a sign-in that is not yet a session', () => {
  it('a_second_step_answer_is_completed_with_a_code', async () => {
    const d = door(
      { json: { challenge: PASSWORD_CHALLENGE, method: 'password' } },
      { json: STEP_ANSWER },
      { json: { ...TOKEN_ANSWER, second_step: 'totp' } },
    );
    const endpoint = endpointOver(d);
    const first = await signInWithPassword(endpoint, {
      identifier: 'ana@example.test',
      password: 'correct-horse',
    });
    // No token is a second step, not a failure.
    expect(first.kind).toBe('second-step');
    if (first.kind !== 'second-step') throw new Error('unreachable');
    expect(first.step).toEqual({
      handle: 'ss_abc',
      binder: 'b'.repeat(64),
      expiresAt: 1300,
      methods: ['totp', 'security-key', 'backup-code'],
      enrol: false,
    });
    expect(d.sent[1]?.url).toBe('https://door.example.com/auth/complete');
    expect(d.sent[1]?.body).toMatchObject({
      method: 'password',
      kind: 'password',
      challenge: PASSWORD_CHALLENGE,
      identifier: 'ana@example.test',
    });

    // A code is one request, sent with the handle and its binder together, and
    // the answer is the session.
    const done = await completeSecondStep(endpoint, first.step, { kind: 'totp', code: ' 123456 ' });
    expect(done.kind).toBe('session');
    if (done.kind !== 'session') throw new Error('unreachable');
    expect(done.session.token).toBe('tok');
    expect(done.via).toBe('totp');
    expect(d.sent).toHaveLength(3);
    expect(d.sent[2]?.url).toBe('https://door.example.com/auth/second-step');
    expect(d.sent[2]?.body).toEqual({
      handle: 'ss_abc',
      binder: 'b'.repeat(64),
      kind: 'totp',
      code: '123456',
    });
    expect(d.sent[2]?.headers['Authorization']).toBeUndefined();
  });

  it('a backup code is the same one request, under its own kind', async () => {
    const d = door({ json: { ...TOKEN_ANSWER, second_step: 'backup-code', recovery_codes_low: true } });
    const done = await completeSecondStep(endpointOver(d), STEP, {
      kind: 'backup-code',
      code: 'AB12-CD34-EF56-GH78-JK90-MN12',
    });
    expect(d.sent[0]?.body).toMatchObject({ kind: 'backup_code', handle: 'ss_abc' });
    expect(done.kind === 'session' && done.via).toBe('backup-code');
    expect(done.kind === 'session' && done.session.raw['recovery_codes_low']).toBe(true);
  });

  it('a_key_second_step_reuses_the_passkey_assertion_encoding', async () => {
    const d = door({ json: KEY_OPTIONS }, { json: { ...TOKEN_ANSWER, second_step: 'security-key' } });
    let seen: PublicKeyCredentialRequestOptions | undefined;
    const done = await completeSecondStep(
      endpointOver(d, {
        create: async () => null,
        get: async (o) => {
          seen = o?.publicKey;
          return ASSERTION as unknown as Credential;
        },
      }),
      STEP,
      { kind: 'security-key' },
    );
    // Two requests: the challenge, then the assertion.
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/second-step/begin');
    expect(d.sent[0]?.body).toEqual({
      handle: 'ss_abc',
      binder: 'b'.repeat(64),
      kind: 'security-key',
    });
    // The key signs the decoded bytes, narrowed to the person's own keys, and
    // is asked for the user verification the door named.
    expect(Array.from(new Uint8Array(seen?.challenge as ArrayBuffer))).toEqual(
      Array.from(CHALLENGE_BYTES),
    );
    expect(seen?.userVerification).toBe('preferred');
    expect(seen?.allowCredentials).toHaveLength(1);
    expect(Array.from(new Uint8Array(seen?.allowCredentials?.[0]?.id as ArrayBuffer))).toEqual([
      251, 252, 253, 254, 255,
    ]);
    // What goes back is the string the door sent, the handle and binder, and
    // the four byte strings as the passkey module encodes them.
    const body = d.sent[1]?.body as Record<string, string>;
    expect(d.sent[1]?.url).toBe('https://door.example.com/auth/second-step');
    expect(body['kind']).toBe('passkey_assertion');
    expect(body['handle']).toBe('ss_abc');
    expect(body['binder']).toBe('b'.repeat(64));
    expect(body['challenge']).toBe(CHALLENGE);
    for (const key of ['credential_id', 'client_data_json', 'authenticator_data', 'signature']) {
      expect(body[key], key).toMatch(/^[A-Za-z0-9_-]+$/);
    }
    expect(Array.from(fromBase64Url(body['credential_id'] as string))).toEqual([251, 252, 253, 254, 255]);
    expect(Array.from(fromBase64Url(body['signature'] as string))).toEqual([48, 69, 2, 33, 0, 255]);
    // clientDataJSON is posted byte for byte.
    const posted = fromBase64Url(body['client_data_json'] as string);
    expect(Array.from(posted)).toEqual(Array.from(new Uint8Array(ASSERTION.response.clientDataJSON)));
    expect(done.kind === 'session' && done.via).toBe('security-key');
  });

  it('a passkey sign-in is a session, or a second step when one was turned on', async () => {
    const plain = door(
      { json: { ...KEY_OPTIONS, allow_credentials: [], user_verification: 'required' } },
      { json: TOKEN_ANSWER },
    );
    const creds = { create: async () => null, get: async () => ASSERTION as unknown as Credential };
    const a = await signInWithPasskeyStep(endpointOver(plain, creds), { space: 'acme' });
    expect(a.kind).toBe('session');
    expect(plain.sent[1]?.body).toMatchObject({ kind: 'passkey_assertion', space: 'acme' });

    const stepped = door(
      { json: { ...KEY_OPTIONS, allow_credentials: [], user_verification: 'required' } },
      { json: { ...STEP_ANSWER, methods: ['totp'] } },
    );
    const b = await signInWithPasskeyStep(endpointOver(stepped, creds));
    expect(b.kind === 'second-step' && b.step.methods).toEqual(['totp']);
  });

  it('a person a level makes enrol is told to, and the step carries no methods', async () => {
    const d = door(
      { json: { challenge: PASSWORD_CHALLENGE, method: 'password' } },
      { json: { second_step: 'ss_e', binder: 'c'.repeat(64), enrol: true, expires_at: 1300 } },
    );
    const r = await signInWithPassword(endpointOver(d), { identifier: 'a', password: 'b' });
    expect(r.kind === 'second-step' && r.step.enrol).toBe(true);
    expect(r.kind === 'second-step' && r.step.methods).toEqual([]);
  });

  it('turns the door\'s one refusal into one message, and an unreachable door into another', async () => {
    const refused = door({ json: { challenge: PASSWORD_CHALLENGE } }, { status: 401, json: { error: {} } });
    await expect(
      signInWithPassword(endpointOver(refused), { identifier: 'a', password: 'b' }),
    ).rejects.toMatchObject({ failure: 'refused' });
    const wrong = door({ status: 401, json: { error: {} } });
    await expect(completeSecondStep(endpointOver(wrong), STEP, { kind: 'totp', code: '000000' })).rejects.toBeInstanceOf(
      PasskeyError,
    );
  });
});

describe('proving a first factor again', () => {
  it('a reauthentication is sent with the bearer and never yields a session', async () => {
    const d = door(
      { json: { challenge: PASSWORD_CHALLENGE } },
      { json: { reauth: 'ra_1', expires_at: 1300 } },
    );
    const r = await reauthenticateWithPassword(endpointOver(d), 'tok', {
      identifier: 'ana@example.test',
      password: 'correct-horse',
    });
    expect(r).toEqual({ kind: 'proof', reauth: 'ra_1', expiresAt: 1300 });
    expect(d.sent[1]?.body).toMatchObject({ kind: 'password', reauth: true });
    expect(d.sent[1]?.headers['Authorization']).toBe('Bearer tok');
  });

  it('a person who holds a factor is asked for the step, and completing it yields the proof', async () => {
    const d = door(
      { json: { challenge: PASSWORD_CHALLENGE } },
      { json: STEP_ANSWER },
      { json: { reauth: 'ra_2', expires_at: 1300 } },
    );
    const endpoint = endpointOver(d);
    const r = await reauthenticateWithPassword(endpoint, 'tok', { identifier: 'a', password: 'b' });
    expect(r.kind).toBe('second-step');
    if (r.kind !== 'second-step') throw new Error('unreachable');
    const done = await completeSecondStep(endpoint, r.step, { kind: 'totp', code: '123456' });
    expect(done).toEqual({ kind: 'proof', reauth: 'ra_2', expiresAt: 1300 });
  });

  it('a change that needs a proof says so, and the proof rides in x-reauth', async () => {
    const needs = door({
      status: 403,
      json: { error: { code: 'reauthentication_required', message: 'prove it' } },
    });
    await expect(removeFactor(endpointOver(needs), 'tok', 'sf_1')).rejects.toMatchObject({
      failure: 'reauthentication_required',
    });

    const d = door({ json: { removed: 'sf_1', ...TOKEN_ANSWER, token: 'replacement' } });
    const out = await removeFactor(endpointOver(d), 'tok', 'sf_1', { reauth: 'ra_9' });
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/factors/sf_1/remove');
    expect(d.sent[0]?.headers['x-reauth']).toBe('ra_9');
    expect(d.sent[0]?.headers['Authorization']).toBe('Bearer tok');
    expect(out.removed).toBe('sf_1');
    expect(out.session.token).toBe('replacement');
  });
});

describe('the person\'s factors', () => {
  it('lists them without a secret, and sets the person\'s own level', async () => {
    const d = door(
      {
        json: {
          factors: [
            { id: 'sf_1', kind: 'totp', label: 'phone', enrolled_at: 10, last_used_at: 20, locked: false },
            { id: 'sf_2', kind: 'security-key', label: 'my key', enrolled_at: 11, last_used_at: 0, locked: true },
          ],
          backup_codes_remaining: 7,
          require_second_step: 'when-enrolled',
          second_step: 'enabled',
        },
      },
      { json: { require_second_step: 'also-for-passkeys' } },
    );
    const endpoint = endpointOver(d);
    const l = await listFactors(endpoint, 'tok');
    expect(d.sent[0]?.method).toBe('GET');
    expect(l.factors.map((f) => [f.id, f.kind, f.label, f.locked])).toEqual([
      ['sf_1', 'totp', 'phone', false],
      ['sf_2', 'security-key', 'my key', true],
    ]);
    expect(l.backupCodesRemaining).toBe(7);
    expect(l.secondStep).toBe('enabled');
    const level = await setSecondStepLevel(endpoint, 'tok', 'also-for-passkeys', { reauth: 'ra_3' });
    expect(level).toBe('also-for-passkeys');
    expect(d.sent[1]?.body).toEqual({ require_second_step: 'also-for-passkeys' });
    expect(d.sent[1]?.headers['x-reauth']).toBe('ra_3');
  });

  it('enrols a code-generating factor as a signed-in person, or as one a level made enrol', async () => {
    const begun = {
      factor: 'sf_new',
      kind: 'totp',
      label: 'phone',
      secret: 'GEZDGNBVGY3TQOJQ',
      uri: 'otpauth://totp/Example:ana?secret=GEZDGNBVGY3TQOJQ',
      expires_at: 1300,
    };
    // A signed-in person: bearer, and the proof to start.
    const a = door({ json: begun }, { json: { factor: 'sf_new', kind: 'totp', label: 'phone' } });
    const endpointA = endpointOver(a);
    const t = await beginTotpEnrolment(endpointA, { token: 'tok', reauth: 'ra_4' }, { label: 'phone' });
    expect(t.secret).toBe('GEZDGNBVGY3TQOJQ');
    expect(t.uri.startsWith('otpauth://totp/')).toBe(true);
    expect(a.sent[0]?.headers['x-reauth']).toBe('ra_4');
    expect(a.sent[0]?.headers['Authorization']).toBe('Bearer tok');
    await confirmTotpEnrolment(endpointA, { token: 'tok' }, 'sf_new', ' 123 456');
    expect(a.sent[1]?.body).toEqual({ factor: 'sf_new', code: '123 456' });

    // A person made to enrol: the step's handle and binder, no token.
    const b = door({ json: begun }, { json: { factor: 'sf_new', kind: 'totp', label: 'phone' } });
    const endpointB = endpointOver(b);
    await beginTotpEnrolment(endpointB, { step: STEP });
    await confirmTotpEnrolment(endpointB, { step: STEP }, 'sf_new', '123456');
    for (const sent of b.sent) {
      expect(sent.headers['Authorization']).toBeUndefined();
      expect(sent.body).toMatchObject({ second_step: 'ss_abc', binder: 'b'.repeat(64) });
    }
  });

  it('registers a security key through the passkey module\'s encoding', async () => {
    const d = door({ json: CREATION_OPTIONS }, { json: { factor: 'sf_k', kind: 'security-key', user_verified: false } });
    let seen: PublicKeyCredentialCreationOptions | undefined;
    const out = await enrolSecurityKey(
      endpointOver(d, {
        get: async () => null,
        create: async (o) => {
          seen = o?.publicKey;
          return ATTESTATION as unknown as Credential;
        },
      }),
      { token: 'tok', reauth: 'ra_5' },
      { label: 'my key' },
    );
    expect(out).toEqual({ factor: 'sf_k', userVerified: false });
    // A key that need not be discoverable, with user verification only preferred.
    expect(seen?.authenticatorSelection?.residentKey).toBe('discouraged');
    expect(seen?.authenticatorSelection?.userVerification).toBe('preferred');
    expect(seen?.attestation).toBe('none');
    expect(Array.from(new Uint8Array(seen?.challenge as ArrayBuffer))).toEqual(
      Array.from(CHALLENGE_BYTES),
    );
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/factors/key/begin');
    expect(d.sent[0]?.headers['x-reauth']).toBe('ra_5');
    const body = d.sent[1]?.body as Record<string, string>;
    expect(d.sent[1]?.url).toBe('https://door.example.com/auth/factors/key/complete');
    expect(body['kind']).toBe('passkey_registration');
    expect(body['challenge']).toBe(CHALLENGE);
    expect(Array.from(fromBase64Url(body['attestation_object'] as string))).toEqual([
      163, 99, 102, 109, 116, 100, 110, 111, 110, 101, 255,
    ]);
  });
});
