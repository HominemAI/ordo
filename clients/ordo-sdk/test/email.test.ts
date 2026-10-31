/**
 * Sign in with an emailed code, against a mock door.
 *
 * What is proved is what this package owns: the shape of both requests, that the
 * code is trimmed, that the binder travels in the body and never the URL, that
 * the `device` marker reaches the caller, and that a refusal rejects the way a
 * password sign-in does.
 */

import { describe, expect, it } from 'vitest';
import { PasskeyError, type PasskeyEndpoint } from '../src/auth/passkey.js';
import { beginEmailSignIn, completeEmailSignIn } from '../src/auth/email.js';
import { signInWithPassword } from '../src/auth/second-step.js';

interface Sent {
  readonly url: string;
  readonly method: string;
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
      body: typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null,
    });
    const r = replies[Math.min(i++, replies.length - 1)] ?? { status: 500 };
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), {
      status: r.status ?? 200,
    });
  };
  return { sent, fetch };
}

const BEGUN = { challenge: 'ch_1', binder: 'b'.repeat(64), method: 'email-otp' };
const SENT = { challenge: 'ch_1', binder: 'b'.repeat(64) };
const SESSION = {
  token: 'tok_email',
  via: 'email-otp',
  expires_at: 1000,
  session_expires_at: 2000,
};
const REFUSED = {
  status: 401,
  json: { error: { code: 'authentication_failed', message: 'x' } },
};

const endpointOf = (fetch: typeof globalThis.fetch): PasskeyEndpoint => ({
  baseUrl: 'https://door.example',
  fetch,
});

describe('beginEmailSignIn when the door is busy', () => {
  const BUSY = {
    status: 503,
    json: { error: { code: 'server_busy', message: 'every database connection is in use' } },
  };
  const fast = { retries: 2, firstDelayMs: 0, maxDelayMs: 0, jitter: false };

  it('sends the same request again on server_busy, as ops do', async () => {
    const d = door(BUSY, { json: BEGUN });
    const got = await beginEmailSignIn(
      { ...endpointOf(d.fetch), busyRetry: fast },
      { address: 'kim@example.org' },
    );
    expect(got.challenge).toBe('ch_1');
    expect(d.sent).toHaveLength(2);
    expect(d.sent[1]).toEqual(d.sent[0]);
  });

  it('does not retry credential_store_unavailable', async () => {
    const d = door({
      status: 503,
      json: { error: { code: 'credential_store_unavailable', message: 'x' } },
    });
    await expect(
      beginEmailSignIn({ ...endpointOf(d.fetch), busyRetry: fast }, { address: 'kim@example.org' }),
    ).rejects.toMatchObject({ failure: 'unavailable' });
    expect(d.sent).toHaveLength(1);
  });
});

describe('beginEmailSignIn', () => {
  it('sends invitation and space when given', async () => {
    const d = door({ json: BEGUN });
    await beginEmailSignIn(endpointOf(d.fetch), {
      address: 'a@example.com',
      space: 'acme',
      invitation: 'inv_ticket',
    });
    expect(d.sent[0]?.body).toEqual({
      method: 'email',
      identifier: 'a@example.com',
      space: 'acme',
      invitation: 'inv_ticket',
    });
  });

  it('omits invitation and space when not given', async () => {
    const d = door({ json: BEGUN });
    await beginEmailSignIn(endpointOf(d.fetch), { address: 'a@example.com' });
    expect(d.sent[0]?.body).not.toHaveProperty('invitation');
    expect(d.sent[0]?.body).not.toHaveProperty('space');
  });

  it('posts method email with the address and returns challenge and binder', async () => {
    const d = door({ json: BEGUN });
    const sent = await beginEmailSignIn(endpointOf(d.fetch), { address: 'a@example.com' });
    expect(sent).toEqual(SENT);
    expect(d.sent).toHaveLength(1);
    expect(d.sent[0]?.url).toBe('https://door.example/auth/begin');
    expect(d.sent[0]?.method).toBe('POST');
    expect(d.sent[0]?.body).toEqual({ method: 'email', identifier: 'a@example.com' });
  });

  it('sends human and device when given', async () => {
    const d = door({ json: BEGUN });
    await beginEmailSignIn(endpointOf(d.fetch), {
      address: 'a@example.com',
      human: 'turnstile-token',
      device: 'dev_1',
    });
    expect(d.sent[0]?.body).toEqual({
      method: 'email',
      identifier: 'a@example.com',
      human: 'turnstile-token',
      device: 'dev_1',
    });
  });

  it('rejects a refusal as refused', async () => {
    const d = door(REFUSED);
    await expect(beginEmailSignIn(endpointOf(d.fetch), { address: 'nope' })).rejects.toMatchObject({
      failure: 'refused',
    });
  });
});

