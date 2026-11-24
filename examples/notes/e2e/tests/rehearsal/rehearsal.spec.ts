/**
 * The signed rehearsal (plan-versions.md increment 12b, §3.2): production
 * refuses a plan digest staging did not sign.
 *
 * The stack's 2 deployments, as in `clone`: production (the notes door,
 * handle `main`) and staging (handle `staging`). A new developer signs the
 * `ordo` command in to each. Every command goes through a door.
 *
 * The step starts from the version production serves. After `seal` that is
 * 0.3.4 (notes.body sealed, notes off the reader line), and the step goes to
 * 0.3.5, which adds an optional `mood`. Run alone (`--no-deps`), v3 is
 * applied and the step is 0.3.0 to 0.3.3, which takes notes off the reader
 * line. Version numbers only rise, so it runs after `seal`, last.
 *
 * 1. Production stores the new version, not activated, before staging
 *    joins its chain.
 * 2. Staging clones Ada's tenant, stores the version and the plan, and
 *    rehearses it on the clone: green, signed with a key staging made
 *    itself (Q11: nothing to set up).
 * 3. Production stores its own plan for the step, and running it on Ada's
 *    tenant (`ordo evolve --tenant`) is refused: staging's signature was
 *    not accepted here. The refusal names `ordo rehearsals rehearse` and
 *    `accept`.
 * 4. Break-glass (§3.2 item 6) is refused while staging answers. Then
 *    production's record of staging's door names a port nothing answers
 *    on, the developer breaks glass with a reason, `ordo rehearsals list`
 *    shows it active, and the step runs on Ada's tenant database (run
 *    alone, her pin then meets the promotion gate). The door is put back.
 * 5. Production accepts the rehearsal, read from staging as the developer:
 *    the signature checks and production renders the same digest.
 *
 * A regression looks like a step that runs unrefused, a rehearsal that is
 * not signed, an acceptance whose digest is not staging's, or a break-glass
 * granted while staging answers.
 *
 * It puts staging in production's chain and takes it out at the end.
 * Locally, `--project=rehearsal --no-deps`. Chromium only.
 */

import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test } from '@playwright/test';
import { developerOfTwo, type TwoDeployments } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, ensureServing, servingVersion, stack, unique } from '../support.js';

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

/** Done, or already so: a second run on a reused stack finds it set. */
function okOrAlready(r: { code: number; out: string; err: string }): void {
  if (r.code !== 0) expect(`${r.out}${r.err}`).toMatch(/already|unchanged/i);
}

/**
 * Notes v3 at `version`, beside its `fn` code, written as `seal`'s `v3As`
 * writes it: `sealed` seals notes.body, `noReader` takes notes off the
 * reader line. `mood` adds an optional field.
 */
