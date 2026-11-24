/**
 * A staging clone of a named production tenant (plan-versions.md increment
 * 12, §3.4): it serves in staging on the version it serves in production,
 * with none of the person's own words.
 *
 * The stack runs 2 deployments at one identity service: production (the
 * notes door, handle `main`) and staging (stack.sh step 5b, handle
 * `staging`, registered there, no program). A new developer is invited by
 * the host command, joins with a passkey, is made a member of `staging`,
 * and signs the `ordo` command in to each. Every command below goes
 * through a door.
 *
 * The developer registers the chain on both deployments, records
 * production's door on staging and staging's clone key on production, and
 * runs `ordo clones clone` on staging. Staging holds no version, so the
 * tenant's comes down with the clone through the promotion gate, and the
 * clone gets a database of its own, brought to that version. Then the
 * developer reads the clone through staging's door.
 *
 * Built: a tenant with no notes. **Not yet: a tenant with notes.** Notes has
 * `acl`, and the import a clone lands through mints the caller's access row
 * for each note it creates, then refuses the bundle's own access row for the
 * same note as a duplicate (`expected_absent` on `notesAccess.note,
 * member`). That test is marked `fixme` until the import is fixed (row 12,
 * "Not built").
 *
 * A regression looks like a refused clone, a version that does not arrive,
 * a clone staging's door does not serve, or any of Ada's words in what it
 * serves.
 *
 * It registers `staging` in production's chain, and `ordo apply` goes to the
 * first step of a chain, so it runs last (playwright.config.ts) and takes
 * `staging` out of the chain again at the end. Chromium only.
 */

import { expect, test } from '@playwright/test';
import { developerOfTwo, type TwoDeployments } from '../developer.js';
import { owner } from '../people.js';
import { addNote, noteInList, stack, unique } from '../support.js';

test.describe.configure({ mode: 'serial' });

let dev: TwoDeployments | null = null;

function prod(): string[] {
  return ['--host', stack().door, '--space', 'system'];
}

function staging(): string[] {
  return ['--host', stack().staging, '--space', 'system'];
}

function ok(r: { code: number; out: string; err: string }): string {
  expect(r.code, `${r.out}\n${r.err}`).toBe(0);
  return r.out;
}

/** The developer, and the chain and clone key set up once for the file. */
async function developer(browser: import('@playwright/test').Browser): Promise<TwoDeployments> {
  if (dev) return dev;
  const d = await developerOfTwo(browser, unique('Dev'), 'staging');
  dev = d;
  await test.step('the chain: staging before production, staging knows where production is', () => {
    ok(d.main(['environments', 'register', 'staging', '--reason', 'staging clones', ...prod()]));
    ok(d.other(['environments', 'register', 'staging', '--reason', 'this is staging', ...staging()]));
    ok(d.other(['environments', 'set-own', 'staging', '--reason', 'this is staging', ...staging()]));
    ok(
      d.other([
        'environments', 'door', 'production', stack().door, 'system', '--handle', 'main',
        '--reason', 'clones come from here', ...staging(),
      ]),
    );
  });
  await test.step("production records staging's clone key", () => {
    const key = /^clone key +(\S+)$/m.exec(ok(d.other(['clones', 'key', ...staging()])))?.[1] ?? '';
    expect(key).not.toBe('');
    ok(d.main(['clones', 'set-key', 'staging', key, '--reason', 'staging clones', ...prod()]));
  });
  return d;
}

/** `ordo clones clone <name>` on staging: the clone's space and version. */
function cloneOf(d: TwoDeployments, name: string): { space: string; version: string; out: string } {
  const out = ok(d.other(['clones', 'clone', name, '--reason', 'QA on a real shape', ...staging()]));
  const space = /^cloned .* into \S+ \((\S+)\)$/m.exec(out)?.[1] ?? '';
  const version = /^version +(\S+)$/m.exec(out)?.[1] ?? '';
  expect(space, out).not.toBe('');
  expect(version, out).toMatch(/^\d+\.\d+\.\d+$/);
  return { space, version, out };
}

/** `myNotes` in the clone, through staging's door, as the developer. */
async function cloneNotes(d: TwoDeployments, space: string): Promise<{ body: string; rows: { title: string; body: string }[] }> {
  const token = d.otherToken();
  let body = '';
  await expect
    .poll(
      async () => {
        const r = await fetch(`${stack().staging}/op/myNotes`, {
          headers: { authorization: `Bearer ${token}`, 'x-ordo-space': space },
        });
        body = await r.text();
        return r.status;
      },
      { timeout: 30_000, message: "staging's door serves the clone" },
    )
    .toBe(200);
  return { body, rows: (JSON.parse(body) as { rows: { title: string; body: string }[] }).rows };
}

test.afterAll(async () => {
  if (!dev) return;
  // Production goes back to a chain of one, so a later apply on a reused
  // stack is not refused as a later step.
  dev.main(['environments', 'remove', 'staging', '--reason', 'the clone tests are done', ...prod()]);
  await dev.close();
  dev = null;
});

test("a clone of Ada's tenant serves in staging on her version; it came down with the clone", async ({
  browser,
}, testInfo) => {
  testInfo.setTimeout(300_000);
  // Her tenant is named after her, so the name is one no other test uses.
  const name = unique('Ada');
  const ada = await owner(name);
  try {
    const d = await developer(browser);
    const { space, version, out } = cloneOf(d, name);
    // Staging held no version: hers arrived through the promotion gate.
    expect(out).toMatch(/^arrived +\S+ came down with the clone/m);
    // Production serves her tenant on the same version.
    expect(ok(d.main(['versions', 'list', ...prod()]))).toContain(version);
    expect(ok(d.other(['clones', 'list', ...staging()]))).toContain(space);
    const served = await cloneNotes(d, space);
    expect(served.rows).toHaveLength(0);
    // Production is untouched: her page still works.
    const after = unique('after-');
    await addNote(ada.page, after, 'Written after the clone.');
    await expect(noteInList(ada.page, after)).toBeVisible();
  } finally {
    await ada.close();
  }
});

// Fixme: the import mints the caller's access row for each note, then
// refuses the bundle's access row for the same note (see the header).
test.fixme("a clone of Ada's notes serves them in staging, masked, with none of her words", async ({
  browser,
}, testInfo) => {
  testInfo.setTimeout(300_000);
  const name = unique('Ada');
  const ada = await owner(name);
  try {
    const words = [unique('Lunch with Charles '), unique('Doctor on Tuesday '), unique('Gift for Byron ')];
    for (const w of words) await addNote(ada.page, w, `${w}: private.`);
    const d = await developer(browser);
    const { space } = cloneOf(d, name);
    const served = await cloneNotes(d, space);
    expect(served.rows).toHaveLength(3);
    for (const r of served.rows) {
      expect(r.title).toMatch(/^masked-/);
      expect(r.body).toMatch(/^masked-/);
    }
    for (const w of words) expect(served.body).not.toContain(w.trim());
    for (const w of words) await expect(noteInList(ada.page, w)).toBeVisible();
  } finally {
    await ada.close();
  }
});
