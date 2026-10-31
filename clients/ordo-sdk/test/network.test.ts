/**
 * A call that gets no answer fails with one error, `OrdoNetworkError`, whose
 * kind says what to fix: `timeout`, `unreachable` or `blocked`.
 */

import { beforeEach, describe, expect, it } from 'vitest';
import { call } from '../src/auth/passkey.js';
import { OrdoClient, OrdoNetworkError, namedFetch, syncOp } from '../src/index.js';
import { LocalReplica, MemoryBackend } from '../src/index.js';
import type { OpSpec } from '../src/index.js';
import { resetNetworkProbes } from '../src/core/network.js';

const read = {
  op: 'myNotes',
  version: '0123456789abcdef',
  method: 'GET',
  kind: 'read',
  terminal: ['fetch'],
  paged: false,
  singleRow: false,
  casParam: null,
  params: [],
  refusals: [],
  standingError: null,
  sync: { entity: 'notes', key: 'id' },
  refine: { tier: 'none', fields: [] },
  order: { keys: [], stated: false },
} as unknown as OpSpec;

const DOOR = 'https://door.example';
const PAGE = 'https://page.example';

/** A fetch that fails the way a browser does, and records every request. */
function down(probe: 'answers' | 'fails') {
  const seen: { url: string; mode: string | undefined }[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = String(input);
    seen.push({ url, mode: init?.mode });
    if (init?.mode === 'no-cors' && probe === 'answers') return new Response(null, { status: 200 });
    throw new TypeError('Failed to fetch');
  };
  return { fetch, seen };
}

const client = (fetch: typeof globalThis.fetch, extra: Partial<ConstructorParameters<typeof OrdoClient>[0]> = {}) =>
  new OrdoClient({
    baseUrl: DOOR,
    space: 'spc_a',
    token: 't',
    fetch,
    network: { pageOrigin: PAGE },
    busyRetries: 0,
    ...extra,
  });

beforeEach(() => resetNetworkProbes());

