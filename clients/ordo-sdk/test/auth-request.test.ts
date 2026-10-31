/**
 * Every sign-in and account call goes through one request helper,
 * `authRequest` (owner ruling 2026-10-23: busy versus broken through one
 * interface). A `server_busy` is sent again under `busyRetry` and, past the
 * last retry, is a `ServerBusyError`; a broken store is not retried and stays
 * the module's own `unavailable`.
 */

import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { profile } from '../src/auth/access.js';
import { beginEmailSignIn } from '../src/auth/email.js';
import { tenantStatus } from '../src/auth/provisioning.js';
import { resumeSession } from '../src/auth/resume.js';
import { listSpaces } from '../src/auth/share.js';
import { ServerBusyError } from '../src/auth/request.js';

const BUSY = {
  status: 503,
  headers: { 'Retry-After': '1' },
  json: { error: { code: 'server_busy', message: 'every database connection is in use' } },
};
const BROKEN = {
  status: 503,
  json: { error: { code: 'credential_store_unavailable', message: 'x' } },
};
const fast = { retries: 2, firstDelayMs: 0, maxDelayMs: 0, jitter: false };

function door(...replies: Array<{ status?: number; json?: unknown; headers?: Record<string, string> }>) {
  const sent: string[] = [];
  let i = 0;
  const fetch: typeof globalThis.fetch = async (input) => {
    sent.push(String(input));
    const r = replies[Math.min(i++, replies.length - 1)] ?? { status: 500 };
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), {
      status: r.status ?? 200,
      headers: r.headers ?? {},
    });
  };
  return { sent, fetch };
}

const at = (fetch: typeof globalThis.fetch) => ({
  baseUrl: 'https://door.example',
  fetch,
  busyRetry: fast,
});

/** One call per kind of auth module, each reached through the helper. */
const calls: Array<[string, (f: typeof globalThis.fetch) => Promise<unknown>, unknown]> = [
  ['the shared `call` (email sign-in)', (f) => beginEmailSignIn(at(f), { address: 'kim@example.org' }), {
    challenge: 'ch_1',
    binder: 'b'.repeat(64),
  }],
  ['an op from `access.ts`', (f) => profile({ ...at(f), space: 'spc_a', token: 't' }), { rows: [] }],
  ['`/auth/provisioning`', (f) => tenantStatus(at(f), 't'), { state: 'none' }],
  ['`/auth/refresh`', (f) => resumeSession(at(f)), { token: 't', expires_at: 1, session_expires_at: 2 }],
  ['`/auth/spaces`', (f) => listSpaces(at(f), 't'), []],
];

describe('authRequest: one busy path for every auth module', () => {
  for (const [name, run, ok] of calls) {
    it(`${name} sends server_busy again, and is served`, async () => {
      const d = door(BUSY, { json: ok });
      await run(d.fetch);
      expect(d.sent).toHaveLength(2);
      expect(d.sent[1]).toBe(d.sent[0]);
    });

    it(`${name} throws ServerBusyError once the retries run out`, async () => {
      const d = door(BUSY);
      const err = await run(d.fetch).catch((e: unknown) => e);
      expect(err).toBeInstanceOf(ServerBusyError);
      expect(err).toMatchObject({ failure: 'busy', retryAfterMs: 1000 });
      expect(d.sent).toHaveLength(3);
    });

    it(`${name} does not retry a broken store`, async () => {
      const d = door(BROKEN);
      const err = await run(d.fetch).catch((e: unknown) => e);
      expect(err).not.toBeInstanceOf(ServerBusyError);
      expect(d.sent).toHaveLength(1);
    });
  }
});

describe('no auth module calls fetch itself', () => {
  it('every file in src/auth reaches the door through request.ts', () => {
    const dir = join(__dirname, '..', 'src', 'auth');
    const files = readdirSync(dir).filter((f) => f.endsWith('.ts') && f !== 'request.ts');
    expect(files.length).toBeGreaterThan(10);
    // Assembled, so this file holds none of the needles.
    const needles = [
      ['globalThis', '.fetch.bind'].join(''),
      ['fetch', 'Named('].join(''),
      ['fetch', 'Retrying('].join(''),
      ['fetch', 'WithDeadline('].join(''),
      ['endpoint', '.fetch ??'].join(''),
    ];
    const wrong: string[] = [];
    for (const f of files) {
      // `deadline.ts` defines `fetchWithDeadline`, the helper's own deadline.
      if (f === 'deadline.ts') continue;
      const body = readFileSync(join(dir, f), 'utf8');
      body.split('\n').forEach((line, i) => {
        for (const n of needles) if (line.includes(n)) wrong.push(`${f}:${i + 1}: ${line.trim()}`);
      });
    }
    expect(wrong, 'call `authRequest` (src/auth/request.ts) instead').toEqual([]);
  });
});
