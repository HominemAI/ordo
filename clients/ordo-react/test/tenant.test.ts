/**
 * The tenant status hook and the gate (owner, 2026-10-21): the app
 * developer's own content while a person's tenant is set up, the defaults
 * when a slot is unset, and the app once it is ready.
 */

import { afterEach, describe, expect, it } from 'vitest';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { createElement, type ReactElement, type ReactNode } from 'react';
import { TenantGate, SETTING_UP_TEXT } from '../src/gate.js';
import { useTenantStatus, type TenantPhase, type TenantStatusResult } from '../src/hooks/useTenantStatus.js';
import { readTenantStatus, TenantStatusError } from '../src/tenant.js';

const BASE = 'https://door.example';

type Answer = { status: number; body?: unknown } | 'drop';

/** A door that answers GETs from `reads` in turn (the last repeats) and POSTs from `posts`. */
function door(reads: Answer[], posts: Answer[] = [{ status: 200, body: { state: 'provisioning', space: 'spc_1' } }]) {
  const seen: { method: string; auth: string | null }[] = [];
  let r = 0;
  let p = 0;
  const fetch: typeof globalThis.fetch = async (_input, init) => {
    const method = init?.method ?? 'GET';
    seen.push({ method, auth: new Headers(init?.headers).get('Authorization') });
    const a = method === 'POST' ? posts[Math.min(p++, posts.length - 1)]! : reads[Math.min(r++, reads.length - 1)]!;
    if (a === 'drop') throw new TypeError('Failed to fetch');
    return new Response(a.body === undefined ? null : JSON.stringify(a.body), { status: a.status });
  };
  return { fetch, seen };
}

/** A `wait` the test releases one pause at a time. */
function stepper() {
  const queue: Array<() => void> = [];
  return {
    wait: () => new Promise<void>((resolve) => queue.push(resolve)),
    async next() {
      for (let i = 0; i < 50 && queue.length === 0; i++) await act(async () => {});
      await act(async () => {
        queue.shift()?.();
      });
      await act(async () => {});
    },
  };
}

const settle = async () => {
  for (let i = 0; i < 5; i++) await act(async () => {});
};

const provisioning = (extra: Record<string, unknown> = {}) => ({
  status: 200,
  body: { state: 'provisioning', space: 'spc_1', step: 'custody', stuck: false, attempts: 0, ...extra },
});
const STUCK = provisioning({
  stuck: true,
  attempts: 4,
  reason: 'setting up your account is taking longer than it should. It keeps trying on its own',
  setting: 'ORDO_PROVISION_STUCK_SECS',
});
const READY = { status: 200, body: { state: 'ready', space: 'spc_1' } };

let renderer: ReactTestRenderer | null = null;
afterEach(() => {
  if (renderer) void act(() => renderer?.unmount());
  renderer = null;
});

async function render(node: ReactElement): Promise<ReactTestRenderer> {
  await act(async () => {
    renderer = create(node);
  });
  return renderer!;
}

const text = (r: ReactTestRenderer) => JSON.stringify(r.toJSON());

describe('readTenantStatus', () => {
  it('reads stuck, attempts and the setting, with the bearer token', async () => {
    const d = door([STUCK]);
    const s = await readTenantStatus({ baseUrl: BASE, token: async () => 'tok', fetch: d.fetch });
    expect(s.stuck).toBe(true);
    expect(s.attempts).toBe(4);
    expect(s.setting).toBe('ORDO_PROVISION_STUCK_SECS');
    expect(d.seen[0]).toEqual({ method: 'GET', auth: 'Bearer tok' });
  });

  it('names a refused session and a door it cannot reach', async () => {
    const refused = await readTenantStatus({ baseUrl: BASE, token: 't', fetch: door([{ status: 401 }]).fetch }).catch(
      (e) => e,
    );
    expect((refused as TenantStatusError).problem).toBe('refused');
    const down = await readTenantStatus({ baseUrl: BASE, token: 't', fetch: door(['drop']).fetch }).catch((e) => e);
    expect((down as TenantStatusError).problem).toBe('unavailable');
  });
});

