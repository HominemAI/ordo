/**
 * Tenant builds (plan-versions.md increment 16, §3.8): a beta tenant serves
 * the branch while the rest serve main.
 *
 * Ada and Bob each have notes open in their own tenant. The developer, signed
 * in with `ordo login`, stores the branch (notes as served, plus an op
 * `betaNotes`) beside main with `--no-activate`, makes a build `beta` on it,
 * and puts Ada's tenant in it. Then:
 *
 * 1. Ada's tenant answers `betaNotes` with her notes; Bob's does not have it.
 *    Ada's page keeps working on the branch: a note added after lands.
 * 2. Main moves on (`ordo apply`, which activates a version with `mainNotes`).
 *    Bob's tenant serves it; Ada's stays on the branch. Moving her by hand
 *    (`ordo versions pin`) is refused, naming `ordo tenant-builds leave`.
 * 3. Ada leaves the build: her tenant serves main, and her notes are there.
 *
 * A regression looks like Ada moved back to main by the apply, Bob served the
 * branch, or a pin by hand that moves a build's tenant.
 *
 * Every version number rises, so it runs after `rehearsal`, last. Locally,
 * `--project=tenant-build --no-deps` applies v3 and runs alone. Chromium only.
 */

import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import { developer, door, type Developer } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, ensureServing, noteInList, servingVersion, stack, unique } from '../support.js';

test.describe.configure({ mode: 'serial' });

let dev: Developer | null = null;

test.afterAll(async () => {
  await dev?.close();
  dev = null;
});

function ok(r: { code: number; out: string; err: string }): string {
  expect(r.code, `${r.out}\n${r.err}`).toBe(0);
  return r.out;
}

/**
 * Notes as the stack serves it (`from`), renamed to `version`, with one more
 * read op named `op`. After `seal` and `rehearsal` that is 0.3.4: notes.body
 * sealed and notes off the reader line. Run alone, it is plain v3.
 */
