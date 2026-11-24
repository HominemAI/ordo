/**
 * Restore across versions (plan-versions.md increment 17, §2.9).
 *
 * Ada's tenant has a database of its own. It starts on notes v2 (0.2.0)
 * while the stack serves v3 (0.3.0), and she writes 3 notes. A snapshot of
 * her database is taken. Her tenant moves to v3, and she writes a 4th
 * note. Then her database is restored from the snapshot, and
 * `ordo restore --record --tenant` reads the version the restored data is
 * at: her pin follows it to 0.2.0, cause `restore`. Her page shows the 3
 * notes and not the 4th, under v2 (v3's `starredNotes` is not hers).
 * Then she moves again, to v3, and `starredNotes` answers.
 *
 * The snapshot and the restore are whole-database copies on the stack's
 * own Postgres (`CREATE DATABASE ... TEMPLATE`), the shape a point-in-time
 * restore leaves: no row is written through the door.
 *
 * A regression looks like a restore record that leaves her pin on v3 (her
 * page then refused, the database behind it), notes from after the
 * snapshot, or a tenant that does not move again.
 *
 * It needs v3 active, so it runs after `v3` and before `evolve-checks`.
 * Locally, `--project=restore --no-deps`. Chromium only.
 */

import { execFileSync } from 'node:child_process';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import { developer, door, type Developer } from '../developer.js';
import { owner } from '../people.js';
import {
  addNote,
  applyProgram,
  ensureServing,
  noteInList,
  ordoEnv,
  signIn,
  signOut,
  servingVersion,
  signedIn,
  stack,
  unique,
} from '../support.js';

test.describe.configure({ mode: 'serial' });

