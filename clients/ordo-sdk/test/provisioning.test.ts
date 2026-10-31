/** A person's own tenant, made after sign-up: the status, the retry and the wait. */

import { describe, expect, it } from 'vitest';
import {
  TenantError,
  retryTenant,
  tenantStatus,
  waitForTenant,
  watchTenant,
  type TenantStatus,
} from '../src/auth/provisioning.js';

const BASE = 'https://door.example';
const noPause = async () => {};

/** A door that answers each read with the next of `answers`, then repeats the last. */
function door(answers: ReadonlyArray<{ status: number; body?: unknown } | 'hang' | 'drop'>) {
  const seen: { method: string; url: string; auth: string | null }[] = [];
  let i = 0;
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const headers = new Headers(init?.headers);
    seen.push({ method: init?.method ?? 'GET', url: String(input), auth: headers.get('Authorization') });
    const a = answers[Math.min(i++, answers.length - 1)]!;
    if (a === 'hang') return new Promise<Response>(() => {});
    if (a === 'drop') throw new TypeError('Failed to fetch');
    return new Response(a.body === undefined ? null : JSON.stringify(a.body), { status: a.status });
  };
  return { fetch, seen };
}

describe('a person waits for their own tenant', () => {
  it('reads the status with the bearer token', async () => {
    const d = door([{ status: 200, body: { state: 'provisioning', space: 'spc_1', step: 'custody' } }]);
    const s = await tenantStatus({ baseUrl: BASE, fetch: d.fetch }, 'tok');
    expect(s).toEqual({ state: 'provisioning', space: 'spc_1', step: 'custody' });
    expect(d.seen[0]).toEqual({ method: 'GET', url: `${BASE}/auth/provisioning`, auth: 'Bearer tok' });
  });

  it('resolves with the space once ready, and reports each step on the way', async () => {
    const d = door([
      { status: 200, body: { state: 'provisioning', space: 'spc_1', step: 'reserved' } },
      { status: 200, body: { state: 'provisioning', space: 'spc_1', step: 'database' } },
      { status: 200, body: { state: 'ready', space: 'spc_1' } },
    ]);
    const steps: TenantStatus[] = [];
    const space = await waitForTenant({ baseUrl: BASE, fetch: d.fetch }, async () => 'renewed', {
      wait: noPause,
      onProgress: (s) => steps.push(s),
    });
    expect(space).toBe('spc_1');
    expect(steps.map((s) => s.step ?? s.state)).toEqual(['reserved', 'database', 'ready']);
    expect(d.seen.every((r) => r.auth === 'Bearer renewed')).toBe(true);
  });

  it('keeps reading through a busy or dropped read', async () => {
    const d = door([
      { status: 503, body: { state: 'unavailable' } },
      'drop',
      { status: 200, body: { state: 'ready', space: 'spc_2' } },
    ]);
    await expect(waitForTenant({ baseUrl: BASE, fetch: d.fetch }, 't', { wait: noPause })).resolves.toBe('spc_2');
  });

  it('rejects with the reason when the provision failed', async () => {
    const d = door([{ status: 200, body: { state: 'failed', space: 'spc_3', reason: 'setting up stopped' } }]);
    const err = await waitForTenant({ baseUrl: BASE, fetch: d.fetch }, 't', { wait: noPause }).catch((e) => e);
    expect(err).toBeInstanceOf(TenantError);
    expect(err.failure).toBe('failed');
    expect(err.reason).toBe('setting up stopped');
    expect(err.space).toBe('spc_3');
  });

  it('rejects as off, naming the setting, when the deployment gives no tenants', async () => {
    const d = door([{ status: 200, body: { state: 'off', setting: 'ORDO_PROVISION_SELF_SERVE' } }]);
    const err = await waitForTenant({ baseUrl: BASE, fetch: d.fetch }, 't', { wait: noPause }).catch((e) => e);
    expect(err.failure).toBe('off');
    expect(err.message).toMatch(/ORDO_PROVISION_SELF_SERVE/);
  });

  it('rejects as refused when the session is refused', async () => {
    const d = door([{ status: 401, body: { error: { code: 'unauthenticated' } } }]);
    const err = await waitForTenant({ baseUrl: BASE, fetch: d.fetch }, 't', { wait: noPause }).catch((e) => e);
    expect(err.failure).toBe('refused');
  });

  it('rejects as a timeout, never as unavailable, when it is not ready in time', async () => {
    const d = door([{ status: 200, body: { state: 'provisioning', space: 'spc_4', step: 'database' } }]);
    const err = await waitForTenant({ baseUrl: BASE, fetch: d.fetch }, 't', {
      timeoutMs: 0,
      wait: noPause,
    }).catch((e) => e);
    expect(err).toBeInstanceOf(TenantError);
    expect(err.failure).toBe('timeout');
    expect(err.message).toMatch(/last step: database/);
  });

  it('stops when the caller aborts', async () => {
    const ctl = new AbortController();
    ctl.abort();
    const d = door([{ status: 200, body: { state: 'provisioning' } }]);
    const err = await waitForTenant({ baseUrl: BASE, fetch: d.fetch }, 't', { signal: ctl.signal }).catch((e) => e);
    expect(err.failure).toBe('aborted');
  });

  it('a retry posts and answers with the status; the hour limit is its own failure', async () => {
    const ok = door([{ status: 200, body: { state: 'provisioning', space: 'spc_5', step: 'reserved' } }]);
    expect((await retryTenant({ baseUrl: BASE, fetch: ok.fetch }, 't')).state).toBe('provisioning');
    expect(ok.seen[0]!.method).toBe('POST');
    const full = door([{ status: 429, body: { error: { code: 'quota_exceeded' } } }]);
    const err = await retryTenant({ baseUrl: BASE, fetch: full.fetch }, 't').catch((e) => e);
    expect(err.failure).toBe('limit_reached');
  });
});

