/**
 * A person's own email address (owner ruling 2026-10-22), against a mock door.
 *
 * What is proved is what this package owns: the routes and bodies, the bearer
 * token, that the code is trimmed, how the answers are read, and how the
 * door's refusals reach the caller.
 */

import { describe, expect, it } from 'vitest';
import { PasskeyError, type PasskeyEndpoint } from '../src/auth/passkey.js';
import {
  accountEmail,
  beginAccountEmail,
  confirmAccountEmail,
  isEmailRequired,
  removeAccountEmail,
} from '../src/auth/account-email.js';
import { OrdoRefusal } from '../src/core/refusal.js';

interface Sent {
  readonly url: string;
  readonly method: string;
  readonly auth: string | null;
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
      auth: new Headers(init?.headers).get('Authorization'),
      body: typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null,
    });
    const r = replies[Math.min(i++, replies.length - 1)] ?? { status: 500 };
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), { status: r.status ?? 200 });
  };
  return { sent, fetch };
}

const endpointOf = (fetch: typeof globalThis.fetch): PasskeyEndpoint => ({ baseUrl: 'https://door.example/', fetch });

describe('accountEmail', () => {
  it('reads a person with no address, and what is required', async () => {
    const d = door({
      json: { address: null, source: null, verified_at: null, required: ['setReminder'], setting: 'ORDO_EMAIL_REQUIRED', mail: true },
    });
    const e = await accountEmail(endpointOf(d.fetch), 'tok_ada');
    expect(e).toEqual({
      address: null,
      source: null,
      verifiedAt: null,
      required: ['setReminder'],
      setting: 'ORDO_EMAIL_REQUIRED',
      mail: true,
    });
    expect(d.sent[0]).toMatchObject({ url: 'https://door.example/auth/email', method: 'GET', auth: 'Bearer tok_ada' });
  });

  it('reads an address and takes a token function', async () => {
    const d = door({
      json: { address: 'ada@example.org', source: 'code', verified_at: 1800000000, required: 'off', setting: 'ORDO_EMAIL_REQUIRED', mail: true },
    });
    const e = await accountEmail(endpointOf(d.fetch), async () => 'tok_fresh');
    expect(e.address).toBe('ada@example.org');
    expect(e.verifiedAt).toBe(1800000000);
    expect(e.required).toBe('off');
    expect(d.sent[0]?.auth).toBe('Bearer tok_fresh');
  });

  it('refuses an answer it cannot read', async () => {
    const d = door({ json: { address: null, required: 7 } });
    await expect(accountEmail(endpointOf(d.fetch), 't')).rejects.toMatchObject({ failure: 'malformed' });
  });
});

describe('adding an address', () => {
  it('begins with the trimmed address and confirms with the trimmed code', async () => {
    const d = door(
      { json: { challenge: 'ch_1', binder: 'b'.repeat(64) } },
      { json: { address: 'ada@example.org', source: 'code', verified_at: 1, replaced: true } },
    );
    const sent = await beginAccountEmail(endpointOf(d.fetch), 'tok', '  ada@example.org ');
    expect(sent).toEqual({ challenge: 'ch_1', binder: 'b'.repeat(64) });
    expect(d.sent[0]).toMatchObject({
      url: 'https://door.example/auth/email/begin',
      method: 'POST',
      auth: 'Bearer tok',
      body: { address: 'ada@example.org' },
    });
    const done = await confirmAccountEmail(endpointOf(d.fetch), 'tok', sent, ' 12345678 ');
    expect(done).toEqual({ address: 'ada@example.org', replaced: true });
    expect(d.sent[1]?.body).toEqual({ challenge: 'ch_1', binder: 'b'.repeat(64), code: '12345678' });
    expect(d.sent[1]?.url).not.toContain('b'.repeat(64));
  });

  it('a wrong code is the one refusal', async () => {
    const d = door({ status: 401, json: { error: { code: 'authentication_failed', message: 'x' } } });
    await expect(
      confirmAccountEmail(endpointOf(d.fetch), 'tok', { challenge: 'c', binder: 'b' }, '0'),
    ).rejects.toMatchObject({ failure: 'refused' });
  });

  it('a deployment with no mail names the setting', async () => {
    const d = door({
      status: 403,
      json: { error: { code: 'email_unavailable', message: 'no mail', setting: 'ORDO_MAIL_KIND' } },
    });
    const e = await beginAccountEmail(endpointOf(d.fetch), 'tok', 'a@b.c').catch((x: unknown) => x);
    expect(e).toBeInstanceOf(PasskeyError);
    expect(e).toMatchObject({ failure: 'email_unavailable', setting: 'ORDO_MAIL_KIND' });
  });
});

describe('removeAccountEmail', () => {
  it('posts to remove and answers whether there was one', async () => {
    const d = door({ json: { removed: true } });
    expect(await removeAccountEmail(endpointOf(d.fetch), 'tok')).toBe(true);
    expect(d.sent[0]).toMatchObject({ url: 'https://door.example/auth/email/remove', method: 'POST' });
  });
});

describe('isEmailRequired', () => {
  it('tells the email_required refusal from any other', () => {
    const detail = { message: 'm', op: 'setReminder', param: null, status: 403, declared: false };
    expect(isEmailRequired(new OrdoRefusal({ ...detail, class: 'email_required' }))).toBe(true);
    expect(isEmailRequired(new OrdoRefusal({ ...detail, class: 'access_denied' }))).toBe(false);
    expect(isEmailRequired(new Error('x'))).toBe(false);
  });
});