describe('useTenantStatus', () => {
  it('follows a setup through stuck to ready, and reads through a failed read', async () => {
    const d = door([provisioning(), 'drop', STUCK, READY]);
    const phases: TenantPhase[] = [];
    let last: TenantStatusResult | null = null;
    function Probe(): ReactNode {
      last = useTenantStatus({ baseUrl: BASE, token: 't', fetch: d.fetch, wait: async () => {} });
      if (phases[phases.length - 1] !== last.phase) phases.push(last.phase);
      return null;
    }
    await render(createElement(Probe));
    await settle();
    expect(phases).toEqual(['checking', 'setting-up', 'stuck', 'ready']);
    expect(last!.space).toBe('spc_1');
    // Ready stops the reading: four reads, no more.
    expect(d.seen.filter((s) => s.method === 'GET')).toHaveLength(4);
  });

  it('has no time limit while the server keeps trying', async () => {
    const reads: Answer[] = Array.from({ length: 300 }, () => STUCK);
    reads.push(READY);
    const d = door(reads);
    let last: TenantStatusResult | null = null;
    function Probe(): ReactNode {
      last = useTenantStatus({ baseUrl: BASE, token: 't', fetch: d.fetch, wait: async () => {} });
      return null;
    }
    await render(createElement(Probe));
    for (let i = 0; i < 40 && last!.phase !== 'ready'; i++) await settle();
    expect(last!.phase).toBe('ready');
  });
});

describe('TenantGate', () => {
  it("shows the app developer's own content for each phase, then the app with its space", async () => {
    const d = door([provisioning(), STUCK, READY]);
    const step = stepper();
    const r = await render(
      createElement(
        TenantGate,
        {
          baseUrl: BASE,
          token: 't',
          fetch: d.fetch,
          wait: step.wait,
          settingUp: createElement('span', null, 'mine: setting up'),
          stuck: (t: TenantStatusResult) => createElement('span', null, `mine: stuck after ${t.status?.attempts} tries`),
          children: (space: string) => createElement('main', null, `app in ${space}`),
        },
      ),
    );
    await settle();
    expect(text(r)).toContain('mine: setting up');
    await step.next();
    expect(text(r)).toContain('mine: stuck after 4 tries');
    await step.next();
    expect(text(r)).toContain('app in spc_1');
  });

  it('gives plain defaults: a polite status while setting up, the reason while stuck', async () => {
    const d = door([provisioning(), STUCK]);
    const step = stepper();
    const r = await render(
      createElement(TenantGate, { baseUrl: BASE, token: 't', fetch: d.fetch, wait: step.wait, children: 'the app' }),
    );
    await settle();
    const status = r.root.findByProps({ role: 'status' });
    expect(status.type).toBe('p');
    expect(text(r)).toContain(SETTING_UP_TEXT);
    await step.next();
    expect(r.root.findByProps({ role: 'status' }).children.join('')).toContain('keeps trying');
    expect(text(r)).not.toContain('the app');
  });

  it('a failed setup shows an alert with Try again, which starts it again and opens the app', async () => {
    const d = door(
      [{ status: 200, body: { state: 'failed', space: 'spc_0', reason: 'setting up your account stopped' } }, READY],
      [provisioning()],
    );
    const r = await render(
      createElement(TenantGate, { baseUrl: BASE, token: 't', fetch: d.fetch, wait: async () => {}, children: 'the app' }),
    );
    await settle();
    expect(r.root.findByProps({ role: 'alert' })).toBeTruthy();
    expect(text(r)).toContain('setting up your account stopped');
    const button = r.root.findByType('button');
    await act(async () => {
      button.props.onClick();
    });
    await settle();
    expect(d.seen.some((s) => s.method === 'POST')).toBe(true);
    expect(text(r)).toContain('the app');
  });

  it('a refused session offers no Try again', async () => {
    const d = door([{ status: 401 }]);
    const r = await render(
      createElement(TenantGate, { baseUrl: BASE, token: 't', fetch: d.fetch, wait: async () => {}, children: 'the app' }),
    );
    await settle();
    expect(text(r)).toContain('sign in again');
    expect(r.root.findAllByType('button')).toHaveLength(0);
  });
});
