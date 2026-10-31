/**
 * Share links on the page: the parameters and answers of the sharing ops, and
 * the route a link opens, rendered as the page renders it. The door is a stub
 * that answers what each test says.
 */

import { commitOnce } from '@hominemai/ordo-react';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { afterEach, describe, expect, it } from 'vitest';
import { LinkView } from '../src/components/LinkView.js';
import { linkSecret } from '../src/hooks.js';
import { madeLinkOf, readLinkFor, readLinksOf, shareLinkParams, type Note } from '../src/notes.js';
import { door, hooksOver, refusal, tokenFor, type Door } from './support.js';

(globalThis as Record<string, unknown>)['IS_REACT_ACT_ENVIRONMENT'] = true;

const note: Note = { id: 'note_w1_1', rev: 1, title: 'Groceries', body: 'milk', tags: [], created: '2026-09-29T10:00:00Z' };

const listed = (over: Record<string, unknown>) => ({
  id: 'lnk_1',
  label: 'Groceries',
  ops: ['myNotes'],
  rows: [{ entity: 'notes', id: 'note_w1_1', level: 'read' }],
  opens: 0,
  createdAt: 1000,
  expiresAt: 5000,
  state: 'live',
  issuedBy: 'usr_ada',
  ...over,
});

describe('the share panel makes a link anyone can read', () => {
  it('asks the platform for a read-only link to that one note and builds the address', async () => {
    const d = door({
      json: { rows: [{ link: 'lks_abc-123', id: 'lnk_1', seat: 'mem_1', expiresAt: 9000, ops: ['myNotes'], outcome: 'minted' }] },
    });
    const out = await commitOnce(hooksOver(d), 'std.space.shareLink', { values: shareLinkParams(note) });
    if (out.status !== 'committed') throw new Error(out.status);
    const sent = d.sent[0];
    expect(sent?.url).toBe('https://door.example/op/std.space.shareLink');
    expect(sent?.method).toBe('POST');
    expect(sent?.body).toEqual({
      ops: 'myNotes',
      rows: JSON.stringify([{ entity: 'notes', id: 'note_w1_1', level: 'read' }]),
      expires: '7d',
      label: 'Groceries',
    });
    const made = madeLinkOf(out.response.rows[0], 'https://notes.inf.smg.icu/');
    expect(made).toEqual({ id: 'lnk_1', url: 'https://notes.inf.smg.icu/#/link/lks_abc-123', expiresAt: 9000 });
    expect(readLinkFor('http://localhost:5173', 'S')).toBe('http://localhost:5173/#/link/S');
  });

  it('says so when the door answers no link', () => {
    expect(() => madeLinkOf({ outcome: 'minted' } as never, 'http://x')).toThrow('did not answer a share link');
  });

  it('passes a refusal on by its class, so the provider can ask for a passkey touch', async () => {
    const d = door(refusal(403, 'access_denied'));
    const out = await commitOnce(hooksOver(d), 'std.space.shareLink', { values: shareLinkParams(note) });
    expect(out).toMatchObject({ status: 'refused', refusal: { class: 'access_denied' } });
  });
});

describe('the list of links', () => {
  it('lists this note’s links with their state, and leaves out other notes', async () => {
    const d = door({
      json: {
        rows: [
          listed({ id: 'lnk_1', opens: 2 }),
          listed({ id: 'lnk_2', rows: [{ entity: 'notes', id: 'note_w2_2', level: 'read' }] }),
          listed({ id: 'lnk_3', state: 'revoked' }),
          listed({ id: 'lnk_4', state: 'expired' }),
        ],
      },
    });
    const res = await hooksOver(d).fetchOp('std.space.links');
    expect(d.sent[0]?.url).toBe('https://door.example/op/std.space.links');
    expect(d.sent[0]?.method).toBe('GET');
    expect(readLinksOf(res.rows, note).map((l) => [l.id, l.state, l.opens])).toEqual([
      ['lnk_1', 'live', 2],
      ['lnk_3', 'revoked', 0],
      ['lnk_4', 'expired', 0],
    ]);
  });

  it('revokes a link by its id', async () => {
    const d = door({ json: { rows: [{ outcome: 'link revoked' }] } });
    await commitOnce(hooksOver(d), 'std.space.revokeLink', { values: { link: 'lnk_1' } });
    expect(d.sent[0]?.url).toBe('https://door.example/op/std.space.revokeLink');
    expect(d.sent[0]?.body).toEqual({ link: 'lnk_1' });
  });
});

describe('the route a link opens', () => {
  it('is found in the address fragment and nowhere else', () => {
    expect(linkSecret('#/link/lks_abc-123')).toBe('lks_abc-123');
    expect(linkSecret('#/accept/tkt')).toBeNull();
    expect(linkSecret('')).toBeNull();
    expect(linkSecret('#/link/')).toBeNull();
    expect(linkSecret('#/link/a/b')).toBeNull();
  });
});

describe('the link view', () => {
  let tree: ReactTestRenderer | null = null;
  afterEach(() => {
    act(() => tree?.unmount());
    tree = null;
  });

  const opened = { token: tokenFor('lnk_x'), expires_at: 9_999_999_999, space: 'spc_ada', ops: ['myNotes'] };
  const row = { id: note.id, rev: 1, title: note.title, body: note.body, created: note.created };

  async function show(d: Door) {
    await act(async () => {
      tree = create(<LinkView secret="lks_abc" endpoint={{ baseUrl: 'https://door.example', fetch: d.fetch }} />);
    });
    for (let i = 0; i < 5; i++) await act(async () => void (await new Promise((r) => setTimeout(r, 10))));
    return JSON.stringify((tree as ReactTestRenderer).toJSON());
  }

  it('opens the link with no session and reads the shared note through a hook', async () => {
    const d = door({ json: opened }, { json: { rows: [row] } });
    const text = await show(d);
    expect(text).toContain('Groceries');
    expect(text).toContain('You can read this note.');
    expect(d.sent[0]?.url).toBe('https://door.example/auth/link');
    expect(d.sent[0]?.body).toEqual({ link: 'lks_abc', device: expect.any(String) });
    // No sign-in anywhere: nothing carries a session, and the read carries the link's token.
    expect(d.sent[0]?.headers['Authorization']).toBeUndefined();
    expect(d.sent[1]?.url).toBe('https://door.example/op/myNotes');
    expect(d.sent[1]?.headers['X-Ordo-Space']).toBe('spc_ada');
    expect(d.sent[1]?.headers['Authorization']).toBe(`Bearer ${opened.token}`);
    expect(d.sent.every((s) => !s.url.includes('/auth/begin') && !s.url.includes('/auth/complete'))).toBe(true);
  });

  it('says a link is closed when the door refuses to open it', async () => {
    const text = await show(door(refusal(403, 'link_refused')));
    expect(text).toContain('This link has expired or was revoked.');
  });

  it('says a link is closed when it is revoked while the page is open', async () => {
    const text = await show(door({ json: opened }, refusal(403, 'access_denied')));
    expect(text).toContain('This link has expired or was revoked.');
    expect(text).not.toContain('Groceries');
  });

  it('does not call an unreachable door a closed link', async () => {
    const fetch: typeof globalThis.fetch = async () => {
      throw new TypeError('Failed to fetch');
    };
    const text = await show({ sent: [], fetch });
    expect(text).not.toContain('expired or was revoked');
    expect(text).toContain('could not be reached');
  });
});
