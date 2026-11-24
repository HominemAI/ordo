/**
 * A rename on one tenant under live writes, through the door, with the page
 * open (plan-versions.md increment 11a; plan-backfill-dual-writes.md §9).
 *
 * 0.3.2 is notes v3 with `notes.body` renamed to `text` with `prev(body)`.
 * Its ops still answer `body`, so the v2 page reads the same notes. The
 * developer, signed in with `ordo login` (tests/developer.ts), stores it and
 * its step, then moves Ada's tenant onto it:
 *
 * 1. `ordo evolve --tenant <Ada> --budget 1 --chunk-rows 2`: the expand and
 *    one chunk of the copy. The move stops in `backfilling`, Ada still on
 *    0.3.0, and every write she makes keeps both columns.
 * 2. Ada types into a note and adds one, on the page.
 * 3. `ordo evolve --tenant <Ada>`: the rest of the copy, verify, the switch.
 *    Ada is on 0.3.2 in a 7-day soak. She types into another note.
 * 4. `ordo evolve --tenant <Ada> --revert`: back on 0.3.0 inside the soak.
 *    After a reload, every note holds what she typed in steps 2 and 3.
 * 5. The move switches again and `--close-soak` ends it: `notes.body` is
 *    orphaned, and the note still holds what she typed.
 *
 * A regression looks like a typed text missing after a reload, a refused
 * step, a move that does not stop at its budget, or a revert that loses
 * what was typed on 0.3.2.
 *
 * It applies v2 and v3 and moves one tenant, so it runs after `orphans`,
 * last. Chromium only.
 */

import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import { door, developer } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, noteInList, signIn, signedIn, stack, unique } from '../support.js';

test.describe.configure({ mode: 'serial' });

/**
 * The 0.3.2 program: notes v3 with `body` renamed to `text`, made from
 * notes v3's own file so the two never drift, beside v3's `fn` code.
 */
function renamedBody(): string {
  const v3 = readFileSync(join(stack().programV3, 'notes.ordo'), 'utf8');
  const swaps: [string, string][] = [
    ['domain notes @ 0.3.0', 'domain notes @ 0.3.2'],
    ['  body:  text = ""', '  text:  text = "" prev(body)'],
    ['set notes { title: $title, body: $body,', 'set notes { title: $title, text: $body,'],
    ['set notes { id: $id, rev: $rev, title: $title, body: $body,', 'set notes { id: $id, rev: $rev, title: $title, text: $body,'],
    ['[id, rev, title, body, tags, created, pinned, archived]', '[id, rev, title, body: text, tags, created, pinned, archived]'],
    ['[id, rev, title, body, tags, created]', '[id, rev, title, body: text, tags, created]'],
    ['[id, rev, title, body]', '[id, rev, title, body: text]'],
  ];
  let source = v3;
  for (const [from, to] of swaps) {
    expect(source, `notes v3 still holds \`${from}\``).toContain(from);
    source = source.split(from).join(to);
  }
  expect(source).not.toMatch(/\bbody: \$body\b/);
  const dir = mkdtempSync(join(tmpdir(), 'notes-v3-2-'));
  writeFileSync(join(dir, 'notes.ordo'), source);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

async function openV2(page: Page): Promise<void> {
  await page.goto(stack().pageV2);
  const notes = page.getByLabel('New note', { exact: true });
  const settingUp = page.getByRole('heading', { name: 'Setting up your account' });
  const passkey = page.getByRole('button', { name: 'Sign in with a passkey' });
  await expect(notes.or(settingUp).or(passkey)).toBeVisible({ timeout: 30_000 });
  if (await passkey.isVisible()) await signIn(page);
  await signedIn(page);
}

/** The tenant the page sends to its door, as last seen. */
function watchTenant(page: Page): () => string {
  let seen = '';
  page.on('request', (r) => {
    const space = r.headers()['x-ordo-space'] ?? '';
    const toDoor = new URL(r.url()).port === new URL(stack().door).port;
    if (toDoor && space && space !== 'system') seen = space;
  });
  return () => {
    expect(seen, 'the page has called its door').not.toBe('');
    return seen;
  };
}

/** Open a note, type its text, save. A saved note closes its editor. */
async function typeInto(page: Page, title: string, text: string): Promise<void> {
  await noteInList(page, title).click();
  await page.getByLabel('Note', { exact: true }).fill(text);
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByLabel('Note', { exact: true })).toHaveCount(0);
}

