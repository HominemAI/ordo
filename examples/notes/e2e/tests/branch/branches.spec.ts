/**
 * Dev branches are tenants on their own version (plan-versions.md
 * increment 5, §3.3). Done when: 2 branches are served side by side in dev.
 *
 * The developer, signed in with `ordo login`, applies 2 branches through
 * the door:
 *
 * - branch A is the program the stack serves now;
 * - branch B is the same program with one op added (`branchTitles`), at a
 *   version of its own, filled by a seed file on its first apply.
 *
 * Each branch is a tenant of its own (`branch-...`), with a database of its
 * own and the developer as its owner, pinned to its branch's version. A
 * deployment admin opens it as that owner, with a touch in the last 5
 * minutes, so the developer here has just signed in. The
 * deployment keeps serving its version: Ada's page works throughout.
 * Branch B answers `branchTitles` with its seeded note; branch A, on the
 * version before, does not have the op. A second name that maps to branch
 * A's tenant is refused naming `ordo branches delete`. Both branches are
 * listed, then deleted, which drops their databases.
 *
 * A regression looks like a branch's version served to the deployment, a
 * branch tenant on the wrong version, a collision applied, a seed not run,
 * or a deleted branch still listed.
 *
 * It applies programs to the stack, so it runs after `seal`, last. Locally,
 * `--project=branch --no-deps`. Chromium only.
 */

import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test } from '@playwright/test';
import { door, freshDeveloper, type Developer, type FreshDeveloper } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, ensureServing, noteInList, servingVersion, stack, unique, OFF_THE_GRANT } from '../support.js';

test.describe.configure({ mode: 'serial' });

/**
 * Notes v3 at `version`, as the stack serves it after `seal` when `sealed`
 * (body sealed, `notes` off the grant to note_viewer), beside its `fn` code. `extra`
 * adds the branch's own op.
 */
