/**
 * Reminders on notes (owner ruling 2026-10-22): the pure helpers, the
 * control in the editor, and the notice at the top of the list. The door is
 * a stub that answers by route. No DOM, no server.
 */

import { OrdoProvider } from '@hominemai/ordo-react';
import { act, create, type ReactTestInstance, type ReactTestRenderer } from 'react-test-renderer';
import { afterEach, describe, expect, it } from 'vitest';
import { ReminderControl, ReminderNotices, RemindersOf } from '../src/components/Reminders.js';
import type { Note } from '../src/notes.js';
import { byNote, firedOnly, localInputOf, reminderOf, stampOfLocalInput, type Reminder } from '../src/reminders.js';
import { door, hooksOver, type Reply, type Sent } from './support.js';

(globalThis as Record<string, unknown>)['IS_REACT_ACT_ENVIRONMENT'] = true;

const note: Note = { id: 'note_w1_1', rev: 1, title: 'Dentist', body: '', tags: [], created: '2026-10-22T10:00:00Z', pinned: false, archived: null };
const path = (s: Sent) => new URL(s.url).pathname;

const coming: Reminder = { id: 'remi_w1_1', rev: 1, note: note.id, due: '2026-10-22T15:30:00Z', fired: null };
const fired: Reminder = { ...coming, rev: 2, fired: '2026-10-22T15:30:12Z' };

let tree: ReactTestRenderer | null = null;
afterEach(() => {
  act(() => tree?.unmount());
  tree = null;
});

const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 25))));

function byRoute(routes: Record<string, Reply>) {
  return door((s) => routes[path(s)] ?? { json: { rows: [] } });
}

async function show(d: ReturnType<typeof door>, ui: React.ReactNode) {
  await act(async () => {
    tree = create(<OrdoProvider client={hooksOver(d)}>{ui}</OrdoProvider>);
  });
  await settle();
  const root = (): ReactTestInstance => (tree as ReactTestRenderer).root;
  /** A button by the start of its visible text. */
  const button = (label: string) =>
    root().findAll((n) => n.type === 'button' && typeof n.children[0] === 'string' && (n.children[0] as string).startsWith(label))[0];
  const press = async (label: string) => {
    const b = button(label);
    if (!b) throw new Error(`no button ${label}`);
    await act(async () => void (await (b.props as { onClick: () => unknown }).onClick()));
    await settle();
  };
  const text = () => JSON.stringify((tree as ReactTestRenderer).toJSON());
  return { root, button, press, text };
}

describe('reminder helpers', () => {
  it('reads a row, and a null or empty fired is a reminder still to come', () => {
    expect(reminderOf({ id: 'r', rev: '2', note: 'n', due: '2026-10-22T15:30:00Z', fired: null })).toEqual({
      id: 'r',
      rev: 2,
      note: 'n',
      due: '2026-10-22T15:30:00Z',
      fired: null,
    });
    expect(reminderOf({ id: 'r', rev: 1, note: 'n', due: 'x', fired: '' }).fired).toBeNull();
  });

  it('lists only what fired, the earliest due first, and finds a note’s reminder', () => {
    const later: Reminder = { ...fired, id: 'b', note: 'other', due: '2026-10-23T08:00:00Z' };
    expect(firedOnly([later, coming, fired]).map((r) => r.id)).toEqual([fired.id, 'b']);
    expect(byNote([coming]).get(note.id)).toBe(coming);
  });

  it('turns a datetime-local value into a UTC stamp and back, in the local time zone', () => {
    const stamp = stampOfLocalInput('2026-10-22T17:45');
    expect(stamp).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:00Z$/);
    expect(localInputOf(stamp as string)).toBe('2026-10-22T17:45');
    expect(new Date(stamp as string).getTime()).toBe(new Date(2026, 9, 22, 17, 45).getTime());
    for (const bad of ['', '2026-10-22', 'tomorrow']) expect(stampOfLocalInput(bad)).toBeNull();
  });
});

describe('the reminder control', () => {
  it('sets a reminder at the time typed, and touches nothing on the note', async () => {
    const d = byRoute({});
    const ui = await show(
      d,
      <RemindersOf value={new Map()}>
        <ReminderControl note={note.id} online />
      </RemindersOf>,
    );
    expect(ui.button('Set reminder')?.props['disabled']).toBe(true);
    const input = ui.root().findByProps({ type: 'datetime-local' });
    await act(async () => (input.props as { onChange: (e: unknown) => void }).onChange({ target: { value: '2026-10-22T17:45' } }));
    await ui.press('Set reminder');
    const sent = d.sent.find((s) => path(s) === '/op/setReminder');
    expect(sent?.method).toBe('POST');
    expect(sent?.body).toEqual({ note: note.id, due: stampOfLocalInput('2026-10-22T17:45') });
    expect(d.sent.some((s) => path(s).includes('Note'))).toBe(false);
  });

  it('shows a set reminder and takes it off', async () => {
    const d = byRoute({});
    const ui = await show(
      d,
      <RemindersOf value={byNote([coming])}>
        <ReminderControl note={note.id} online />
      </RemindersOf>,
    );
    expect(ui.text()).toContain('A reminder is set for');
    expect(ui.root().findByProps({ type: 'datetime-local' }).props['value']).toBe(localInputOf(coming.due));
    await ui.press('Remove reminder');
    const sent = d.sent.find((s) => path(s) === '/op/clearReminder');
    expect(sent?.method).toBe('DELETE');
    expect(new URL(sent?.url as string).searchParams.get('note')).toBe(note.id);
  });

  it('renders nothing in an editor shown on its own', async () => {
    const d = byRoute({});
    const ui = await show(d, <ReminderControl note={note.id} online />);
    expect((tree as ReactTestRenderer).toJSON()).toBeNull();
    expect(d.sent).toEqual([]);
    void ui;
  });
});

describe('the reminder notice', () => {
  it('shows a fired reminder in a polite live region, and Done takes it off', async () => {
    const d = byRoute({ '/op/myReminders': { json: { rows: [fired] } } });
    const ui = await show(d, <ReminderNotices notes={[note]} online />);
    const region = ui.root().findByProps({ role: 'status' });
    expect(region.props['aria-live']).toBe('polite');
    expect(ui.text()).toContain('Dentist');
    expect(ui.text()).toContain('was due');
    await ui.press('Done');
    expect(d.sent.some((s) => path(s) === '/op/clearReminder' && s.method === 'DELETE')).toBe(true);
  });

  it('keeps the live region in the page, empty, while nothing has fired', async () => {
    const d = byRoute({ '/op/myReminders': { json: { rows: [coming] } } });
    const ui = await show(d, <ReminderNotices notes={[note]} online />);
    expect(ui.root().findByProps({ role: 'status' }).children).toEqual([]);
    expect(ui.text()).not.toContain('Dentist');
  });
});