/** After a reload, the note holds `text`. */
async function holds(page: Page, title: string, text: string): Promise<void> {
  await noteInList(page, title).click();
  await expect(page.getByLabel('Note', { exact: true })).toHaveValue(text);
  await page.getByRole('button', { name: 'Close', exact: true }).click();
}

test('a rename on one tenant under live writes: nothing typed is lost, and a revert in the soak keeps it', async ({
  browser,
}, testInfo) => {
  testInfo.setTimeout(420_000);
  // Run alone (`--project=moves --no-deps`), v3 is not applied yet.
  applyProgram(stack().programV2);
  applyProgram(stack().programV3);
  const ada = await owner('Ada');
  const dev = await developer(browser);
  try {
    const page = ada.page;
    const tenant = watchTenant(page);
    await openV2(page);
    const titles = [0, 1, 2, 3, 4, 5].map((i) => unique(`moved-${i}-`));
    for (const t of titles) await addNote(page, t, `Written before the move: ${t}.`);
    const space = tenant();

    await test.step('0.3.2 is stored beside 0.3.0, and its step with the rename', () => {
      const stored = dev.ordo(['apply', renamedBody(), '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      expect(stored.out).toContain('0.3.2');
      const plan = dev.ordo(['evolve', '--from', '0.3.0', '--to', '0.3.2', ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      expect(plan.out).toContain('B4');
      expect(plan.out).toMatch(/notes\.body.*notes\.text/);
    });

    await test.step('the move starts and stops at its budget, Ada still on 0.3.0', () => {
      const ran = dev.ordo(['evolve', '--tenant', space, '--to', '0.3.2', '--budget', '1', '--chunk-rows', '2', ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain('move       0.3.0 to 0.3.2: backfilling');
      expect(ran.out).toContain('every write keeps both shapes');
    });

    const during = 'Typed while the backfill ran.';
    const added = unique('added-during-');
    await test.step('Ada types into a note and adds one during the backfill', async () => {
      await typeInto(page, titles[5]!, during);
      await addNote(page, added, 'Added while the backfill ran.');
    });

    await test.step('the move finishes: verify, the switch, a 7-day soak', () => {
      const ran = dev.ordo(['evolve', '--tenant', space, '--to', '0.3.2', ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain('pinned     0.3.2 (was 0.3.0)');
      expect(ran.out).toContain('move       0.3.0 to 0.3.2: soaking');
      expect(ran.out).toMatch(/^soak +until /m);
    });

    const soaked = 'Typed on 0.3.2, in the soak.';
    await test.step('on 0.3.2, Ada reads every note and types into another', async () => {
      await page.reload();
      await signedIn(page);
      for (const t of titles) await expect(noteInList(page, t)).toBeVisible();
      await holds(page, titles[5]!, during);
      await typeInto(page, titles[0]!, soaked);
    });

    await test.step('reverted inside the soak: back on 0.3.0', () => {
      const back = dev.ordo(['evolve', '--tenant', space, '--revert', ...door()]);
      expect(back.code, `${back.out}\n${back.err}`).toBe(0);
      expect(back.out).toContain('pinned     0.3.0 (was 0.3.2)');
      expect(back.out).toContain('gated');
    });

    await test.step('after a reload on 0.3.0, every note holds what Ada typed', async () => {
      await page.reload();
      await signedIn(page);
      await expect(noteInList(page, added)).toBeVisible();
      await holds(page, titles[5]!, during);
      await holds(page, titles[0]!, soaked);
      await expect(page.getByRole('alert')).toHaveCount(0);
    });

    await test.step('switched again and the soak closed: body is orphaned, the text kept', async () => {
      const again = dev.ordo(['evolve', '--tenant', space, '--to', '0.3.2', ...door()]);
      expect(again.code, `${again.out}\n${again.err}`).toBe(0);
      expect(again.out).toContain('soaking');
      const closed = dev.ordo(['evolve', '--tenant', space, '--close-soak', ...door()]);
      expect(closed.code, `${closed.out}\n${closed.err}`).toBe(0);
      expect(closed.out).toContain('closed     the soak is over; orphaned: notes.body');
      await page.reload();
      await signedIn(page);
      await holds(page, titles[0]!, soaked);
      await holds(page, titles[5]!, during);
    });
  } finally {
    await dev.close();
    await ada.close();
  }
});
