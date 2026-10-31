/**
 * The recovery ceremonies' encoding and order, with `navigator.credentials`
 * stubbed and the door answering what it is told.
 *
 * As in `passkey.test.ts`, no test here fakes a user gesture. What is proved is
 * what this package owns: which route is called in which order, which bytes go
 * back to the door and how, that a session's recovery notices and quarantine
 * time are read, and that a recovery code goes nowhere but the door.
 */

import { describe, expect, it } from 'vitest';
import {
  PasskeyError,
  acknowledgeRecovery,
  contestRecovery,
  endEverySession,
  fromBase64Url,
  issueRecoveryCodes,
  linkPasskey,
  listAuthenticators,
  recoverWithTicket,
  redeemRecoveryCode,
  removePasskey,
  signInWithPasskey,
  toBase64Url,
  type PasskeyEndpoint,
} from '../src/index.js';

interface Sent {
  readonly method: string;
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
      method: String(init?.method),
      url: String(input),
      headers: Object.fromEntries(Object.entries((init?.headers ?? {}) as Record<string, string>)),
      body: typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null,
    });
    const r = replies[Math.min(i++, replies.length - 1)] ?? { status: 500 };
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), {
      status: r.status ?? 200,
    });
  };
  return { sent, fetch };
}

const bytes = (...n: number[]) => Uint8Array.from(n);
const buf = (u: Uint8Array): ArrayBuffer =>
  u.buffer.slice(u.byteOffset, u.byteOffset + u.byteLength) as ArrayBuffer;

const CHALLENGE = toBase64Url(Uint8Array.from({ length: 32 }, (_, i) => i * 7 + 1));
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
  exclude_credentials: [] as string[],
  timeout_ms: 300000,
};
const ASSERTION = {
  rawId: buf(bytes(251, 252, 253, 254, 255)),
  response: {
    clientDataJSON: buf(new TextEncoder().encode('{ "type":"webauthn.get" ,"x":1 }\n')),
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
const TICKET = `tkt_${'ab'.repeat(32)}`;

function over(d: ReturnType<typeof door>, gestures: { get?: number; create?: number } = {}) {
  const calls = { get: 0, create: 0 };
  const endpoint: PasskeyEndpoint = {
    baseUrl: 'https://door.example.com/',
    fetch: d.fetch,
    credentials: {
      get: async () => {
        calls.get += 1;
        return ASSERTION as unknown as Credential;
      },
      create: async () => {
        calls.create += 1;
        return ATTESTATION as unknown as Credential;
      },
    },
  };
  void gestures;
  return { endpoint, calls };
}

describe('the list', () => {
  it('reads the ways in, the warning, and the codes left', async () => {
    const d = door({
      json: {
        authenticators: [
          { id: 'passkey:a', kind: 'passkey', state: 'active', backup_eligible: true, backed_up: false },
          { id: 'passkey:b', kind: 'passkey', state: 'retired', contest_until: 900 },
          { id: 'passkey:c', kind: 'passkey', state: 'active', presence_available_at: 800 },
        ],
        notices: ['single_authenticator', 'recovery_codes_low'],
        recovery_codes_remaining: 2,
      },
    });
    const { endpoint } = over(d);
    const l = await listAuthenticators(endpoint, 'tok');
    expect(d.sent[0]).toMatchObject({
      method: 'GET',
      url: 'https://door.example.com/auth/authenticators',
    });
    expect(d.sent[0]?.headers['Authorization']).toBe('Bearer tok');
    expect(l.notices).toEqual(['single_authenticator', 'recovery_codes_low']);
    expect(l.recoveryCodesRemaining).toBe(2);
    expect(l.authenticators.map((a) => a.state)).toEqual(['active', 'retired', 'active']);
    expect(l.authenticators[0]?.backupEligible).toBe(true);
    expect(l.authenticators[1]?.contestUntil).toBe(900);
    expect(l.authenticators[2]?.presenceAvailableAt).toBe(800);
  });
});

describe('adding and removing a passkey', () => {
  it('a_link_runs_both_ceremonies_and_posts_both_answers', async () => {
    const d = door(
      { json: { registration: CREATION_OPTIONS, vouch: REQUEST_OPTIONS } },
      { json: { authenticator: 'passkey:new', authenticators: 2 } },
    );
    const { endpoint, calls } = over(d);
    const done = await linkPasskey(endpoint, 'tok');
    expect(done).toEqual({ authenticator: 'passkey:new', authenticators: 2 });
    expect(calls).toEqual({ get: 1, create: 1 });
    expect(d.sent.map((s) => s.url)).toEqual([
      'https://door.example.com/auth/authenticators/link/begin',
      'https://door.example.com/auth/authenticators/link',
    ]);
    const body = d.sent[1]?.body as Record<string, Record<string, string>>;
    expect(body['registration']?.['challenge']).toBe(CHALLENGE);
    expect(body['vouch']?.['challenge']).toBe(CHALLENGE);
    // The bytes the browser produced, exactly.
    expect(
      Array.from(fromBase64Url(body['registration']?.['attestation_object'] as string)),
    ).toEqual(Array.from(new Uint8Array(ATTESTATION.response.attestationObject)));
    expect(Array.from(fromBase64Url(body['vouch']?.['signature'] as string))).toEqual([
      48, 69, 2, 33, 0, 255,
    ]);
    expect(d.sent.every((s) => s.headers['Authorization'] === 'Bearer tok')).toBe(true);
  });

  it('a_removal_is_vouched_through_the_step_up_challenge', async () => {
    const d = door(
      { json: REQUEST_OPTIONS },
      { json: { removed: 'passkey:lost', sessions_ended: true } },
    );
    const { endpoint } = over(d);
    const done = await removePasskey(endpoint, 'tok', 'passkey:lost', { endSessions: true });
    expect(done).toEqual({ removed: 'passkey:lost', sessionsEnded: true });
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/step-up/begin');
    expect(d.sent[1]?.url).toBe('https://door.example.com/auth/authenticators/unlink');
    expect(d.sent[1]?.body).toMatchObject({
      authenticator: 'passkey:lost',
      end_sessions: true,
    });
    const vouch = (d.sent[1]?.body as Record<string, Record<string, string>>)['vouch'];
    expect(vouch?.['challenge']).toBe(CHALLENGE);
  });

  it('says nothing about sessions unless asked, so the door decides', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: { removed: 'x', sessions_ended: true } });
    await removePasskey(over(d).endpoint, 'tok', 'passkey:x');
    expect(d.sent[1]?.body).not.toHaveProperty('end_sessions');
  });

  it('refuses in one message when the door refuses, the last way in included', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { status: 409, json: { error: { code: 'last_authenticator' } } });
    await expect(removePasskey(over(d).endpoint, 'tok', 'passkey:only')).rejects.toMatchObject({
      failure: 'refused',
    });
  });

  it('signs out everywhere with the session alone', async () => {
    const d = door({ status: 204 });
    await endEverySession(over(d).endpoint, 'tok');
    expect(d.sent[0]).toMatchObject({
      method: 'POST',
      url: 'https://door.example.com/auth/logout-all',
    });
    expect(d.sent[0]?.body).toBeNull();
  });
});

