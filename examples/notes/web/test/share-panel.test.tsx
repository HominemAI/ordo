/**
 * The share panel on the hooks: the platform's sharing ops through
 * `useCommit` and `useOpQuery`, the fresh passkey touch the provider asks for,
 * and the access list that a device never holds. The door is a stub that
 * answers by route.
 */

import { OrdoProvider, type DeviceCopy } from '@hominemai/ordo-react';
import { act, create, type ReactTestInstance, type ReactTestRenderer } from 'react-test-renderer';
import { afterEach, describe, expect, it } from 'vitest';
import { SharePanel } from '../src/components/SharePanel.js';
import type { Note } from '../src/notes.js';
import { door, hooksOver, refusal, type Reply, type Sent } from './support.js';

(globalThis as Record<string, unknown>)['IS_REACT_ACT_ENVIRONMENT'] = true;
// The panel reads the page's origin for the links it builds; Node has no window.
(globalThis as Record<string, unknown>)['window'] = { location: { origin: 'http://localhost:5173' } };

const note: Note = { id: 'note_w1_1', rev: 1, title: 'Groceries', body: 'milk', tags: [], created: '2026-09-29T10:00:00Z' };
const path = (s: Sent) => new URL(s.url).pathname;

let tree: ReactTestRenderer | null = null;
afterEach(() => {
  act(() => tree?.unmount());
  tree = null;
});

const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 25))));

/** A door that answers each route from a table; unlisted routes answer no rows. */
function byRoute(routes: Record<string, Reply | ((s: Sent) => Reply)>) {
  return door((s) => {
    const r = routes[path(s)];
    return (typeof r === 'function' ? r(s) : r) ?? { json: { rows: [] } };
  });
}

async function show(d: ReturnType<typeof door>, extra: { stepUp?: () => Promise<void>; device?: DeviceCopy } = {}) {
  await act(async () => {
    tree = create(
      <OrdoProvider client={hooksOver(d)} {...extra}>
        <SharePanel note={note} online />
      </OrdoProvider>,
    );
  });
  await settle();
  const root = (): ReactTestInstance => (tree as ReactTestRenderer).root;
  const press = async (label: string) => {
    const b = root().findAll((n) => n.type === 'button' && n.children.join('') === label)[0];
    if (!b) throw new Error(`no button ${label}`);
    await act(async () => void (await (b.props as { onClick: () => unknown }).onClick()));
    await settle();
  };
  const text = () => JSON.stringify((tree as ReactTestRenderer).toJSON());
  return { root, press, text };
}

describe('the share panel', () => {
  it('makes a link anyone can read with a hook, shows it once, and lists it', async () => {
    let made = false;
    const d = byRoute({
      '/op/std.space.shareLink': () => {
        made = true;
        return { json: { rows: [{ link: 'lks_abc', id: 'lnk_1', seat: 'mem_1', expiresAt: 9000 }] } };
      },
      '/op/std.space.links': () => ({
        json: {
          rows: made
            ? [{ id: 'lnk_1', label: 'Groceries', rows: [{ entity: 'notes', id: note.id }], opens: 0, expiresAt: 9000, state: 'live' }]
            : [],
        },
      }),
    });
    const ui = await show(d);
    expect(ui.text()).toContain('No links yet.');
    await ui.press('Make a link anyone can read');
    expect(ui.root().findByProps({ 'aria-label': 'Link anyone can read' }).props['value']).toBe('http://localhost:5173/#/link/lks_abc');
    expect(ui.text()).toContain('Revoke link');
    const post = d.sent.find((s) => path(s) === '/op/std.space.shareLink');
    expect(post?.method).toBe('POST');
    expect(post?.headers['X-Ordo-Version']).toBe('platform');
  });

  it('asks for a fresh passkey touch when the door refuses a platform op, and tries it once more', async () => {
    let tries = 0;
    const d = byRoute({
      '/op/std.space.share': () => {
        tries += 1;
        return tries === 1 ? refusal(403, 'access_denied') : { json: { rows: [{ ticket: 'tkt_1' }] } };
      },
    });
    let touches = 0;
    const ui = await show(d, {
      stepUp: async () => {
        touches += 1;
      },
    });
    await ui.press('Make an invitation for a person');
    expect(touches).toBe(1);
    expect(tries).toBe(2);
    expect(ui.root().findByProps({ 'aria-label': 'Invitation to send' }).props['value']).toBe('http://localhost:5173/#/accept/tkt_1');
  });

  it('shows the door’s refusal in the page’s words when a second try is refused too', async () => {
    const d = byRoute({ '/op/std.space.shareLink': refusal(403, 'access_denied') });
    const ui = await show(d, { stepUp: async () => undefined });
    await ui.press('Make a link anyone can read');
    expect(ui.text()).toContain('The deployment did not allow that.');
    expect(d.sent.filter((s) => path(s) === '/op/std.space.shareLink')).toHaveLength(2);
  });

  it('revokes a link by its id with a hook', async () => {
    const d = byRoute({
      '/op/std.space.links': {
        json: {
          rows: [{ id: 'lnk_9', label: 'Groceries', rows: [{ entity: 'notes', id: note.id }], opens: 1, expiresAt: 9000, state: 'live' }],
        },
      },
    });
    const ui = await show(d);
    await ui.press('Revoke link');
    const post = d.sent.find((s) => path(s) === '/op/std.space.revokeLink');
    expect(post?.body).toEqual({ link: 'lnk_9' });
  });

  it('closes one guest’s whole window with a hook, by the seat the access list names', async () => {
    const d = byRoute({
      '/op/std.notesAccess.list': {
        json: { rows: [{ id: 'a2', note: note.id, member: 'mem_guest', level: 'read', rev: 1 }] },
      },
    });
    const ui = await show(d);
    await ui.press('Remove from all my notes');
    const post = d.sent.find((s) => path(s) === '/op/std.space.revokeShare');
    expect(post?.body).toEqual({ seat: 'mem_guest' });
  });

  it('reads who can read a note from the door, never from a device that holds the permission map', async () => {
    const d = byRoute({
      '/op/std.notesAccess.list': {
        json: { rows: [{ id: 'a2', note: note.id, member: 'mem_guest', level: 'read', rev: 1 }] },
      },
    });
    const refusesToHold = {
      canRefine: true,
      revoked: false,
      declare: () => {
        throw new Error('the access list was declared on the device');
      },
      state: () => null,
      read: async () => [],
      watch: () => () => undefined,
      revokeOp: async () => undefined,
      afterCommit: async () => undefined,
      reload: async () => undefined,
      nudge: () => undefined,
    } satisfies DeviceCopy;
    const ui = await show(d, { device: refusesToHold });
    expect(ui.text()).toContain('Reader ');
    expect(d.sent.some((s) => path(s) === '/op/std.notesAccess.list')).toBe(true);
  });
});
