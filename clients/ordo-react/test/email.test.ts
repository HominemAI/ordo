/**
 * A person's own email address (owner ruling 2026-10-22): the calls, and the
 * hook a settings page is built on.
 */

import { afterEach, describe, expect, it } from 'vitest';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { createElement } from 'react';
import { useAccountEmail, type AccountEmailResult } from '../src/hooks/useAccountEmail.js';
import { AccountEmailError, beginAccountEmail, needsAccountEmail, readAccountEmail } from '../src/email.js';

const BASE = 'https://door.example';

interface Seen {
  readonly url: string;
  readonly method: string;
  readonly auth: string | null;
  readonly body: Record<string, unknown> | null;
}

/** A door that keeps one address and checks the code `12345678`. */
function door(required: unknown = 'off') {
  const seen: Seen[] = [];
  let address: string | null = null;
  let mailed: string | null = null;
  const reply = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = String(input);
    const raw = init?.body;
    const body = typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null;
    seen.push({ url, method: init?.method ?? 'GET', auth: new Headers(init?.headers).get('Authorization'), body });
    const path = url.slice(BASE.length);
    if (path === '/auth/email') {
      return reply(200, {
        address,
        source: address ? 'code' : null,
        verified_at: address ? 1 : null,
        required,
        setting: 'ORDO_EMAIL_REQUIRED',
        mail: true,
      });
    }
    if (path === '/auth/email/begin') {
      mailed = String(body?.['address']);
      return reply(200, { challenge: 'ch', binder: 'bd' });
    }
    if (path === '/auth/email/complete') {
      if (body?.['code'] !== '12345678' || mailed === null) {
        return reply(401, { error: { code: 'authentication_failed', message: 'x' } });
      }
      address = mailed;
      return reply(200, { address, source: 'code', verified_at: 1, replaced: false });
    }
    if (path === '/auth/email/remove') {
      const removed = address !== null;
      address = null;
      return reply(200, { removed });
    }
    return reply(404, {});
  };
  return { fetch, seen };
}

let renderer: ReactTestRenderer | null = null;
afterEach(() => {
  if (renderer) void act(() => renderer?.unmount());
  renderer = null;
});

const settle = async () => {
  for (let i = 0; i < 5; i++) await act(async () => {});
};

async function mount(fetch: typeof globalThis.fetch): Promise<{ current: () => AccountEmailResult }> {
  let last: AccountEmailResult | null = null;
  const Probe = () => {
    last = useAccountEmail({ baseUrl: BASE, token: 'tok_ada', fetch });
    return null;
  };
  await act(async () => {
    renderer = create(createElement(Probe));
  });
  await settle();
  return { current: () => last! };
}

describe('the calls', () => {
  it('read with the bearer token and begin with the trimmed address', async () => {
    const d = door(['setReminder']);
    const e = await readAccountEmail({ baseUrl: BASE, token: 'tok', fetch: d.fetch });
    expect(e.required).toEqual(['setReminder']);
    expect(needsAccountEmail(e)).toBe(true);
    expect(d.seen[0]?.auth).toBe('Bearer tok');
    await beginAccountEmail({ baseUrl: BASE, token: 'tok', fetch: d.fetch }, ' a@b.org ');
    expect(d.seen[1]?.body).toEqual({ address: 'a@b.org' });
  });

  it('names the setting when the deployment sends no mail', async () => {
    const fetch: typeof globalThis.fetch = async () =>
      new Response(JSON.stringify({ error: { code: 'email_unavailable', message: 'x', setting: 'ORDO_MAIL_KIND' } }), {
        status: 403,
      });
    const e = await beginAccountEmail({ baseUrl: BASE, token: 't', fetch }, 'a@b.c').catch((x: unknown) => x);
    expect(e).toBeInstanceOf(AccountEmailError);
    expect(e).toMatchObject({ problem: 'email_unavailable', setting: 'ORDO_MAIL_KIND' });
  });
});

describe('useAccountEmail', () => {
  it('adds an address after the code, then removes it', async () => {
    const d = door('all');
    const h = await mount(d.fetch);
    expect(h.current().email?.address).toBeNull();
    expect(h.current().needed).toBe(true);

    await act(async () => h.current().begin('ada@example.org'));
    expect(h.current().pending).toBe('ada@example.org');
    expect(h.current().email?.address).toBeNull();

    await act(async () => h.current().confirm('00000000'));
    expect(h.current().problem?.problem).toBe('refused');
    expect(h.current().pending).toBe('ada@example.org');

    await act(async () => h.current().confirm(' 12345678 '));
    expect(h.current().problem).toBeNull();
    expect(h.current().pending).toBeNull();
    expect(h.current().email?.address).toBe('ada@example.org');
    expect(h.current().needed).toBe(false);

    await act(async () => h.current().remove());
    expect(h.current().email?.address).toBeNull();
  });

  it('cancel forgets the mailed code', async () => {
    const d = door();
    const h = await mount(d.fetch);
    await act(async () => h.current().begin('ada@example.org'));
    act(() => h.current().cancel());
    expect(h.current().pending).toBeNull();
    expect(h.current().needed).toBe(false);
  });
});