describe('completeEmailSignIn', () => {
  it('sends invitation and space from options', async () => {
    const d = door({ json: SESSION });
    await completeEmailSignIn(endpointOf(d.fetch), SENT, '123456', {
      space: 'acme',
      invitation: 'inv_ticket',
    });
    expect(d.sent[0]?.body).toMatchObject({ space: 'acme', invitation: 'inv_ticket' });
  });

  it('posts kind email_code with challenge, binder, the trimmed code and the space', async () => {
    const d = door({ json: SESSION });
    await completeEmailSignIn(endpointOf(d.fetch), SENT, '  123456\n', { space: 'spc_a' });
    const call = d.sent[0]!;
    expect(call.url).toBe('https://door.example/auth/complete');
    expect(call.url).not.toContain(SENT.binder);
    expect(call.body).toEqual({
      method: 'email',
      kind: 'email_code',
      challenge: 'ch_1',
      binder: SENT.binder,
      code: '123456',
      space: 'spc_a',
    });
  });

  it('yields the token and the device marker', async () => {
    const d = door({ json: { ...SESSION, device: 'dev_9' } });
    const out = await completeEmailSignIn(endpointOf(d.fetch), SENT, '123456');
    expect(out.kind).toBe('session');
    if (out.kind !== 'session') return;
    expect(out.session.token).toBe('tok_email');
    expect(out.session.device).toBe('dev_9');
    expect(out.session.expiresAt).toBe(1000);
  });

  it('leaves device unset when the door sends none', async () => {
    const d = door({ json: SESSION });
    const out = await completeEmailSignIn(endpointOf(d.fetch), SENT, '123456');
    expect(out.kind === 'session' && out.session.device).toBeUndefined();
  });

  it('yields a SecondStep when the door asks for one', async () => {
    const d = door({
      json: {
        second_step: 'ss_1',
        binder: 'c'.repeat(64),
        methods: ['totp', 'backup-code'],
        expires_at: 1300,
      },
    });
    const out = await completeEmailSignIn(endpointOf(d.fetch), SENT, '123456');
    expect(out.kind).toBe('second-step');
    if (out.kind !== 'second-step') return;
    expect(out.step.handle).toBe('ss_1');
    expect(out.step.methods).toEqual(['totp', 'backup-code']);
  });

  it('rejects a 401 the same way password sign-in does', async () => {
    const wrong = door(REFUSED);
    const email = await completeEmailSignIn(endpointOf(wrong.fetch), SENT, '000000').catch(
      (e: unknown) => e,
    );
    const pw = door({ json: { challenge: 'c' } }, REFUSED);
    const password = await signInWithPassword(endpointOf(pw.fetch), {
      identifier: 'a',
      password: 'b',
    }).catch((e: unknown) => e);
    expect(email).toBeInstanceOf(PasskeyError);
    expect(email).toMatchObject({ failure: 'refused', message: (password as Error).message });
  });

  it('rejects a 429 as a PasskeyError', async () => {
    const d = door({ status: 429, json: { error: { code: 'quota_exceeded' } } });
    await expect(completeEmailSignIn(endpointOf(d.fetch), SENT, '123456')).rejects.toBeInstanceOf(
      PasskeyError,
    );
  });
});
