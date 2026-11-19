/**
 * Consent is enforced by Postgres (owner ruling 2026-11-18,
 * `example-apps-notes-v2-plus-note_viewer` asks 14 and 17).
 *
 * notes v2's `reader note_viewer reads notes, notesAccess` grants note_viewer's
 * role those two tables and the sign-in columns. Asked directly, as that
 * role, Postgres answers the two tables and refuses every other notes v2
 * table. The stack sets no ORDO_PEER_SHARED_ROLE, so the two roles are real.
 *
 * And note_viewer's door, asked with no token, answers the public notes only.
 *
 * A regression looks like the viewer's role reading a table the line does
 * not name, a sign-in column it was not granted, or a note not tagged
 * `public` in the public answer.
 */

import { expect, test } from '@playwright/test';
import { sqlAs, stack, unique } from '../support.js';

test('the viewer’s role reads the two named tables of notes v2', () => {
  expect(sqlAs('viewer_app', 'SELECT count(*) >= 0 FROM notes.notes')).toBe('t');
  expect(sqlAs('viewer_app', 'SELECT count(*) >= 0 FROM notes."notesAccess"')).toBe('t');
});

test('Postgres refuses the viewer’s role every notes v2 table the reader line does not name', () => {
  for (const table of ['attachments', 'reminders', '"$commandlog"']) {
    expect(() => sqlAs('viewer_app', `SELECT 1 FROM notes.${table} LIMIT 1`), table).toThrow(/permission denied/);
  }
  // A sign-in table is granted by column: the matching columns, and no other.
  expect(sqlAs('viewer_app', 'SELECT count(principal) >= 0 FROM notes.members')).toBe('t');
  expect(() => sqlAs('viewer_app', 'SELECT "displayName" FROM notes.members LIMIT 1')).toThrow(/permission denied/);
});

// The public op answers only notes tagged `public`, through the real door,
// to a visitor with no token. Guards reported fix: an op named like the
// external it reads (`publicNotes` over `external publicNotes`) answered
// every row of the table, unfiltered, on the public path.
test('a visitor gets the notes tagged public and no other', async ({ request }) => {
  const open = unique('note_open');
  const closed = unique('note_closed');
  sqlAs(
    'notes_app',
    `INSERT INTO notes.notes ("$space", id, title, tags, created) VALUES
       ('system', '${open}', '${open}', '["work", "public"]', now()),
       ('system', '${closed}', '${closed}', '["work"]', now())`,
  );
  const res = await request.get(`${stack().viewerDoor}/op/publicNotes`);
  expect(res.status()).toBe(200);
  const titles = ((await res.json()) as { rows: { title: string }[] }).rows.map((r) => r.title);
  expect(titles).toContain(open);
  expect(titles).not.toContain(closed);
  // Signed out, the guarded list is refused.
  expect((await request.get(`${stack().viewerDoor}/op/myNotes`)).status()).toBe(403);
});

test('notes v2’s role cannot read the viewer’s schema', () => {
  expect(() => sqlAs('notes_app', 'SELECT 1 FROM viewer."$commandlog" LIMIT 1')).toThrow(/permission denied/);
  expect(stack().pg.db).toBeTruthy();
});
