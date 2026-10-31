/**
 * The note editor on the React building blocks: the page's own component,
 * rendered over `OrdoProvider` and a stubbed door. No DOM, no server.
 *
 * It shows what README section 2 promises when two tabs edit one note: the
 * second save says **Changed elsewhere**, overwrites nothing, and offers
 * **Keep my text** and **Use theirs**.
 */

import { OrdoProvider } from '@hominemai/ordo-react';
import { act, create, type ReactTestInstance, type ReactTestRenderer } from 'react-test-renderer';
import { afterEach, describe, expect, it } from 'vitest';
import { NoteEditor } from '../src/components/NoteEditor.js';
import type { Note } from '../src/notes.js';
import { door, hooksOver, refusal, type Door } from './support.js';

(globalThis as Record<string, unknown>)['IS_REACT_ACT_ENVIRONMENT'] = true;

const opened: Note = { id: 'note_w3_3', rev: 3, title: 'Mine', body: 'my body', tags: ['mine'], created: '2026-09-29T10:00:03Z', pinned: false, archived: null };
const stored = { id: opened.id, rev: 4, title: 'Theirs', body: 'their body', tags: ['theirs'], created: opened.created };

let tree: ReactTestRenderer | null = null;
afterEach(() => {
  act(() => tree?.unmount());
  tree = null;
});

const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 20))));

async function show(d: Door, onClose: () => void) {
  await act(async () => {
    tree = create(
      <OrdoProvider client={hooksOver(d)}>
        <NoteEditor note={opened} online onClose={onClose} />
      </OrdoProvider>,
    );
  });
  const root = (): ReactTestInstance => (tree as ReactTestRenderer).root;
  const button = (label: string) => root().findAll((n) => n.type === 'button' && n.children.join('') === label)[0];
  const press = async (label: string) => {
    const b = button(label);
    if (!b) throw new Error(`no button ${label}`);
    await act(async () => void (await (b.props as { onClick: () => unknown }).onClick()));
    await settle();
  };
  const text = () => JSON.stringify((tree as ReactTestRenderer).toJSON());
  return { root, button, press, text };
}

describe('the editor', () => {
  it('saves at the revision it opened and closes', async () => {
    const d = door({ json: { rows: [] } });
    let closed = 0;
    const ui = await show(d, () => closed++);
    await ui.press('Save');
    expect(d.sent[0]?.body).toEqual({ id: opened.id, rev: 3, title: 'Mine', body: 'my body', tags: ['mine'] });
    expect(closed).toBe(1);
  });

  it('says Changed elsewhere, overwrites nothing, and lands on Keep my text at the stored revision', async () => {
    const d = door(refusal(409, 'cas_miss'), { json: { rows: [stored] } }, { json: { rows: [] } });
    let closed = 0;
    const ui = await show(d, () => closed++);
    await ui.press('Save');
    expect(ui.text()).toContain('Changed elsewhere.');
    expect(ui.text()).toContain('their body');
    expect(closed).toBe(0);
    // Save is held while the conflict stands.
    expect(ui.button('Save')?.props['disabled']).toBe(true);

    await ui.press('Keep my text');
    expect(d.sent.map((s) => s.method)).toEqual(['POST', 'GET', 'POST']);
    expect(d.sent[2]?.body).toEqual({ id: opened.id, rev: 4, title: 'Mine', body: 'my body', tags: ['mine'] });
    expect(closed).toBe(1);
  });

  it('takes their text on Use theirs, and the next save asserts their revision', async () => {
    const d = door(refusal(409, 'cas_miss'), { json: { rows: [stored] } }, { json: { rows: [] } });
    const ui = await show(d, () => undefined);
    await ui.press('Save');
    await ui.press('Use theirs');
    expect(ui.text()).not.toContain('Changed elsewhere.');
    const title = ui.root().findByProps({ id: 'title' });
    expect(title.props['value']).toBe('Theirs');
    await ui.press('Save');
    expect(d.sent[2]?.body).toEqual({ id: opened.id, rev: 4, title: 'Theirs', body: 'their body', tags: ['theirs'] });
  });

  it('says the note was deleted elsewhere when nothing is stored, and keeps the text', async () => {
    const d = door(refusal(409, 'cas_miss'), { json: { rows: [] } });
    const ui = await show(d, () => undefined);
    await ui.press('Save');
    expect(ui.text()).toContain('This note was deleted somewhere else. Your text is still here.');
    expect(ui.root().findByProps({ id: 'title' }).props['value']).toBe('Mine');
  });

  it('reports any other refusal by its class, and does not close', async () => {
    const d = door(refusal(403, 'access_denied'));
    let closed = 0;
    const ui = await show(d, () => closed++);
    await ui.press('Save');
    expect(ui.text()).toContain('The deployment did not allow that.');
    expect(closed).toBe(0);
  });

  it('deletes through the row action the descriptor grants, and closes', async () => {
    const d = door({ json: { rows: [] } });
    let closed = 0;
    const ui = await show(d, () => closed++);
    await ui.press('Delete');
    expect(d.sent[0]?.method).toBe('DELETE');
    expect(d.sent[0]?.url).toBe(`https://door.example/op/deleteNote?id=${opened.id}`);
    expect(closed).toBe(1);
  });
});