describe('recovery codes', () => {
  it('hands back the ten codes once and sends nothing else anywhere', async () => {
    const codes = Array.from({ length: 10 }, (_, i) => `ABCD-EFGH-JKMN-PQRS-TVWX-YZ${i}0`);
    const d = door({ json: { codes, count: 10 } });
    const got = await issueRecoveryCodes(over(d).endpoint, 'tok');
    expect(got).toEqual(codes);
    expect(d.sent).toHaveLength(1);
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/recovery-codes');
  });

  it('a_code_goes_to_the_door_and_nowhere_else', async () => {
    const d = door({
      json: {
        case: 'rec_1',
        ticket: TICKET,
        ticket_expires_at: 5000,
        matures_at: 1000,
        remaining: 9,
      },
    });
    const start = await redeemRecoveryCode(over(d).endpoint, 'ABCD-EFGH-JKMN-PQRS-TVWX-YZ00');
    expect(start).toEqual({
      case: 'rec_1',
      ticket: TICKET,
      ticketExpiresAt: 5000,
      maturesAt: 1000,
      remaining: 9,
    });
    expect(d.sent).toHaveLength(1);
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/recover/code');
    expect(d.sent[0]?.body).toEqual({ code: 'ABCD-EFGH-JKMN-PQRS-TVWX-YZ00' });
    // No token goes with a code: a person with a code has no session.
    expect(d.sent[0]?.headers['Authorization']).toBeUndefined();
  });

  it('reads a delayed case as no ticket yet', async () => {
    const d = door({ json: { case: 'rec_2', matures_at: 9999, remaining: 8 } });
    const start = await redeemRecoveryCode(over(d).endpoint, 'x');
    expect(start.ticket).toBeUndefined();
    expect(start.maturesAt).toBe(9999);
  });

  it('is refused in one message for a wrong code and a throttle alike', async () => {
    for (const status of [401, 429]) {
      const d = door({ status, json: { error: { code: 'x' } } });
      await expect(redeemRecoveryCode(over(d).endpoint, 'nope')).rejects.toBeInstanceOf(
        PasskeyError,
      );
    }
  });
});

