/**
 * A backend function call goes through the one client and the one error
 * (`docs/howto-functions.md`, "Calling a function").
 */
import { describe, expect, it } from 'vitest';

import { OrdoClient } from '../src/core/client.js';
import { OrdoNetworkError } from '../src/core/network.js';
import { OrdoLocalRefusal, OrdoRefusal } from '../src/core/refusal.js';

interface Seen {
  url: string;
  init: RequestInit;
}

function door(answers: Response[]): { fetch: typeof globalThis.fetch; seen: Seen[] } {
  const seen: Seen[] = [];
  return {
    seen,
    fetch: async (input, init) => {
      seen.push({ url: String(input), init: init ?? {} });
      return answers.shift() ?? new Response('{}', { status: 200 });
    },
  };
}

function client(d: { fetch: typeof globalThis.fetch }, token: string | undefined = 'tok'): OrdoClient {
  return new OrdoClient({
    baseUrl: 'https://door.test/',
    space: 'acme',
    ...(token === undefined ? {} : { token }),
    fetch: d.fetch,
  });
}

const json = (status: number, body: unknown, headers: Record<string, string> = {}): Response =>
  new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json', ...headers } });

describe('OrdoClient.callFunction', () => {
  it('posts the parameters to /fn/<name> as the signed-in person and returns result', async () => {
    const d = door([json(200, { result: { title: 'Groceries' } })]);
    const out = await client(d).callFunction<{ title: string }>('suggestTitle', { text: 'milk, eggs' });
    expect(out.result).toEqual({ title: 'Groceries' });
    expect(out.replayed).toBe(false);
    const [req] = d.seen;
    expect(req?.url).toBe('https://door.test/fn/suggestTitle');
    expect(req?.init.method).toBe('POST');
    const h = req?.init.headers as Record<string, string>;
    expect(h['Authorization']).toBe('Bearer tok');
    expect(h['X-Ordo-Space']).toBe('acme');
    expect(h['Idempotency-Key']).toBeUndefined();
    expect(JSON.parse(String(req?.init.body))).toEqual({ text: 'milk, eggs' });
  });

  it('sends an idempotency key only when given one, and reports a replay', async () => {
    const d = door([json(200, { result: 1 }, { 'X-Ordo-Idempotent-Replay': 'true' })]);
    const out = await client(d).callFunction('f', {}, { idempotencyKey: 'k-1' });
    expect((d.seen[0]?.init.headers as Record<string, string>)['Idempotency-Key']).toBe('k-1');
    expect(out.replayed).toBe(true);
  });

  it('throws a refusal as the one OrdoRefusal, with its class and message', async () => {
    const message = 'secret `llmKey` has no value in this deployment; the deployment owner sets it with `ordo secret set llmKey`';
    const d = door([json(400, { error: { code: 'egress_refused', message } })]);
    const err = await client(d)
      .callFunction('suggestTitle', { text: 'x' })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoRefusal);
    expect((err as OrdoRefusal).class).toBe('egress_refused');
    expect((err as OrdoRefusal).message).toContain('ordo secret set llmKey');
  });

  it('carries the Retry-After of function_busy', async () => {
    const d = door([json(429, { error: { code: 'function_busy', message: 'busy' } }, { 'Retry-After': '3' })]);
    const err = (await client(d)
      .callFunction('f')
      .catch((e: unknown) => e)) as OrdoRefusal;
    expect(err.class).toBe('function_busy');
    expect(err.retryAfterMs).toBe(3000);
  });

  it('refuses a bad name and non-JSON parameters before the wire', async () => {
    const d = door([]);
    const c = client(d);
    await expect(c.callFunction('../op/x')).rejects.toBeInstanceOf(OrdoLocalRefusal);
    await expect(c.callFunction('f', { when: new Date() })).rejects.toMatchObject({ class: 'invalid_param' });
    await expect(c.callFunction('f', { n: Number.NaN })).rejects.toBeInstanceOf(OrdoLocalRefusal);
    expect(d.seen).toHaveLength(0);
  });

  it('names a call that got no answer', async () => {
    const c = client({
      fetch: async () => {
        throw new TypeError('Failed to fetch');
      },
    });
    await expect(c.callFunction('f')).rejects.toBeInstanceOf(OrdoNetworkError);
  });
});
