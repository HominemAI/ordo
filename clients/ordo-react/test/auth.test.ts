/**
 * `useOrdoAuth` over the SDK's `createOrdoAuth`: it resumes once, then
 * follows the object through a sign-in and a sign-out.
 */

import { describe, expect, it } from 'vitest';
import { act, create } from 'react-test-renderer';
import { createElement } from 'react';
import { createOrdoAuth, type OrdoAuth } from '@hominemai/ordo-sdk/auth/session.js';
import { useOrdoAuth, type OrdoAuthState } from '../src/hooks/useOrdoAuth.js';

const settle = async () => {
  for (let i = 0; i < 5; i++) await act(async () => {});
};

const wire = (token: string) => ({ token, expires_at: Date.now() / 1000 + 3600, session_expires_at: 9e9 });

function door(routes: Record<string, { status: number; json?: unknown }>) {
  const paths: string[] = [];
  const fetch: typeof globalThis.fetch = async (input) => {
    const path = new URL(String(input)).pathname;
    paths.push(path);
    const r = routes[path] ?? { status: 404 };
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), { status: r.status });
  };
  return { paths, fetch };
}

function mount(auth: OrdoAuth) {
  const seen: OrdoAuthState[] = [];
  function Probe() {
    seen.push(useOrdoAuth(auth));
    return null;
  }
  act(() => {
    create(createElement(Probe));
  });
  return { seen, last: () => seen[seen.length - 1]! };
}

describe('useOrdoAuth', () => {
  it('is signed out when nothing resumes, then follows a sign-in and a sign-out', async () => {
    const d = door({
      '/auth/refresh': { status: 401, json: { error: { code: 'authentication_failed', message: 'x' } } },
      '/auth/complete': { status: 200, json: wire('tok_1') },
      '/auth/logout': { status: 204 },
    });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch, autoRenew: false });
    const m = mount(auth);
    expect(m.seen[0]?.phase).toBe('checking');
    await settle();
    expect(m.last().phase).toBe('signed-out');

    await act(async () => {
      await auth.completeEmail({ challenge: 'c', binder: 'b' }, '123', { display: 'Ana' });
    });
    expect(m.last().phase).toBe('signed-in');
    expect(m.last().session?.token).toBe('tok_1');

    await act(async () => {
      await m.last().auth.signOut();
    });
    expect(m.last().phase).toBe('signed-out');
    expect(d.paths).toEqual(['/auth/refresh', '/auth/complete', '/auth/logout']);
  });

  it('starts signed in when the browser holds a sign-in', async () => {
    const d = door({ '/auth/refresh': { status: 200, json: wire('tok_r') } });
    const auth = createOrdoAuth({ baseUrl: 'https://door.example', fetch: d.fetch, autoRenew: false });
    const m = mount(auth);
    await settle();
    expect(m.last().phase).toBe('signed-in');
    expect(m.last().session?.token).toBe('tok_r');
  });
});