async function openV2(page: Page): Promise<void> {
  await page.goto(stack().pageV2);
  const notes = page.getByLabel('New note', { exact: true });
  const settingUp = page.getByRole('heading', { name: 'Setting up your account' });
  const passkey = page.getByRole('button', { name: 'Sign in with a passkey' });
  await expect(notes.or(settingUp).or(passkey)).toBeVisible({ timeout: 30_000 });
  if (await passkey.isVisible()) await signIn(page);
  await signedIn(page);
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

/** One read op in her tenant: its status and rows. */
async function read(
  request: APIRequestContext,
  who: { token: string; space: string },
  name: string,
): Promise<{ status: number; rows: { title?: string }[]; text: string }> {
  const r = await request.fetch(`${stack().door}/op/${name}`, {
    headers: { 'X-Ordo-Space': who.space, Authorization: `Bearer ${who.token}`, Origin: stack().page },
  });
  const text = await r.text();
  let rows: { title?: string }[] = [];
  if (r.status() === 200) rows = (JSON.parse(text) as { rows: { title?: string }[] }).rows;
  return { status: r.status(), rows, text };
}

/** `psql -c` against the stack's Postgres, as the stack's own user. */
function psql(db: string, sql: string): string {
  const s = stack();
  const bin = process.env['ORDO_PSQL'] ?? 'psql';
  return execFileSync(bin, ['-h', s.pg.host, '-p', s.pg.port, '-U', s.pg.user, '-d', db, '-qAtc', sql], {
    encoding: 'utf8',
    env: { ...process.env, PGPASSWORD: process.env['ORDO_PG_PASSWORD'] ?? process.env['PGPASSWORD'] ?? '' },
  }).trim();
}

/**
 * `target` becomes a copy of `source`. Nobody may be connected to either
 * while Postgres copies, so connections are closed off and ended first; the
 * door reconnects once they are allowed again.
 */
function copyDatabase(source: string, target: string, replace: boolean): void {
  const close = (db: string) => {
    psql('postgres', `ALTER DATABASE "${db}" WITH ALLOW_CONNECTIONS false`);
    psql('postgres', `SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '${db}'`);
  };
  close(source);
  try {
    if (replace) psql('postgres', `DROP DATABASE IF EXISTS "${target}" WITH (FORCE)`);
    psql('postgres', `CREATE DATABASE "${target}" TEMPLATE "${source}"`);
  } finally {
    psql('postgres', `ALTER DATABASE "${source}" WITH ALLOW_CONNECTIONS true`);
  }
}

/** Wait until `name` answers 200 in her tenant; on time out, say what it answered. */
async function answers(
  request: APIRequestContext,
  who: { token: string; space: string },
  name: string,
  message: string,
): Promise<void> {
  let last = '';
  await expect
    .poll(
      async () => {
        const r = await read(request, who, name);
        last = `${r.status} ${r.text}`;
        return r.status;
      },
      { timeout: 90_000, message: `${message} (last answer: ${last})` },
    )
    .toBe(200)
    .catch((e: unknown) => {
      throw new Error(`${message}: \`${name}\` last answered ${last}`, { cause: e });
    });
}

function ok(r: { code: number; out: string; err: string }): string {
  expect(r.code, `${r.out}\n${r.err}`).toBe(0);
  return r.out;
}

let dev: Developer | null = null;

test.afterAll(async () => {
  if (!dev) return;
  // New tenants start on the active version again, whatever happened above.
  dev.ordo(['versions', 'set', 'new-tenant', '--clear', '--reason', 'the restore test is done', ...door()]);
  await dev.close();
  dev = null;
});

test('a snapshot taken on v2 restores beside tenants on v3, serves v2 with its notes, and moves again', async ({
  browser,
  request,
}, testInfo) => {
  testInfo.setTimeout(420_000);
  // v3 serves after the `v3` project. Run alone (`--no-deps`), v2 then v3
  // are applied here, so both are stored.
  if (servingVersion() !== '0.3.0') {
    ensureServing(stack().programV2, '0.2.0');
    applyProgram(stack().programV3);
  }
  const d = await developer(browser);
  dev = d;

  // Her tenant starts on v2 while the stack serves v3.
  ok(d.ordo(['versions', 'set', 'new-tenant', '0.2.0', '--reason', 'a tenant on v2 to snapshot', ...door()]));
  const ada = await owner(unique('Ada'));
  try {
    ok(d.ordo(['versions', 'set', 'new-tenant', '--clear', '--reason', 'Ada has her tenant', ...door()]));
    const page = ada.page;
    const session = watchSession(page);
    await openV2(page);
    const before = [unique('before-a-'), unique('before-b-'), unique('before-c-')];
    for (const t of before) await addNote(page, t, 'Written on v2.');
    const who = session();
    expect((await read(request, who, 'starredNotes')).status, 'v2 has no starredNotes').not.toBe(200);

    const db = psql(stack().pg.db, `SELECT "db" FROM "$spacedb" WHERE "$space" = '${who.space}'`);
    expect(db, 'her tenant has a database of its own').not.toBe('');
    const snapshot = `${db}_snap`.slice(0, 60);

    await test.step('a snapshot of her database, taken on v2', () => {
      copyDatabase(db, snapshot, true);
    });
    const takenAt = Math.floor(Date.now() / 1000);

    const after = unique('after-move-');
    await test.step('she moves to v3 and writes a 4th note', async () => {
      ok(d.ordo(['versions', 'pin', who.space, '0.3.0', ...door()]));
      await answers(request, who, 'starredNotes', 'her tenant serves v3');
      await page.reload();
      await signedIn(page);
      for (const t of before) await expect(noteInList(page, t)).toBeVisible();
      await addNote(page, after, 'Written on v3, after the snapshot.');
    });

    await test.step('her database is restored from the snapshot; the record pins her to v2', () => {
      copyDatabase(snapshot, db, true);
      const out = execFileSync(
        stack().ordo,
        [
          'restore', '--record', '--direct', '--tenant', '--space', who.space,
          '--as', 'notes-e2e', '--source', 'pitr', '--at', String(takenAt),
        ],
        { env: ordoEnv(), encoding: 'utf8' },
      );
      console.log(out);
      expect(out).toContain(`database `);
      expect(out).toContain('version PINNED 0.3.0 -> 0.2.0 · cause restore');
    });

    await test.step('her page shows the 3 notes from the snapshot, under v2', async () => {
      await answers(request, who, 'myNotes', 'her tenant serves again');
      const served = await read(request, who, 'myNotes');
      const titles = served.rows.map((r) => r.title);
      for (const t of before) expect(titles).toContain(t);
      expect(titles).not.toContain(after);
      expect((await read(request, who, 'starredNotes')).status, 'v2 has no starredNotes').not.toBe(200);
      // A fresh sign-in on the page: this device's copy is dropped at sign-out.
      await signOut(page);
      await openV2(page);
      for (const t of before) await expect(noteInList(page, t)).toBeVisible({ timeout: 30_000 });
      await expect(noteInList(page, after)).toHaveCount(0);
    });

    await test.step('she moves again, to v3', async () => {
      ok(d.ordo(['versions', 'pin', who.space, '0.3.0', ...door()]));
      // Closed for a moment while the catch-up brings her database to v3.
      await answers(request, session(), 'myNotes', 'her tenant serves after the move');
      await page.reload();
      await signedIn(page);
      for (const t of before) await expect(noteInList(page, t)).toBeVisible();
      // Her session since the sign-in above.
      await answers(request, session(), 'starredNotes', 'her tenant serves v3 again');
    });
    psql('postgres', `DROP DATABASE IF EXISTS "${snapshot}" WITH (FORCE)`);
  } finally {
    await ada.close();
  }
});
