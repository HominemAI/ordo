/**
 * Consent is notes v2's grant, on its door (plan-federation-grants.md §3;
 * owner, board `federation-ordo-deployments-share-data-through`, asks 1
 * and 2: one Ordo reaches another only through its door).
 *
 * Each app runs as its own database role in its own schema, and neither
 * role is granted anything in the other's: Postgres refuses note_viewer's
 * role every notes v2 table. note_viewer reads notes v2 only by calling the
 * two ops `grant note_viewer` opens. Its door, asked with no token, answers
 * the public notes only: notes v2's `publicNotes`, called as note_viewer
 * itself.
 *
 * A regression looks like the viewer's role reading a notes v2 table, or a
 * note not tagged `public` in the public answer.
 */

import { expect, test } from '@playwright/test';
import { sqlAs, stack, unique } from '../support.js';

test('Postgres refuses the viewer’s role every notes v2 table: it reads only through the door', () => {
  for (const table of ['notes', '"notesAccess"', 'attachments', '"$commandlog"', 'members']) {
    expect(() => sqlAs('viewer_app', `SELECT 1 FROM notes.${table} LIMIT 1`), table).toThrow(/permission denied/);
  }
});

test('notes v2’s role cannot read the viewer’s schema', () => {
  expect(() => sqlAs('notes_app', 'SELECT 1 FROM viewer."$commandlog" LIMIT 1')).toThrow(/permission denied/);
});

// The public op answers only notes tagged `public`, through both doors, to
// a visitor with no token. Guards reported fix: an op named like the
// external it reads answered every row of the table, unfiltered, on the
// public path; the compiler refuses that name (`op_named_like_entity`).
test('a visitor gets the notes tagged public and no other', async ({ request }) => {
  const open = unique('note_open');
  const closed = unique('note_closed');
  sqlAs(
    'notes_app',
    `INSERT INTO notes.notes ("$space", id, title, tags, created) VALUES
       ('system', '${open}', '${open}', '["work", "public"]', now()),
       ('system', '${closed}', '${closed}', '["work"]', now())`,
  );
  await expect(async () => {
    const res = await request.get(`${stack().viewerDoor}/op/publicNotes`);
    expect(res.status(), await res.text()).toBe(200);
    const titles = ((await res.json()) as { rows: { title: string }[] }).rows.map((r) => r.title);
    expect(titles).toContain(open);
    expect(titles).not.toContain(closed);
  }).toPass({ timeout: 30_000 });
  // Signed out, the person's list is refused.
  expect((await request.get(`${stack().viewerDoor}/op/myNotes`)).status()).toBe(403);
});
