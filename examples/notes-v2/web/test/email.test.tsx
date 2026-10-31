/**
 * Email for reminders (owner ruling 2026-10-22): a passkey person adds an
 * address, types the emailed code, and then sees it. The door is a stub.
 */

import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { afterEach, describe, expect, it } from 'vitest';
import { EmailForReminders } from '../src/components/EmailForReminders.js';
import { refusalText } from '../src/errors.js';

(globalThis as Record<string, unknown>)['IS_REACT_ACT_ENVIRONMENT'] = true;

const BASE = 'https://door.example';

function stubDoor(opts: { required?: unknown; mail?: boolean } = {}) {
  let address: string | null = null;
  let mailed: string | null = null;
  const paths: string[] = [];
  const reply = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const path = String(input).slice(BASE.length);
    paths.push(path);
    const body = typeof init?.body === 'string' ? (JSON.parse(init.body) as Record<string, unknown>) : {};
    switch (path) {
      case '/auth/email':
        return reply(200, {
          address,
          source: address ? 'code' : null,
          verified_at: address ? 1 : null,
          required: opts.required ?? 'off',
          setting: 'ORDO_EMAIL_REQUIRED',
          mail: opts.mail ?? true,
        });
      case '/auth/email/begin':
        mailed = String(body['address']);
        return reply(200, { challenge: 'c', binder: 'b' });
      case '/auth/email/complete':
        if (body['code'] !== '12345678') return reply(401, { error: { code: 'authentication_failed', message: 'x' } });
        address = mailed;
        return reply(200, { address, source: 'code', verified_at: 1, replaced: false });
      default:
        return reply(404, {});
    }
  };
  return { fetch, paths };
}

let tree: ReactTestRenderer | null = null;
afterEach(() => {
  act(() => tree?.unmount());
  tree = null;
});

const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 10))));

async function show(fetch: typeof globalThis.fetch) {
  await act(async () => {
    tree = create(<EmailForReminders baseUrl={BASE} token={async () => 'tok'} fetch={fetch} />);
  });
  await settle();
  const root = () => (tree as ReactTestRenderer).root;
  const text = () => JSON.stringify((tree as ReactTestRenderer).toJSON());
  const type = async (id: string, value: string) => {
    const input = root().findByProps({ id });
    await act(async () => (input.props as { onChange: (e: unknown) => void }).onChange({ target: { value } }));
  };
  const submit = async () => {
    const form = root().findByType('form');
    await act(async () => (form.props as { onSubmit: (e: unknown) => void }).onSubmit({ preventDefault() {} }));
    await settle();
  };
  return { root, text, type, submit };
}

describe('EmailForReminders', () => {
  it('adds an address once the emailed code is typed', async () => {
    const d = stubDoor();
    const v = await show(d.fetch);
    expect(v.text()).toContain('Email for reminders');
    const email = v.root().findByProps({ id: 'reminder-email' });
    expect(email.props).toMatchObject({ type: 'email', autoComplete: 'email', required: true });

    await v.type('reminder-email', 'ada@example.org');
    await v.submit();
    expect(v.text()).toContain('A code is on its way');
    expect(v.root().findByProps({ id: 'reminder-email-code' }).props.autoComplete).toBe('one-time-code');

    await v.type('reminder-email-code', '00000000');
    await v.submit();
    expect(v.text()).toContain('That code was not accepted');

    await v.type('reminder-email-code', '12345678');
    await v.submit();
    expect(v.text()).toContain('Reminders are emailed to');
    expect(v.text()).toContain('ada@example.org');
    expect(v.root().findAllByProps({ id: 'reminder-email-code' })).toHaveLength(0);
    expect(d.paths).toContain('/auth/email/complete');
  });

  it('says so when the app requires an address', async () => {
    const v = await show(stubDoor({ required: 'all' }).fetch);
    expect(v.text()).toContain('This app needs an email address');
  });

  it('says reminders stay in the app when the deployment sends no mail', async () => {
    const v = await show(stubDoor({ mail: false }).fetch);
    expect(v.text()).toContain('sends no mail');
    expect(v.root().findAllByType('form')).toHaveLength(0);
  });

  it('words the email_required refusal for the page', () => {
    const r = { class: 'email_required', message: 'm' } as Parameters<typeof refusalText>[0];
    expect(refusalText(r)).toContain('Email for reminders');
  });
});
