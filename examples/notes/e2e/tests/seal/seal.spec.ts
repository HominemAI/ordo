/**
 * Sealing a field on live data (plan-versions.md increment 13).
 *
 * The versions are notes v3 rewritten here (`v3As`). Ada, on v3, writes a
 * note from the page. 0.3.2 seals `notes.body`, which note-viewer reads
 * (the `reader` line): its plan is refused, naming the reader. 0.3.3 takes
 * `notes` off the reader line and is applied. 0.3.4 seals `notes.body`.
 * The developer runs the step from 0.3.3 through the door. The backfill
 * worker seals every note's body under its tenant's key, verifies each
 * tenant's count and hash and that every sealed value opens to its
 * plaintext, then drops the plaintext column: `--run` on the deployment's
 * database, `--tenant` on each tenant's, which pins that tenant to 0.3.4.
 * 0.3.4 is activated. The page reads Ada's note, and a note written after
 * the seal, and the table holds no plaintext.
 *
 * A regression looks like a seal under a reader that is not refused, a plan
 * with no sealed backfill, a refused run, a page that shows ciphertext or
 * nothing, or a body readable in the table.
 *
 * Not built here: live writes during the seal (increment 11a), and a
 * rollout that runs every tenant's step (11b). Nothing is written while a
 * seal runs here.
 *
 * It seals a field for the whole stack, so it runs after `rehearsal`, last.
 * Locally, `--project=seal --no-deps`. Chromium only.
 */

import { cpSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import { door, developer } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, ensureServing, noteInList, servingVersion, signIn, signedIn, stack, unique } from '../support.js';

test.describe.configure({ mode: 'serial' });

/**
 * Notes v3 at `version`, beside its `fn` code. `sealed` makes `notes.body`
 * sealed; `noReader` takes `notes` off the `reader note_viewer` line.
 */