function program(version: string, sealed: boolean, extra: boolean): string {
  let src = readFileSync(join(stack().programV3, 'notes.ordo'), 'utf8');
  const swap = (from: string, to: string) => {
    expect(src, `notes v3 still holds ${from}`).toContain(from);
    src = src.replace(from, to);
  };
  swap('domain notes @ 0.3.0', `domain notes @ ${version}`);
  if (sealed) {
    swap('  body:  text = ""\n', '  body:  text sealed\n');
    for (const [from, to] of OFF_THE_GRANT) swap(from, to);
  }
  if (extra) src += '\n# Branch B only.\nop branchTitles unbounded =\n  notes[id, title]\n  sort -created\n';
  const dir = mkdtempSync(join(tmpdir(), `notes-branch-${version}-`));
  writeFileSync(join(dir, 'notes.ordo'), src);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

function ok(r: { code: number; out: string; err: string }): string {
  expect(r.code, `${r.out}\n${r.err}`).toBe(0);
  return r.out;
}

/** `ordo apply <dir> --branch <name>`: the branch's tenant and space. */
function applyBranch(dev: Developer, dir: string, branch: string, extra: string[] = []): { tenant: string; space: string; out: string } {
  const out = ok(dev.ordo(['apply', dir, '--branch', branch, ...extra, ...door()]));
  const m = /^branch +\S+ → tenant (\S+) \((\S+)\)$/m.exec(out);
  expect(m, out).not.toBeNull();
  return { tenant: m![1]!, space: m![2]!, out };
}

/** One op in a branch's tenant, as the developer: the status and the rows. */
async function call(dev: FreshDeveloper, space: string, op: string): Promise<{ status: number; rows: { title: string }[] }> {
  const r = await fetch(`${stack().door}/op/${op}`, {
    headers: { authorization: `Bearer ${dev.token()}`, 'x-ordo-space': space },
  });
  const body = (await r.json().catch(() => ({}))) as { rows?: { title: string }[] };
  return { status: r.status, rows: body.rows ?? [] };
}

test('2 branches are served side by side in dev, each on its own version', async ({ browser }, testInfo) => {
  testInfo.setTimeout(300_000);
  // The stack serves 0.3.4 after `seal`. Run alone, v2 then v3 are applied.
  let serving = servingVersion();
  if (serving !== '0.3.4' && serving !== '0.3.0') {
    ensureServing(stack().programV2, '0.2.1');
    applyProgram(stack().programV3);
    serving = '0.3.0';
  }
  const sealed = serving === '0.3.4';
  const ada = await owner(unique('Ada'));
  // Signed in now: a deployment admin reaches a branch's tenant with a
  // touch in the last 5 minutes.
  const dev = await freshDeveloper(browser, unique('Dev'));
  const a = `e2e/${unique('a')}`;
  const b = `e2e/${unique('b')}`;
  try {
    const seed = join(mkdtempSync(join(tmpdir(), 'notes-branch-seed-')), 'seed.jsonl');
    const seeded = unique('seeded-');
    writeFileSync(
      seed,
      `${JSON.stringify({ op: 'createNote', params: { title: seeded, body: 'from the seed', tags: [], created: new Date().toISOString().replace(/\.\d+Z$/, 'Z') } })}\n`,
    );

    const branchA = await test.step('branch A: the version the stack serves', () => {
      const r = applyBranch(dev, program(serving!, sealed, false), a);
      expect(r.out).toMatch(/^schema +untouched/m);
      expect(r.out).toMatch(new RegExp(`^pinned +${serving!.replace(/\./g, '\\.')} +\\(a new tenant`, 'm'));
      return r;
    });
    const versionB = sealed ? '0.5.1' : '0.5.0';
    const branchB = await test.step('branch B: one op more, its own version, seeded', () => {
      const r = applyBranch(dev, program(versionB, sealed, true), b, ['--seed', seed]);
      expect(r.out).toMatch(/^seeded +1 op\(s\)/m);
      expect(r.out).toMatch(new RegExp(`^pinned +${versionB.replace(/\./g, '\\.')} `, 'm'));
      return r;
    });
    expect(branchA.tenant).toMatch(/^branch-e2e-a/);
    expect(branchB.tenant).toMatch(/^branch-e2e-b/);
    expect(branchA.space).not.toBe(branchB.space);

    await test.step('the deployment still serves its version', () => {
      expect(servingVersion()).toBe(serving);
    });

    await test.step('branch B serves its own op with its seeded note', async () => {
      await expect
        .poll(async () => (await call(dev, branchB.space, 'branchTitles')).status, { timeout: 30_000, message: 'branch B serves branchTitles' })
        .toBe(200);
      const rows = (await call(dev, branchB.space, 'branchTitles')).rows;
      expect(rows.map((r) => r.title)).toContain(seeded);
    });

    await test.step('branch A, on the version before, has no such op, and serves its own', async () => {
      await expect
        .poll(async () => (await call(dev, branchA.space, 'myNotes')).status, { timeout: 30_000, message: 'branch A serves myNotes' })
        .toBe(200);
      expect((await call(dev, branchA.space, 'myNotes')).rows).toHaveLength(0);
      expect((await call(dev, branchA.space, 'branchTitles')).status).not.toBe(200);
    });

    await test.step("Ada's page works on the deployment's version", async () => {
      const title = unique('main-');
      await addNote(ada.page, title, 'Written beside 2 branches.');
      await expect(noteInList(ada.page, title)).toBeVisible();
    });

    await test.step("a name that maps to branch A's tenant is refused, naming the fix", () => {
      const r = dev.ordo(['apply', program(serving!, sealed, false), '--branch', a.toUpperCase(), ...door()]);
      expect(r.code).not.toBe(0);
      expect(`${r.out}${r.err}`).toContain(`ordo branches delete ${a} --reason`);
    });

    await test.step('both are listed, then deleted by command', () => {
      const list = ok(dev.ordo(['branches', 'list', ...door()]));
      expect(list).toContain(branchA.tenant);
      expect(list).toContain(branchB.tenant);
      for (const name of [a, b]) {
        const out = ok(dev.ordo(['branches', 'delete', name, '--reason', 'e2e done', ...door()]));
        expect(out).toMatch(/its database dropped/);
      }
      const after = ok(dev.ordo(['branches', 'list', ...door()]));
      expect(after).not.toContain(branchA.tenant);
      expect(after).not.toContain(branchB.tenant);
    });
  } finally {
    await dev.close();
    await ada.close();
  }
});