describe('a recovery', () => {
  it('registers_the_new_passkey_with_the_ticket_and_reads_what_it_did', async () => {
    const d = door(
      { json: CREATION_OPTIONS },
      {
        json: {
          recovered: true,
          case: 'rec_1',
          authenticator: 'passkey:new',
          retired: ['passkey:old'],
          presence_available_at: 90000,
          contest_until: 260000,
          warnings: [],
        },
      },
    );
    const { endpoint, calls } = over(d);
    const r = await recoverWithTicket(endpoint, TICKET, { lost: ['passkey:old'] });
    expect(calls).toEqual({ get: 0, create: 1 });
    expect(d.sent[0]).toMatchObject({
      url: 'https://door.example.com/auth/recover/begin',
      body: { ticket: TICKET },
    });
    // No token: the person has none. That is why they are recovering.
    expect(d.sent[0]?.headers['Authorization']).toBeUndefined();
    const body = d.sent[1]?.body as Record<string, unknown>;
    expect(d.sent[1]?.url).toBe('https://door.example.com/auth/recover/complete');
    expect(body['kind']).toBe('passkey_registration');
    expect(body['ticket']).toBe(TICKET);
    expect(body['challenge']).toBe(CHALLENGE);
    expect(body['lost']).toEqual(['passkey:old']);
    expect(
      Array.from(fromBase64Url(body['client_data_json'] as string)),
    ).toEqual(Array.from(new Uint8Array(ATTESTATION.response.clientDataJSON)));
    expect(r).toEqual({
      case: 'rec_1',
      authenticator: 'passkey:new',
      retired: ['passkey:old'],
      presenceAvailableAt: 90000,
      contestUntil: 260000,
      warnings: [],
    });
  });

  it('retires them all when no lost passkey is named', async () => {
    const d = door(
      { json: CREATION_OPTIONS },
      {
        json: {
          case: 'rec_1',
          authenticator: 'p',
          retired: [],
          presence_available_at: 1,
          contest_until: 2,
          warnings: [],
        },
      },
    );
    await recoverWithTicket(over(d).endpoint, TICKET);
    expect(d.sent[1]?.body).not.toHaveProperty('lost');
  });

  it('a_contest_is_an_ordinary_assertion_and_says_which_it_was', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: { contested: 'rec_1', restored: 1 } });
    const { endpoint, calls } = over(d);
    const r = await contestRecovery(endpoint);
    expect(r).toEqual({ contested: 'rec_1' });
    expect(calls).toEqual({ get: 1, create: 0 });
    expect(d.sent[0]?.url).toBe('https://door.example.com/auth/begin');
    expect(d.sent[1]?.url).toBe('https://door.example.com/auth/recover/contest');
    expect(d.sent[1]?.body).toMatchObject({ kind: 'passkey_assertion', challenge: CHALLENGE });
    const cancelled = door({ json: REQUEST_OPTIONS }, { json: { cancelled: 'rec_2' } });
    expect(await contestRecovery(over(cancelled).endpoint)).toEqual({ cancelled: 'rec_2' });
  });

  it('acknowledges_a_case_with_the_session_and_the_case_id', async () => {
    const d = door({ status: 204 });
    await acknowledgeRecovery(over(d).endpoint, 'tok', 'rec_1');
    expect(d.sent[0]).toMatchObject({
      url: 'https://door.example.com/auth/recover/ack',
      body: { case: 'rec_1' },
    });
    expect(d.sent[0]?.headers['Authorization']).toBe('Bearer tok');
  });
});

describe('what a sign-in carries', () => {
  it('reads the notices, the quarantine time and the recoveries to acknowledge', async () => {
    const d = door(
      { json: REQUEST_OPTIONS },
      {
        json: {
          token: 'tok',
          expires_at: 4000,
          session_expires_at: 90000,
          authenticators: 1,
          notices: ['single_authenticator'],
          presence_available_at: 86500,
          recoveries: [
            {
              case: 'rec_1',
              path: 'operator',
              authority: 'deployment-owner',
              evidence: 'verified-out-of-band',
              state: 'completed',
              contest_until: 259000,
            },
          ],
        },
      },
    );
    const s = await signInWithPasskey(over(d).endpoint);
    expect(s.notices).toEqual(['single_authenticator']);
    expect(s.presenceAvailableAt).toBe(86500);
    expect(s.recoveries).toEqual([
      {
        case: 'rec_1',
        path: 'operator',
        authority: 'deployment-owner',
        evidence: 'verified-out-of-band',
        state: 'completed',
        contestUntil: 259000,
      },
    ]);
  });

  it('has none of them when the door says none', async () => {
    const d = door(
      { json: REQUEST_OPTIONS },
      { json: { token: 'tok', expires_at: 4000, session_expires_at: 90000 } },
    );
    const s = await signInWithPasskey(over(d).endpoint);
    expect(s.notices).toBeUndefined();
    expect(s.presenceAvailableAt).toBeUndefined();
    expect(s.recoveries).toBeUndefined();
  });
});
