/**
 * The one request helper and the one error shape (audit-service-interfaces.md,
 * fix order step 7).
 */
import { describe, expect, it } from 'vitest';

import { busyRetryOf, conflictRetryOf, reconnectDelayMs } from '../src/core/busy.js';
import { NO_RETRY, busyOnly, doorRequest, refusalOf } from '../src/core/door.js';
import { OrdoNetworkError } from '../src/core/network.js';
import { OrdoLocalRefusal, OrdoRefusal, decodeRefusal } from '../src/core/refusal.js';
import { PasskeyError } from '../src/auth/passkey.js';
import { DelegationError } from '../src/auth/delegation.js';
import { ServerBusyError } from '../src/auth/request.js';

const BASE = 'https://door.test';
const site = (fetch: typeof globalThis.fetch) => ({ baseUrl: BASE, fetch, timeoutOption: 'callTimeoutMs' });
const busy = (retryAfter = '0') =>
  new Response(JSON.stringify({ error: { code: 'server_busy', message: 'busy' } }), {
    status: 503,
    headers: { 'Retry-After': retryAfter },
  });
const fast = busyRetryOf({ retries: 2, firstDelayMs: 0, maxDelayMs: 0, jitter: false });

describe('doorRequest', () => {
  it('sends a busy call again under the policy, and once without one', async () => {
    let n = 0;
    const fetch: typeof globalThis.fetch = async () => (++n < 3 ? busy() : new Response('{}', { status: 200 }));
    const ok = await doorRequest(site(fetch), `${BASE}/x`, {}, { retry: busyOnly(fast) });
    expect(ok.status).toBe(200);
    expect(n).toBe(3);

    n = 0;
    const once = await doorRequest(site(fetch), `${BASE}/x`, {}, { retry: NO_RETRY });
    expect(once.status).toBe(503);
    expect(n).toBe(1);
  });

  it('leaves a conflict to the caller unless the policy retries it', async () => {
    let n = 0;
    const fetch: typeof globalThis.fetch = async () => {
      n += 1;
      return new Response(JSON.stringify({ error: { code: 'serialization_conflict', message: 'x' } }), { status: 409 });
    };
    await doorRequest(site(fetch), `${BASE}/x`, {}, { retry: busyOnly(fast) });
    expect(n).toBe(1);
    n = 0;
    const conflict = conflictRetryOf({ retries: 1, firstDelayMs: 0, maxDelayMs: 0, jitter: false });
    await doorRequest(site(fetch), `${BASE}/x`, {}, { retry: { busy: fast, conflict } });
    expect(n).toBe(2);
  });

  it('names a call that got no answer', async () => {
    const fetch: typeof globalThis.fetch = async () => {
      throw new TypeError('Failed to fetch');
    };
    await expect(doorRequest(site(fetch), `${BASE}/x`, {})).rejects.toBeInstanceOf(OrdoNetworkError);
    await expect(doorRequest(site(fetch), `${BASE}/x`, {}, { named: false })).rejects.toBeInstanceOf(TypeError);
  });

  it("reads a refusal with the door's Retry-After", async () => {
    const r = await refusalOf(busy('3'), 'notes.list');
    expect(r).toBeInstanceOf(OrdoRefusal);
    expect(r.retryAfterMs).toBe(3000);
    expect(r.retryable).toBe(true);
    expect(r.disposition).toBe('retry');
    expect(r.status).toBe(503);
  });
});

describe('one error shape', () => {
  it('decodes an edge word as its own class', () => {
    const d = decodeRefusal({ error: { code: 'authentication_failed', message: 'no' } }, 401, '', []);
    expect(d.class).toBe('authentication_failed');
    expect(d.code).toBe('authentication_failed');
  });

  it('keeps an unknown code verbatim beside the internal class', () => {
    const d = decodeRefusal({ error: { code: 'brand_new', message: 'x' } }, 400, 'op', []);
    expect(d.class).toBe('internal');
    expect(d.code).toBe('brand_new');
    expect(new OrdoRefusal(d).disposition).toBe('fault');
  });

  it('says so when a non-2xx has no envelope', () => {
    const d = decodeRefusal('<html>', 502, 'op', []);
    expect(d.class).toBe('internal');
    expect(d.message).toContain('without an Ordo error envelope');
  });

  it('gives the detail under both names, and marks a local refusal', () => {
    const r = new OrdoLocalRefusal({ class: 'missing_param', message: 'm', op: 'o', param: 'p' });
    expect(r.refusal).toBe(r.detail);
    expect(r.local).toBe(true);
    expect(r.status).toBe(0);
    expect(r.code).toBe('missing_param');
  });

  it("makes every module's own error an OrdoRefusal", () => {
    const p = new PasskeyError('linking_off', 'off', { setting: 'S', status: 403, code: 'linking_off' });
    expect(p).toBeInstanceOf(OrdoRefusal);
    expect(p.message).toBe('off');
    expect([p.failure, p.class, p.code, p.status]).toEqual(['linking_off', 'linking_off', 'linking_off', 403]);

    const local = new PasskeyError('malformed', 'unreadable');
    expect([local.code, local.status]).toEqual(['malformed', 0]);

    const d = new DelegationError('declined', 'no', 'delegation_declined', 403);
    expect(d).toBeInstanceOf(OrdoRefusal);
    expect([d.code, d.status]).toEqual(['delegation_declined', 403]);

    const b = new ServerBusyError(2000);
    expect(b).toBeInstanceOf(OrdoRefusal);
    expect([b.class, b.retryAfterMs, b.retryable, b.failure]).toEqual(['server_busy', 2000, true, 'busy']);
  });
});

describe('the one reconnect backoff', () => {
  it('doubles from 250 ms to 30 s and never undercuts Retry-After', () => {
    expect([0, 1, 2, 3].map((n) => reconnectDelayMs(n))).toEqual([0, 250, 500, 1000]);
    expect(reconnectDelayMs(20)).toBe(30_000);
    expect(reconnectDelayMs(1, 4000)).toBe(4000);
    expect(reconnectDelayMs(1, 90_000)).toBe(30_000);
  });
});