function servedPlus(from: string, version: string, op: string): string {
  let src = readFileSync(join(stack().programV3, 'notes.ordo'), 'utf8');
  const swap = (a: string, b: string) => {
    expect(src, `notes v3 still holds ${a}`).toContain(a);
    src = src.replace(a, b);
  };
  swap('domain notes @ 0.3.0', `domain notes @ ${version}`);
  if (from === '0.3.4') {
    swap('  body:  text = ""\n', '  body:  text sealed\n');
    swap('reader note_viewer reads notes, notesAccess', 'reader note_viewer reads notesAccess');
  }
  src += `\n# Only this version has it (the tenant-build test).\nop ${op} unbounded =\n  notes[id, title]\n  sort -title\n`;
  const dir = mkdtempSync(join(tmpdir(), `notes-${version}-`));
  writeFileSync(join(dir, 'notes.ordo'), src);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

/** The token and tenant the page sends to its door, as last seen. */
function watchSession(page: Page): () => { token: string; space: string } {
  let seen = { token: '', space: '' };
  page.on('request', (r) => {
    const h = r.headers();
    const auth = h['authorization'] ?? '';
    const space = h['x-ordo-space'] ?? '';
    const toDoor = new URL(r.url()).port === new URL(stack().door).port;
    if (toDoor && auth.startsWith('Bearer ') && space && space !== 'system') {
      seen = { token: auth.slice('Bearer '.length), space };
    }
  });
  return () => {
    expect(seen.token, 'the page has called its door').not.toBe('');
    return seen;
  };
}

/** A read op in the person's own tenant: its status and the titles it listed. */
async function read(
  request: APIRequestContext,
  who: { token: string; space: string },
  name: string,
): Promise<{ status: number; titles: string[]; text: string }> {
  const r = await request.get(`${stack().door}/op/${name}`, {
    headers: { 'X-Ordo-Space': who.space, Authorization: `Bearer ${who.token}`, Origin: stack().page },
  });
  const text = await r.text();
  let titles: string[] = [];
  if (r.status() === 200) {
    titles = ((JSON.parse(text) as { rows: { title: string }[] }).rows ?? []).map((n) => n.title);
  }
  return { status: r.status(), titles, text };
}

test('a beta tenant serves the branch while the rest serve main', async ({ browser, request }, testInfo) => {
  testInfo.setTimeout(420_000);
  // After `rehearsal`, production serves 0.3.4. Run alone, v3 is applied here.
  if (!['0.3.0', '0.3.4'].includes(servingVersion() ?? '')) {
    ensureServing(stack().programV2, '0.2.0');
    applyProgram(stack().programV3);
  }
  const from = servingVersion() ?? '';
  const beta = servedPlus(from, '0.3.6', 'betaNotes');
  const main = servedPlus(from, '0.3.7', 'mainNotes');
  const d = await developer(browser);
  dev = d;

  const ada = await owner(unique('Ada'));
  const bob = await owner(unique('Bob'));
  try {
    const adaSession = watchSession(ada.page);
    const bobSession = watchSession(bob.page);
    const adaNote = unique('Ada beta ');
    const bobNote = unique('Bob main ');
    await addNote(ada.page, adaNote, 'Written before the build.');
    await addNote(bob.page, bobNote, 'Written on main.');
    const adaWho = () => adaSession();
    const bobWho = () => bobSession();
    const adaTenant = adaWho().space;

    await test.step('the branch is stored beside main, and Ada joins build beta', () => {
      expect(ok(d.ordo(['apply', beta, '--no-activate', ...door()]))).toContain('0.3.6');
      const set = ok(d.ordo(['tenant-builds', 'set', 'beta', '0.3.6', '--reason', 'public beta of betaNotes', ...door()]));
      expect(set).toMatch(/^build +beta serves #\d+ · 0\.3\.6$/m);
      const joined = ok(d.ordo(['tenant-builds', 'join', 'beta', adaTenant, ...door()]));
      expect(joined).toContain(`joined     ${adaTenant} to build beta on`);
      const list = ok(d.ordo(['tenant-builds', 'list', ...door()]));
      expect(list).toContain(`  tenant   ${adaTenant}`);
    });

    await test.step("Ada's tenant serves the branch; Bob's serves main", async () => {
      await expect
        .poll(async () => (await read(request, adaWho(), 'betaNotes')).status, { timeout: 60_000 })
        .toBe(200);
      expect((await read(request, adaWho(), 'betaNotes')).titles).toContain(adaNote);
      const bobBeta = await read(request, bobWho(), 'betaNotes');
      expect(bobBeta.status, bobBeta.text).not.toBe(200);
      // Her page keeps working on the branch.
      const after = unique('On the branch ');
      await addNote(ada.page, after, 'Written on the beta build.');
      await expect(noteInList(ada.page, after)).toBeVisible({ timeout: 30_000 });
      await expect(ada.page.getByRole('alert')).toHaveCount(0);
      expect((await read(request, adaWho(), 'betaNotes')).titles).toContain(after);
    });

    await test.step("main moves on: Bob's tenant follows, Ada's stays on the branch", async () => {
      expect(ok(d.ordo(['apply', main, ...door()]))).toContain('0.3.7');
      await expect
        .poll(async () => (await read(request, bobWho(), 'mainNotes')).status, { timeout: 60_000 })
        .toBe(200);
      expect((await read(request, bobWho(), 'mainNotes')).titles).toContain(bobNote);
      expect((await read(request, adaWho(), 'betaNotes')).status).toBe(200);
      const adaMain = await read(request, adaWho(), 'mainNotes');
      expect(adaMain.status, adaMain.text).not.toBe(200);
      const pinned = d.ordo(['versions', 'pin', adaTenant, '0.3.7', ...door()]);
      expect(pinned.code).not.toBe(0);
      expect(`${pinned.out}${pinned.err}`).toContain(`ordo tenant-builds leave ${adaTenant}`);
      const bobNew = unique('Bob after ');
      await addNote(bob.page, bobNew, 'Written on the new main.');
      await expect(noteInList(bob.page, bobNew)).toBeVisible({ timeout: 30_000 });
    });

    await test.step("Ada leaves the build: her tenant serves main, with her notes", async () => {
      const left = ok(d.ordo(['tenant-builds', 'leave', adaTenant, ...door()]));
      expect(left).toContain('0.3.7 (the active version)');
      await expect
        .poll(async () => (await read(request, adaWho(), 'mainNotes')).status, { timeout: 60_000 })
        .toBe(200);
      expect((await read(request, adaWho(), 'mainNotes')).titles).toContain(adaNote);
      ok(d.ordo(['tenant-builds', 'remove', 'beta', '--reason', 'the beta is over', ...door()]));
      expect(ok(d.ordo(['tenant-builds', 'list', ...door()]))).toContain('no tenant builds');
      await ada.page.reload();
      await expect(noteInList(ada.page, adaNote)).toBeVisible({ timeout: 30_000 });
      await expect(ada.page.getByRole('alert')).toHaveCount(0);
    });
  } finally {
    await ada.close();
    await bob.close();
  }
});
