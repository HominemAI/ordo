/**
 * The signed rehearsal (plan-versions.md increment 12b, §3.2): production
 * refuses a plan digest staging did not sign.
 *
 * The stack's 2 deployments, as in `clone`: production (the notes door,
 * handle `main`) and staging (handle `staging`). A new developer signs the
 * `ordo` command in to each. Every command goes through a door.
 *
 * The step is notes v3 (0.3.0) to 0.3.3, which takes `notes` off the
 * `reader note_viewer` line: the same 0.3.3 `seal` applies next, written
 * the same way, so `seal` finds it stored and its numbers still rise.
 *
 * 1. Production stores 0.3.3, not activated, before staging joins its chain.
 * 2. Staging clones Ada's tenant (on 0.3.0), stores 0.3.3 and the plan,
 *    and rehearses it on the clone: green, signed with a key staging made
 *    itself (Q11: nothing to set up).
 * 3. Production stores its own plan for the step and `--run` is refused:
 *    staging's signature was not accepted here. The refusal names
 *    `ordo rehearsals rehearse` and `accept`.
 * 4. Production accepts the rehearsal, read from staging as the developer:
 *    the signature checks and production renders the same digest.
 *
 * A regression looks like a `--run` that is not refused, a rehearsal that
 * is not signed, or an acceptance whose digest is not staging's.
 *
 * It puts staging in production's chain and takes it out at the end, so it
 * runs after `clone` and before `seal`. Locally,
 * `--project=rehearsal --no-deps`. Chromium only.
 */

import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test } from '@playwright/test';
import { developerOfTwo, type TwoDeployments } from '../developer.js';
import { owner } from '../people.js';
import { addNote, stack, unique } from '../support.js';

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
 * Notes v3 at 0.3.3 with `notes` off the reader line, beside its `fn` code:
 * byte for byte what `seal`'s `v3As('0.3.3', false, true)` writes.
 */
function v3At033(): string {
  let src = readFileSync(join(stack().programV3, 'notes.ordo'), 'utf8');
  const swap = (from: string, to: string) => {
    expect(src, `notes v3 still holds ${from}`).toContain(from);
    src = src.replace(from, to);
  };
  swap('domain notes @ 0.3.0', 'domain notes @ 0.3.3');
  swap('reader note_viewer reads notes, notesAccess', 'reader note_viewer reads notesAccess');
  const dir = mkdtempSync(join(tmpdir(), 'notes-0.3.3-'));
  writeFileSync(join(dir, 'notes.ordo'), src);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

test.afterAll(async () => {
  if (!dev) return;
  // Production goes back to a chain of one, so `seal` applies to it.
  dev.main(['environments', 'remove', 'staging', '--reason', 'the rehearsal test is done', ...prod()]);
  await dev.close();
  dev = null;
});

test('production refuses a plan digest staging did not sign, and accepts the one it did', async ({ browser }, testInfo) => {
  testInfo.setTimeout(300_000);
  const dir = v3At033();
  const d = await developerOfTwo(browser, unique('Dev'), 'staging');
  dev = d;

  await test.step('production stores 0.3.3, not activated, while it is a chain of one', () => {
    const out = ok(d.main(['apply', dir, '--no-activate', ...prod()]));
    expect(out).toContain('0.3.3');
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
  try {
    await addNote(ada.page, unique('Rehearsed '), 'A note the rehearsal carries, masked.');
    await test.step("staging clones Ada's tenant on 0.3.0", () => {
      const args = ['clones', 'clone', name, '--reason', 'rehearse 0.3.3', ...staging()];
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
      expect(out).toMatch(/^version +0\.3\.0$/m);
    });
  } finally {
    await ada.close();
  }

  let signed = '';
  await test.step('staging stores 0.3.3 and the plan, and rehearses it on the clone: green and signed', () => {
    ok(d.other(['apply', dir, '--no-activate', ...staging()]));
    ok(d.other(['evolve', '--from', '0.3.0', '--to', '0.3.3', ...staging()]));
    const out = ok(
      d.other([
        'rehearsals', 'rehearse', '--from', '0.3.0', '--to', '0.3.3', '--tenant', space,
        '--reason', 'the reader line on a real shape', ...staging(),
      ]),
    );
    expect(out).toMatch(/^rehearsed +0\.3\.0 to 0\.3\.3 on 1 clone\(s\), green$/m);
    expect(out).toMatch(/^signed by +staging \(key [0-9a-f]{64}\)$/m);
    signed = /^digest +(\S+)$/m.exec(out)?.[1] ?? '';
    expect(signed).toMatch(/^sha256:/);
  });

  await test.step('production refuses to run the step: staging signed nothing it accepted', () => {
    const r = d.main(['evolve', '--from', '0.3.0', '--to', '0.3.3', '--run', ...prod()]);
    expect(r.code, r.out).not.toBe(0);
    const said = `${r.out}${r.err}`;
    expect(said).toContain('plan_not_rehearsed');
    expect(said).toContain('ordo rehearsals rehearse');
    expect(said).toContain('ordo rehearsals accept');
  });

  await test.step("production accepts staging's rehearsal: read as the developer, the same digest", () => {
    const out = ok(d.main(['rehearsals', 'accept', '--from', '0.3.0', '--to', '0.3.3', ...prod()]));
    expect(out).toMatch(/^accepted +0\.3\.0 to 0\.3\.3, rehearsed on staging$/m);
    expect(out).toContain(`digest      ${signed}`);
    const list = ok(d.main(['rehearsals', 'list', ...prod()]));
    expect(list).toContain(`0.3.0 to 0.3.3  ${signed}  from staging`);
  });
});
