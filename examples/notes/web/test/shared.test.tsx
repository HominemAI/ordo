/**
 * The guest's screens on the hooks: a window the door will not open, and the
 * hooks' client a window gives a guest. The positive path (a window that opens,
 * syncs, and is closed by the sharer) runs against the real binary in
 * `live.test.ts`.
 */

import { commitOnce } from '@hominemai/ordo-react';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { afterEach, describe, expect, it } from 'vitest';
import { SharedView } from '../src/components/SharedView.js';
import type { Session } from '../src/session.js';
import { linkInto, sharedDevices, windowInto } from '../src/shared.js';
import { door, refusal, tokenFor, type Reply, type Sent } from './support.js';

(globalThis as Record<string, unknown>)['IS_REACT_ACT_ENVIRONMENT'] = true;

let tree: ReactTestRenderer | null = null;
afterEach(() => {
  act(() => tree?.unmount());
  tree = null;
});

const path = (s: Sent) => new URL(s.url).pathname;
const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 25))));

const WINDOW_TOKEN = tokenFor('usr_bo', 7);
const windowOpened: Reply = { json: { token: WINDOW_TOKEN, expires_at: 9_999_999_999, space: 'spc_ada' } };

function sessionOver(d: ReturnType<typeof door>): Session {
  return {
    endpoint: { baseUrl: 'https://door.example', fetch: d.fetch },
    principal: 'usr_bo',
    token: async () => 'session-token',
  } as unknown as Session;
}

describe('the shared view', () => {
  it('says a share was closed when the door will not open its window', async () => {
    const d = door((s) =>
      path(s) === '/auth/spaces'
        ? { json: [{ space: 'spc_ada', name: 'Ada', role: null, via: 'guest' }] }
        : refusal(403, 'share_refused'),
    );
    await act(async () => {
      const session = sessionOver(d);
      tree = create(<SharedView session={session} accepted={[]} devices={sharedDevices(session)} />);
    });
    await settle();
    await settle();
    const text = JSON.stringify((tree as ReactTestRenderer).toJSON());
    expect(text).toContain('"From ","Ada"');
    expect(text).toContain('This share was closed. What it showed has been removed from this device.');
  });
});

describe('a guest’s clients', () => {
  it('give the hooks a client for the shared tenant that sends the window token and holds none', async () => {
    const d = door(windowOpened, { json: { rows: [] } });
    const { hooks, sdk } = await windowInto(sessionOver(d), 'spc_ada');
    expect(hooks.space).toBe('spc_ada');
    expect(sdk.space).toBe('spc_ada');
    await hooks.fetchOp('myNotes');
    expect(d.sent[0]?.url).toBe('https://door.example/auth/window');
    expect(d.sent[0]?.headers['Authorization']).toBe('Bearer session-token');
    expect(d.sent[1]?.headers['Authorization']).toBe(`Bearer ${WINDOW_TOKEN}`);
    expect(d.sent[1]?.headers['X-Ordo-Space']).toBe('spc_ada');
    // The token is not a property of either client.
    expect(JSON.stringify(hooks)).not.toContain(WINDOW_TOKEN);
    expect(JSON.stringify(sdk)).not.toContain(WINDOW_TOKEN);
  });

  it('refuse a write through the window the way the door does, as a typed refusal', async () => {
    const d = door(windowOpened, refusal(403, 'access_denied'));
    const { hooks } = await windowInto(sessionOver(d), 'spc_ada');
    const out = await commitOnce(hooks, 'createNote', { values: { title: 't', body: 'b', tags: [], created: '2026-09-29T10:00:00Z' } });
    expect(out).toMatchObject({ status: 'refused', refusal: { class: 'access_denied' } });
  });

  it('open a link with no session, for the tenant the door named', async () => {
    const d = door({ json: { token: WINDOW_TOKEN, expires_at: 9_999_999_999, space: 'spc_ada' } }, { json: { rows: [] } });
    const { hooks } = await linkInto({ baseUrl: 'https://door.example', fetch: d.fetch }, 'lks_abc');
    expect(hooks.space).toBe('spc_ada');
    await hooks.fetchOp('myNotes');
    expect(d.sent[0]?.headers['Authorization']).toBeUndefined();
    expect(d.sent[1]?.headers['Authorization']).toBe(`Bearer ${WINDOW_TOKEN}`);
  });
});
