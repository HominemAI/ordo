/**
 * notes renames a column note_viewer reads (plan-versions.md increment 7b;
 * plan-evolution-federation.md §7, tests 1 to 4 and 6).
 *
 * notes moves from 0.3.0 to 0.3.1 (fixtures/notes-heading), which renames
 * `notes.title` to `heading` with `prev(title)`. note_viewer maps `title`.
 * The rename keeps the old column and keeps it written, so the viewer reads
 * through it: a note from before shows with its title, and so does a note
 * written in notes after the rename. The viewer's role still reads `title`,
 * and Postgres holds the same value under both names.
 *
 * Before the rename runs, the developer stores 0.3.1 and renders the step
 * with `ordo evolve`, signed in through the identity service
 * (tests/developer.ts): it names note_viewer and the date its grant on the
 * old column ends. While the old column is still written, the viewer's
 * `/health/ready` names it as changing.
 *
 * Then notes lands the rename: 0.3.2 drops the `prev` mark, so nothing
 * writes `title` any more. It is an orphan, notes takes it out of the
 * viewer's grant (owner ruling 2026-11-23, Q1), and the viewer's page names
 * the column, its new name and the fix. The viewer maps `heading` (0.1.1)
 * and shows every note again. `ordo rollback --dry-run` to 0.1.0 then says
 * that version can no longer read `title`, and changes nothing.
 *
 * A regression looks like the viewer held or empty after the rename, a
 * note shown with no title, a note written after the rename missing at
 * the viewer, or, once the rename lands, the viewer still listing notes
 * from a column nobody writes.
 *
 * The steps share the stack's state, so they run in order. They change
 * notes' program for the whole stack, so they run after `v3` and before
 * `orphans` (playwright.config.ts).
 */