function v3As(version: string, sealed: boolean, noReader: boolean, mood: boolean): string {
  let src = readFileSync(join(stack().programV3, 'notes.ordo'), 'utf8');
  const swap = (from: string, to: string) => {
    expect(src, `notes v3 still holds ${from}`).toContain(from);
    src = src.replace(from, to);
  };
  swap('domain notes @ 0.3.0', `domain notes @ ${version}`);
  if (sealed) swap('  body:  text = ""\n', '  body:  text sealed\n');
  if (noReader) swap('reader note_viewer reads notes, notesAccess', 'reader note_viewer reads notesAccess');
  if (mood) {
    const body = sealed ? '  body:  text sealed\n' : '  body:  text = ""\n';
    swap(body, `${body}  mood:  text?\n`);
  }
  const dir = mkdtempSync(join(tmpdir(), `notes-${version}-`));
  writeFileSync(join(dir, 'notes.ordo'), src);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

test.afterAll(async () => {
  if (!dev) return;
  // Production goes back to a chain of one, so a later apply on a reused
  // stack goes to it.
  dev.main(['environments', 'remove', 'staging', '--reason', 'the rehearsal test is done', ...prod()]);
  await dev.close();
  dev = null;
});

test('production refuses a plan digest staging did not sign, and accepts the one it did', async ({ browser }, testInfo) => {
  testInfo.setTimeout(300_000);
  // After `seal`, production serves 0.3.4. Run alone, v3 is applied here.
  if (!['0.3.0', '0.3.4'].includes(servingVersion() ?? '')) {
    ensureServing(stack().programV2, '0.2.0');
    applyProgram(stack().programV3);
  }
  const from = servingVersion() ?? '';
  const to = from === '0.3.4' ? '0.3.5' : '0.3.3';
  const dir = from === '0.3.4' ? v3As(to, true, true, true) : v3As(to, false, true, false);
  const d = await developerOfTwo(browser, unique('Dev'), 'staging');
  dev = d;

  await test.step(`production stores ${to}, not activated, while it is a chain of one`, () => {
    const out = ok(d.main(['apply', dir, '--no-activate', ...prod()]));
    expect(out).toContain(to);
  });

  await test.step('the chain: staging before production, each knows where the other answers', () => {
    okOrAlready(d.main(['environments', 'register', 'staging', '--reason', 'rehearsals', ...prod()]));
    okOrAlready(d.other(['environments', 'register', 'staging', '--reason', 'this is staging', ...staging()]));
    okOrAlready(d.other(['environments', 'set-own', 'staging', '--reason', 'this is staging', ...staging()]));
    ok(
      d.other([
        'environments', 'door', 'production', stack().door, 'system', '--handle', 'main',
        '--reason', 'clones come from here', ...staging(),
      ]),
    );
    ok(
      d.main([
        'environments', 'door', 'staging', stack().staging, 'system', '--handle', 'staging',
        '--reason', 'rehearsals are read from here', ...prod(),
      ]),
    );
    const key = /^clone key +(\S+)$/m.exec(ok(d.other(['clones', 'key', ...staging()])))?.[1] ?? '';
    expect(key).not.toBe('');
    ok(d.main(['clones', 'set-key', 'staging', key, '--reason', 'staging clones', ...prod()]));
  });

  const name = unique('Ada');
  const ada = await owner(name);
  let space = '';
  let tenant = '';
  try {
    await addNote(ada.page, unique('Rehearsed '), 'A note the rehearsal carries, masked.');
    await test.step(`staging clones Ada's tenant on ${from}`, () => {
      const args = ['clones', 'clone', name, '--reason', `rehearse ${to}`, ...staging()];
      let r = d.other(args);
      // notes v3 declares llmKey: staging gets its own test value first.
      const missing = /declares secrets this deployment has no value for: ([\w, ]+)\. Set each/.exec(`${r.out}${r.err}`);
      if (r.code !== 0 && missing) {
        for (const secret of missing[1]!.split(',').map((x) => x.trim())) {
          ok(d.other(['secret', 'set', secret, ...staging()], `staging-test-${secret}`));
        }
        r = d.other(args);
      }
      const out = ok(r);
      space = /^cloned .* into \S+ \((\S+)\)$/m.exec(out)?.[1] ?? '';
      expect(space, out).not.toBe('');
      // Her tenant's id in production, which the clone names.
      tenant = /^cloned +(\S+) from production into/m.exec(out)?.[1] ?? '';
      expect(tenant, out).not.toBe('');
      expect(out).toMatch(new RegExp(`^version +${from.replace(/\./g, '\\.')}$`, 'm'));
    });
  } finally {
    await ada.close();
  }

  let signed = '';
  await test.step(`staging stores ${to} and the plan, and rehearses it on the clone: green and signed`, () => {
    ok(d.other(['apply', dir, '--no-activate', ...staging()]));
    ok(d.other(['evolve', '--from', from, '--to', to, ...staging()]));
    const out = ok(
      d.other([
        'rehearsals', 'rehearse', '--from', from, '--to', to, '--clone', space,
        '--reason', 'the reader line on a real shape', ...staging(),
      ]),
    );
    expect(out).toContain(`rehearsed   ${from} to ${to} on 1 clone(s), green`);
    expect(out).toMatch(/^signed by +staging \(key [0-9a-f]{64}\)$/m);
    signed = /^digest +(\S+)$/m.exec(out)?.[1] ?? '';
    expect(signed).toMatch(/^sha256:/);
  });

  await test.step("production refuses to run the step on Ada's tenant: staging signed nothing it accepted", () => {
    // Production's own plan for the step. Its own database already records
    // the new version (the apply above made its additive changes), so the
    // step that would run is on Ada's tenant database, still on `from`.
    ok(d.main(['evolve', '--from', from, '--to', to, ...prod()]));
    const r = d.main(['evolve', '--tenant', tenant, '--to', to, ...prod()]);
    expect(r.code, r.out).not.toBe(0);
    const said = `${r.out}${r.err}`;
    expect(said).toContain('plan_not_rehearsed');
    expect(said).toContain('ordo rehearsals rehearse');
    expect(said).toContain('ordo rehearsals accept');
  });

  await test.step('break-glass is refused while staging answers', () => {
    const r = d.main(['rehearsals', 'break-glass', '--from', from, '--to', to, '--reason', 'staging is slow', ...prod()]);
    expect(r.code, r.out).not.toBe(0);
    const said = `${r.out}${r.err}`;
    expect(said).toContain('break_glass_staging_answers');
    expect(said).toContain('ordo rehearsals accept');
  });

  await test.step("staging is down: the developer breaks glass with a reason, and the step runs on Ada's tenant", () => {
    // Production's record of staging's door now names a port nothing
    // answers on: staging is down, as production sees it.
    ok(
      d.main([
        'environments', 'door', 'staging', 'http://127.0.0.1:9', 'system', '--handle', 'staging',
        '--reason', 'staging is down', ...prod(),
      ]),
    );
    const reason = 'staging cluster down; ship the mood field';
    const out = ok(d.main(['rehearsals', 'break-glass', '--from', from, '--to', to, '--reason', reason, ...prod()]));
    expect(out).toContain(`broke glass ${from} to ${to}: staging is down`);
    expect(out).toContain(`reason      ${reason}`);
    const list = ok(d.main(['rehearsals', 'list', ...prod()]));
    expect(list).toMatch(new RegExp(`^BREAK-GLASS\\n  ${from.replace(/\./g, '\\.')} to ${to.replace(/\./g, '\\.')} .*\\(active\\)  reason: ${reason}$`, 'm'));
    // The rehearsal gate lets the step run: her tenant's database is
    // replayed. After `seal` she is pinned too; run alone, production
    // refuses the pin by its other gate (the version was applied to
    // production, not promoted), after the replay.
    const run = d.main(['evolve', '--tenant', tenant, '--to', to, ...prod()]);
    const ran = `${run.out}${run.err}`;
    expect(ran).not.toContain('plan_not_rehearsed');
    expect(ran).toMatch(new RegExp(`replayed +${from.replace(/\./g, '\\.')} to |the tenant's database was replayed`));
    // Staging is back: production records its door again.
    ok(
      d.main([
        'environments', 'door', 'staging', stack().staging, 'system', '--handle', 'staging',
        '--reason', 'staging is back', ...prod(),
      ]),
    );
  });

  await test.step("production accepts staging's rehearsal: read as the developer, the same digest", () => {
    const out = ok(d.main(['rehearsals', 'accept', '--from', from, '--to', to, ...prod()]));
    expect(out).toContain(`accepted    ${from} to ${to}, rehearsed on staging`);
    expect(out).toContain(`digest      ${signed}`);
    const list = ok(d.main(['rehearsals', 'list', ...prod()]));
    expect(list).toContain(`${from} to ${to}  ${signed}  from staging`);
  });
});