describe('OrdoNetworkError', () => {
  it('a blocked call: the server answers a no-cors probe, so it is up and refuses the origin', async () => {
    const { fetch, seen } = down('answers');
    const err = await client(fetch).request(read, {}).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect(err).toMatchObject({ kind: 'blocked', pageOrigin: PAGE, baseUrl: DOOR });
    expect((err as OrdoNetworkError).message).toContain(`ORDO_CORS_ORIGINS=${PAGE}`);
    expect((err as OrdoNetworkError).message).not.toContain('ORDO_AUTH_ORIGINS');
    expect(seen.map((s) => [s.url, s.mode])).toEqual([
      [`${DOOR}/op/myNotes`, undefined],
      [`${DOOR}/health/live`, 'no-cors'],
    ]);
  });

  it('a blocked sign-in call names ORDO_AUTH_ORIGINS as well', async () => {
    const { fetch } = down('answers');
    const err = await call({ baseUrl: DOOR, fetch, network: { pageOrigin: PAGE } }, '/auth/begin', {}).catch(
      (e: unknown) => e,
    );
    expect(err).toMatchObject({ kind: 'blocked', settings: ['ORDO_CORS_ORIGINS', 'ORDO_AUTH_ORIGINS'] });
    expect((err as Error).message).toContain(`ORDO_AUTH_ORIGINS=${PAGE}`);
  });

  it('an unreachable call: the probe fails too, so the server could not be reached', async () => {
    const { fetch } = down('fails');
    const err = await client(fetch).request(read, {}).catch((e: unknown) => e);
    expect(err).toMatchObject({ kind: 'unreachable', option: null, settings: [] });
    expect((err as Error).message).toBe(`The server at ${DOOR} could not be reached.`);
  });

  it('a call to the page\'s own origin is never blocked, and sends no probe', async () => {
    const { fetch, seen } = down('answers');
    const err = await client(fetch, { network: { pageOrigin: DOOR } })
      .request(read, {})
      .catch((e: unknown) => e);
    expect(err).toMatchObject({ kind: 'unreachable' });
    expect(seen).toHaveLength(1);
  });

  it('with no page origin (Node) a failure is unreachable and sends no probe', async () => {
    const { fetch, seen } = down('answers');
    const err = await client(fetch, { network: {} }).request(read, {}).catch((e: unknown) => e);
    expect(err).toMatchObject({ kind: 'unreachable' });
    expect(seen).toHaveLength(1);
  });

  it('a slow call is a timeout that names the option that sets the wait', async () => {
    const never: typeof globalThis.fetch = () => new Promise<Response>(() => {});
    const err = await client(never, { callTimeoutMs: 20 }).request(read, {}).catch((e: unknown) => e);
    expect(err).toMatchObject({ kind: 'timeout', option: 'callTimeoutMs' });
    expect((err as Error).message).toContain('callTimeoutMs');
    expect((err as Error).message).toContain('20 ms');
  });

  it('a burst of failures sends one probe', async () => {
    const { fetch, seen } = down('answers');
    const c = client(fetch);
    const errs = await Promise.all([1, 2, 3].map(() => c.request(read, {}).catch((e: unknown) => e)));
    for (const e of errs) expect(e).toMatchObject({ kind: 'blocked' });
    expect(seen.filter((s) => s.mode === 'no-cors')).toHaveLength(1);
  });

  it('the probe answer is kept for probeCacheMs and no longer', async () => {
    const { fetch, seen } = down('answers');
    const c = client(fetch, { network: { pageOrigin: PAGE, probeCacheMs: 0 } });
    await c.request(read, {}).catch(() => undefined);
    await c.request(read, {}).catch(() => undefined);
    expect(seen.filter((s) => s.mode === 'no-cors')).toHaveLength(2);
  });

  it('the probe path is an option', async () => {
    const { fetch, seen } = down('answers');
    await client(fetch, { network: { pageOrigin: PAGE, probePath: '/health/ready' } })
      .request(read, {})
      .catch(() => undefined);
    expect(seen.at(-1)?.url).toBe(`${DOOR}/health/ready`);
  });

  it("the caller's own abort is not a network error", async () => {
    const { fetch } = down('answers');
    const ctl = new AbortController();
    const slow: typeof globalThis.fetch = (input, init) => {
      ctl.abort();
      return fetch(input, init);
    };
    const err = await client(slow).request(read, {}, { signal: ctl.signal }).catch((e: unknown) => e);
    expect(err).not.toBeInstanceOf(OrdoNetworkError);
  });

  it('namedFetch gives any client that takes a fetch the same errors, and passes answers through', async () => {
    const { fetch } = down('answers');
    const named = namedFetch({ baseUrl: DOOR, fetch, pageOrigin: PAGE });
    await expect(named(`${DOOR}/op/x`)).rejects.toMatchObject({ kind: 'blocked' });
    const fine = namedFetch({ baseUrl: DOOR, fetch: async () => new Response('{}', { status: 200 }) });
    expect((await fine(`${DOOR}/op/x`)).status).toBe(200);
  });

  it('opening a feed names the failure', async () => {
    const { fetch } = down('answers');
    const err = await client(fetch).openFeed(read, {}).catch((e: unknown) => e);
    expect(err).toMatchObject({ kind: 'blocked' });
  });

  it('a feed that breaks mid-stream is unreachable', async () => {
    let sent = false;
    const fetch: typeof globalThis.fetch = async () => {
      const body = new ReadableStream<Uint8Array>({
        pull(controller) {
          if (sent) {
            controller.error(new TypeError('network error'));
            return;
          }
          sent = true;
          controller.enqueue(
            new TextEncoder().encode(
              'event: load\ndata: {"entity":"notes","key":"id","position":1,"pin":null,"rows":[],"loaded":0,"complete":true}\n\n',
            ),
          );
        },
      });
      return new Response(body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
    };
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_a', 'usr_1');
    const err = await syncOp(client(fetch), replica, read).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect(err).toMatchObject({ kind: 'unreachable' });
  });
});