import { copyFileSync, cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { developer, door, type Developer } from '../developer.js';
import { addNote, applyNotes, joinNotes, list, ordoAs, person, shows, sqlAs, stack, unique } from '../support.js';

test.describe.configure({ mode: 'serial' });

/** The note written before the rename, shown at every step. */
let before = '';

/** The deployment's developer, signed in once for every step. */
let dev: Developer;

test.beforeAll(async ({ browser }) => {
  dev = await developer(browser);
});

test.afterAll(async () => {
  await dev?.close();
});

/** The 0.3.1 program: the fixture as `notes.ordo`, beside notes v3's `fn`. */
function withHeading(): string {
  const dir = mkdtempSync(join(tmpdir(), 'notes-heading-'));
  copyFileSync(
    fileURLToPath(new URL('../../fixtures/notes-heading.ordo.fixture', import.meta.url)),
    join(dir, 'notes.ordo'),
  );
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

/**
 * The 0.3.2 program: 0.3.1 with its `prev` mark cut, as `ordo apply
 * --remove-landed-prev` cuts it. Nothing writes `title` from here.
 */
function landed(): string {
  const dir = withHeading();
  const source = readFileSync(join(dir, 'notes.ordo'), 'utf8');
  const mark = 'heading: text = "" prev(title)';
  expect(source).toContain(mark);
  const cut = source.replace('domain notes @ 0.3.1', 'domain notes @ 0.3.2').replace(mark, 'heading: text = ""');
  writeFileSync(join(dir, 'notes.ordo'), cut);
  return dir;
}

/** note_viewer at 0.1.1: both translations read `title` from `heading`. */
function viewerOnHeading(): string {
  const dir = mkdtempSync(join(tmpdir(), 'viewer-heading-'));
  const source = readFileSync(fileURLToPath(new URL('../../../viewer.ordo', import.meta.url)), 'utf8');
  const mapped = source
    .replace('domain noteviewer @ 0.1.0', 'domain noteviewer @ 0.1.1')
    .replaceAll('title: text <- title', 'title: text <- heading');
  expect(mapped.match(/<- heading/g)).toHaveLength(2);
  writeFileSync(join(dir, 'viewer.ordo'), mapped);
  return dir;
}

/** What the viewer's door logged so far. */
function viewerLog(): string {
  return readFileSync(`${stack().logs}/viewer.log`, 'utf8');
}

test('notes renames `title` to `heading`: the viewer reads every note through it, with its title', async ({
  browser,
}) => {
  test.setTimeout(300_000);
  const di = await person(browser);
  const notes = di.page;
  await joinNotes(notes, 'Di');
  before = unique('Public before the rename');
  await addNote(notes, before, 'Written under 0.3.0.', 'public');

  const viewer = await di.context.newPage();
  await viewer.goto(stack().viewerPage);
  await shows(viewer, 'Public notes', before);

  // §7, test 2: the step is rendered through notes' door before it runs,
  // and names the reader and the date its grant on the old column ends.
  const program = withHeading();
  const stored = dev.ordo(['apply', program, '--no-activate', ...door('notes')]);
  expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
  expect(stored.out).toContain('activated  no');
  const plan = dev.ordo(['evolve', '--from', '0.3.0', '--to', '0.3.1', ...door('notes')]);
  expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
  expect(plan.out).toContain('reader `note_viewer` reads `notes`');
  expect(plan.out).toMatch(/grant on the old column ends no earlier than \d{4}-\d{2}-\d{2}/);

  const applied = applyNotes(program);
  expect(applied).toContain('0.3.1');

  // Postgres: the viewer's role still reads `title`, and the new column
  // holds the same value.
  const q = `SELECT title || '|' || heading FROM notes.notes WHERE title = '${before}'`;
  await expect.poll(() => sqlAs('viewer_app', q), { timeout: 30_000 }).toBe(`${before}|${before}`);

  // The viewer reads through the rename: the note from before, by its title.
  await shows(viewer, 'Public notes', before);
  await expect(viewer.getByRole('alert')).toHaveCount(0);

  // A note written after the rename reaches the old column too, so the
  // viewer shows it with its title.
  const after = unique('Public after the rename');
  await addNote(notes, after, 'Written under 0.3.1.', 'public');
  await shows(viewer, 'Public notes', after);
  await expect(list(viewer, 'Public notes').getByRole('heading', { name: after })).toBeVisible();

  // The viewer is told while it can still switch (§3.3): its watcher logs
  // the column as changing, with the new name, and holds nothing.
  await expect
    .poll(viewerLog, { timeout: 30_000 })
    .toMatch(/changing: `notes\.notes\.title`, which `published` reads as `title`, is renamed to `heading` by `notes`/);
  // And on its `/health/ready`, for the deployment's people.
  await expect
    .poll(
      async () => {
        const r = await fetch(`${stack().viewerDoor}/health/ready`, {
          headers: { authorization: `Bearer ${dev.token()}` },
        });
        const body = (await r.json()) as { external?: { changing?: string[] } };
        return (body.external?.changing ?? []).join('\n');
      },
      { timeout: 30_000 },
    )
    .toContain('is renamed to `heading` by `notes`');
  await di.context.close();
});

// plan-evolution-federation.md §7, test 3: the rename lands. Nothing writes
// `title`, so the viewer loses it by a column revoke (owner ruling
// 2026-11-23, Q1), and its page shows `external_changed` naming `heading`.
test('the rename lands: the viewer is refused naming `heading` and the fix', async ({ page }) => {
  test.setTimeout(300_000);
  const applied = applyNotes(landed());
  expect(applied).toContain('0.3.2');
  expect(applied).toContain('orphaned    `notes.title`');

  // Postgres is the control: `title` is refused, `heading` reads.
  expect(() => sqlAs('viewer_app', 'SELECT title FROM notes.notes LIMIT 1')).toThrow(/permission denied/);
  expect(sqlAs('viewer_app', `SELECT heading FROM notes.notes WHERE heading = '${before}'`)).toBe(before);

  // The page names the column, its new name and the fix.
  await page.goto(stack().viewerPage);
  await expect(async () => {
    await page.reload();
    const alert = page.getByRole('alert');
    await expect(alert).toBeVisible({ timeout: 3_000 });
    await expect(alert).toContainText('external_changed');
    await expect(alert).toContainText('column `title` (it is now `heading`) is no longer written by `notes`');
    await expect(alert).toContainText('ordo introspect --external');
  }).toPass({ timeout: 90_000 });
});

// §7, test 4: the viewer maps `heading` and applies. Every note shows again.
test('the viewer maps `heading`: every note shows again, with its title', async ({ page }) => {
  test.setTimeout(300_000);
  const r = ordoAs('viewer', ['apply', viewerOnHeading(), '--direct', '--as', 'fed-e2e', '--wait-secs', '240']);
  expect(r.code, `${r.out}\n${r.err}`).toBe(0);
  expect(r.out).toContain('0.1.1');
  await page.goto(stack().viewerPage);
  await shows(page, 'Public notes', before);
  await expect(page.getByRole('alert')).toHaveCount(0);
});

// §7, test 6: a rollback of the viewer to the version that maps `title` is
// checked before it runs. The dry run says that version can no longer read
// `title`, and nothing moves.
test('a rollback to 0.1.0 is told it can no longer read `title`', async () => {
  const dry = dev.ordo(['rollback', '--version', '0.1.0', '--dry-run', ...door('viewer')]);
  // A refusal is said on standard error, with the exit code.
  const said = `${dry.out}${dry.err}`;
  expect(dry.code, said).not.toBe(0);
  expect(said).toContain('dry run    rollback to 0.1.0 (active: 0.1.1); nothing changed');
  expect(said).toMatch(/REFUSED +external_changed on `published`: column `title` \(it is now `heading`\)/);
  expect(said).toContain('0.1.0 can no longer read the tables above');
  const now = dev.ordo(['versions', 'list', ...door('viewer')]);
  expect(now.code, `${now.out}\n${now.err}`).toBe(0);
  expect(now.out).toMatch(/0\.1\.1.*active/);
});