describe('the tenant status hook: watchTenant (owner, 2026-10-21)', () => {
  const STUCK = {
    state: 'provisioning',
    space: 'spc_9',
    step: 'custody',
    stuck: true,
    attempts: 3,
    reason: 'setting up your account is taking longer than it should. It keeps trying on its own',
    setting: 'ORDO_PROVISION_STUCK_SECS',
  };

  it('tells each change once, carries stuck through, and resolves when ready', async () => {
    const d = door([
      { status: 200, body: { state: 'provisioning', space: 'spc_9', step: 'reserved', stuck: false, attempts: 0 } },
      { status: 200, body: { state: 'provisioning', space: 'spc_9', step: 'reserved', stuck: false, attempts: 0 } },
      { status: 200, body: STUCK },
      { status: 200, body: STUCK },
      { status: 200, body: { state: 'ready', space: 'spc_9' } },
    ]);
    const told: TenantStatus[] = [];
    const pauses: number[] = [];
    const watch = watchTenant({ baseUrl: BASE, fetch: d.fetch }, 't', (s) => told.push(s), {
      wait: async (ms) => {
        pauses.push(ms);
      },
    });
    await expect(watch.ready).resolves.toBe('spc_9');
    expect(told.map((s) => [s.state, s.stuck ?? null])).toEqual([
      ['provisioning', false],
      ['provisioning', true],
      ['ready', null],
    ]);
    expect(told[1]).toEqual(STUCK);
    // Once stuck it reads every ten seconds rather than every quarter second.
    expect(pauses.slice(2)).toEqual([10_000, 10_000]);
  });

  it('has no time limit: it keeps reading a long setup and through failed reads', async () => {
    const answers: Array<{ status: number; body?: unknown } | 'drop'> = [];
    for (let i = 0; i < 200; i++) answers.push({ status: 200, body: { state: 'provisioning', space: 'spc_8', step: 'database' } });
    answers.push('drop', { status: 503 }, { status: 200, body: { state: 'ready', space: 'spc_8' } });
    const d = door(answers);
    const errors: unknown[] = [];
    const watch = watchTenant({ baseUrl: BASE, fetch: d.fetch }, 't', () => {}, {
      wait: noPause,
      onError: (e) => errors.push(e),
    });
    await expect(watch.ready).resolves.toBe('spc_8');
    expect(errors).toHaveLength(2);
  });

  it('stop() ends it with aborted, and a refused session ends it at once', async () => {
    const d = door([{ status: 200, body: { state: 'provisioning', space: 'spc_7', step: 'reserved' } }]);
    let watch: ReturnType<typeof watchTenant> | null = null;
    watch = watchTenant({ baseUrl: BASE, fetch: d.fetch }, 't', () => watch?.stop(), { wait: noPause });
    const err = await watch.ready.catch((e) => e);
    expect(err).toBeInstanceOf(TenantError);
    expect((err as TenantError).failure).toBe('aborted');

    const refused = door([{ status: 401 }]);
    const e2 = await watchTenant({ baseUrl: BASE, fetch: refused.fetch }, 't', () => {}, { wait: noPause }).ready.catch(
      (e) => e,
    );
    expect((e2 as TenantError).failure).toBe('refused');
  });
});