function v3As(version: string, sealed: boolean, noReader: boolean): string {
  let src = readFileSync(join(stack().programV3, 'notes.ordo'), 'utf8');
  const swap = (from: string, to: string) => {
    expect(src, `notes v3 still holds ${from}`).toContain(from);
    src = src.replace(from, to);
  };
  swap('domain notes @ 0.3.0', `domain notes @ ${version}`);
  if (sealed) swap('  body:  text = ""\n', '  body:  text sealed\n');
  if (noReader) swap('reader note_viewer reads notes, notesAccess', 'reader note_viewer reads notesAccess');
  const dir = mkdtempSync(join(tmpdir(), `notes-${version}-`));
  writeFileSync(join(dir, 'notes.ordo'), src);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

/** SQL as the stack's own database user, in database `db`, answered as text. */
function sql(db: string, statement: string): string {
  const brew = '/opt/homebrew/opt/postgresql@17/bin/psql';
  const psql = process.env['ORDO_PSQL'] ?? (existsSync(brew) ? brew : 'psql');
  const s = stack();
  return execFileSync(psql, ['-h', s.pg.host, '-p', s.pg.port, '-U', s.pg.user, '-d', db, '-t', '-A', '-v', 'ON_ERROR_STOP=1', '-c', statement], {
    // CI's Postgres takes a password (ORDO_PG_PASSWORD); stack.sh sets
    // PGPASSWORD for itself, not for the tests.
    env: { ...process.env, PGPASSWORD: process.env['PGPASSWORD'] ?? process.env['ORDO_PG_PASSWORD'] ?? '' },
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
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

test('a field sealed on live data: the page reads it, and the table holds no plaintext', async ({ browser }, testInfo) => {
  testInfo.setTimeout(300_000);
  // v3 serves after `rehearsal`, which stores 0.3.3 and activates nothing. Run alone (`--no-deps`), v2 and v3 are
  // applied here.
  if (servingVersion() !== '0.3.0') {
    ensureServing(stack().programV2, '0.2.0');
    applyProgram(stack().programV3);
  }
  const ada = await owner('Ada');
  const dev = await developer(browser);
  try {
    const page = ada.page;
    await openV2(page);
    const title = unique('sealed-');
    const marker = unique('kept-under-key-');
    await addNote(page, title, `${marker} and more`);
    // Every person is a tenant with a database of their own (stack.sh).
    // Ada's is the one that holds her note.
    const dbs = sql(stack().pg.db, `SELECT DISTINCT db FROM "$spacedb"`).split('\n').filter(Boolean);
    const holds = (db: string) => {
      try {
        return sql(db, `SELECT count(*) FROM notes WHERE title = '${title}'`) === '1';
      } catch {
        return false; // a tenant database with no notes table yet
      }
    };
    const adaDb = dbs.find(holds) ?? '';
    expect(adaDb, `Ada's note is in one of ${dbs.length} tenant database(s)`).not.toBe('');
    expect(sql(adaDb, `SELECT count(*) FROM notes WHERE body = '${marker} and more'`), 'the body is plaintext before').toBe('1');

    await test.step('a seal of a field note-viewer reads is refused, naming it', () => {
      const dir = v3As('0.3.2', true, false);
      const stored = dev.ordo(['apply', dir, '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      const plan = dev.ordo(['evolve', '--from', '0.3.0', '--to', '0.3.2', ...door()]);
      expect(plan.code, plan.out).not.toBe(0);
      const said = `${plan.out}${plan.err}`;
      expect(said).toContain('seal_read_by_reader');
      expect(said).toContain('note_viewer');
      expect(said).toContain('7b');
    });

    await test.step('0.3.3 takes notes off the reader line', () => {
      applyProgram(v3As('0.3.3', false, true));
      expect(servingVersion()).toBe('0.3.3');
    });

    const dir = v3As('0.3.4', true, true);
    await test.step('0.3.4 is stored beside 0.3.3', () => {
      const stored = dev.ordo(['apply', dir, '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      expect(stored.out).toContain('0.3.4');
      expect(stored.out).toContain('activated  no');
    });

    await test.step('the plan seals notes.body, and the run seals every note', () => {
      const plan = dev.ordo(['evolve', '--from', '0.3.3', '--to', '0.3.4', ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      expect(plan.out).toContain('backfill: notes.body = seal');
      expect(plan.out).toContain('plaintext: no plaintext dual write');
      expect(plan.out).toMatch(/^prechecks +\d+ database\(s\) asked, 0 issue\(s\)$/m);
      const ran = dev.ordo(['evolve', '--from', '0.3.3', '--to', '0.3.4', '--run', ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toMatch(/^ran +/m);
    });

    await test.step("each tenant's database runs the stored step: Ada's notes are sealed", () => {
      // `--run` runs the step on the deployment's own database. A tenant
      // with a database of its own runs the stored steps with `--tenant`.
      // A rollout that does this for every tenant is increment 11b.
      const spaces = sql(stack().pg.db, `SELECT "$space" FROM "$spacedb" ORDER BY "$space"`).split('\n').filter(Boolean);
      const ada = sql(stack().pg.db, `SELECT "$space" FROM "$spacedb" WHERE db = '${adaDb}'`);
      expect(spaces).toContain(ada);
      for (const space of spaces) {
        const replayed = dev.ordo(['evolve', '--tenant', space, '--to', '0.3.4', ...door()]);
        if (space !== ada) continue;
        expect(replayed.code, `${replayed.out}\n${replayed.err}`).toBe(0);
        expect(replayed.out).toContain('notes.body = seal');
        expect(replayed.out).toMatch(/verified 1 row\(s\) in 1 space\(s\)/);
      }
    });

    await test.step('0.3.4 is activated', () => {
      const active = dev.ordo(['apply', dir, ...door()]);
      expect(active.code, `${active.out}\n${active.err}`).toBe(0);
      expect(active.out).toContain('activated  0.3.4');
      expect(servingVersion()).toBe('0.3.4');
    });

    await test.step('the table holds no plaintext', () => {
      expect(sql(adaDb, `SELECT data_type FROM information_schema.columns WHERE table_name = 'notes' AND column_name = 'body'`)).toBe('bytea');
      expect(sql(adaDb, `SELECT count(*) FROM information_schema.columns WHERE table_name = 'notes' AND column_name LIKE 'body$%'`)).toBe('0');
      expect(sql(adaDb, `SELECT count(*) FROM notes WHERE position(convert_to('${marker}', 'UTF8') in body) > 0`)).toBe('0');
    });

    await test.step('the page reads the sealed note, and one written after', async () => {
      await page.reload();
      await signedIn(page);
      await expect(noteInList(page, title)).toContainText(marker);
      await noteInList(page, title).click();
      await expect(page.getByRole('textbox', { name: 'Note', exact: true })).toHaveValue(`${marker} and more`);
      await page.getByRole('button', { name: 'Close', exact: true }).click();
      const later = unique('after-seal-');
      const laterBody = unique('also-sealed-');
      await addNote(page, later, laterBody);
      await page.reload();
      await signedIn(page);
      await expect(noteInList(page, later)).toContainText(laterBody);
      expect(sql(adaDb, `SELECT count(*) FROM notes WHERE position(convert_to('${laterBody}', 'UTF8') in body) > 0`)).toBe('0');
      await expect(page.getByRole('alert')).toHaveCount(0);
    });
  } finally {
    await dev.close();
    await ada.close();
  }
});
