/**
 * A rename on 3 tenants under live writes, moved by a rollout, and a
 * rollback in the soak (plan-versions.md increment 11b; §2.5;
 * plan-backfill-dual-writes.md step B5).
 *
 * Ann, Ben and Cat each sign up, so each has a tenant with a database of
 * its own, and write notes on the v2 page. The new version renames
 * `notes.title` to `heading` with `prev(title)`, its ops still answering
 * `title`. After `moves`, production serves 0.3.4 and the new version is
 * 0.3.7; run alone, v3 is applied and it is 0.3.3. The developer, signed
 * in with `ordo login`:
 *
 * 1. Stores it and its step, and runs the step on the deployment's own
 *    database (`ordo evolve --run`), so a rollout may start.
 * 2. Sets `backfill.chunk_rows` 1 and `backfill.budget` 1, so each
 *    tenant's copy takes one row per turn of the rollout, and
 *    `backfill.concurrency` 3 with `backfill.per_database` 1, so the 3
 *    moves of a turn run at once (part 2); 0 is refused, naming it.
 * 3. `ordo rollout start <to> --strategy group --group <the 3>`, a turn
 *    a second: the rollout moves each through its stored step, a move
 *    with dual writes. The rest wait behind the group's day-long gate.
 *    While `ordo rollout status` says they are backfilling, each person
 *    retitles a note and adds one.
 * 4. The 3 move and soak. After a reload each reads what they typed, and
 *    retitles another note on the new version.
 * 5. `ordo rollout abort`, then `ordo rollback --version <from> --dry-run`
 *    names each as kept, and `ordo rollback --version <from>` puts the 3
 *    back. After a reload, every title each person typed is there.
 * 6. Ann shared a note with Ben before the rollout. Her move killed that
 *    share, and it stays closed after the rollback: Ben's page says so and
 *    holds none of it. Ann shares again and Ben reads it (11b part 3).
 * 7. The new version also adds `check heading != "<a title>"`. In the soak
 *    the check is not in the tenants' databases: the host holds it, and
 *    Cat's save of that title is refused by name, the note unchanged
 *    (11b part 4).
 *
 * A regression looks like a typed title missing after a reload, a tenant
 * the rollout moves without a copy (never `backfilling`), a failed tenant,
 * or a rollback that leaves a tenant on the new version.
 *
 * It stores 0.3.7, above `moves`' 0.3.6 and below `branch`'s 0.5.x, so it
 * runs between them. Chromium only.
 */

import { expect, test } from '@playwright/test';
import { door, developer } from '../developer.js';
import { acceptInvitation, invitationFor, owner, sharedList } from '../people.js';
import { addNote, applyProgram, ensureServing, noteInList, servingVersion, signedIn, stack, unique } from '../support.js';
import { holds, openV2, renamedTitle, retitle, watchTenant } from './moves.js';

test.describe.configure({ mode: 'serial' });

/** A title the new version's check refuses (11b part 4). */
const REFUSED = 'A title the new version refuses';

test('a rollout moves 3 tenants through a rename under live writes, and a rollback in the soak keeps every write', async ({
  browser,
}, testInfo) => {
  testInfo.setTimeout(600_000);
  // After `moves`, production serves 0.3.4. Run alone, v3 is applied.
  if (!['0.3.0', '0.3.4'].includes(servingVersion() ?? '')) {
    ensureServing(stack().programV2, '0.2.0');
    applyProgram(stack().programV3);
  }
  const from = servingVersion() ?? '';
  const sealed = from === '0.3.4';
  const to = sealed ? '0.3.7' : '0.3.3';
  const people = await Promise.all(['Ann', 'Ben', 'Cat'].map((name) => owner(name)));
  const dev = await developer(browser);
  const ordo = (args: string[]) => {
    const r = dev.ordo([...args, ...door()]);
    expect(r.code, `ordo ${args.join(' ')}\n${r.out}\n${r.err}`).toBe(0);
    return r.out;
  };
  try {
    const tenants: string[] = [];
    const titles: string[][] = [];
    for (const p of people) {
      const tenant = watchTenant(p.page);
      await openV2(p.page);
      const mine = [0, 1, 2, 3, 4, 5].map((i) => unique(`rolled-${i}-`));
      for (const t of mine) await addNote(p.page, t, `Written before the rollout: ${t}.`);
      tenants.push(tenant());
      titles.push(mine);
    }
    // Ann shares a note with Ben before the rollout (11b part 3).
    const [ann, ben] = [people[0]!.page, people[1]!.page];
    const sharedTitle = unique('shared-for-ben-');
    await test.step('Ann shares a note with Ben, who reads it', async () => {
      await addNote(ann, sharedTitle, 'Ann wrote this for Ben.');
      const link = await invitationFor(ann, sharedTitle);
      await ann.getByRole('button', { name: 'Close', exact: true }).click();
      await acceptInvitation(ben, link, 'Ben', sharedTitle);
      await openV2(ben);
    });

    await test.step(`${to} is stored, and its step runs on the deployment's own database`, () => {
      expect(ordo(['apply', renamedTitle(to, sealed, REFUSED), '--no-activate'])).toContain(to);
      expect(ordo(['evolve', '--from', from, '--to', to])).toContain('B4');
      // After `moves`, the deployment's own database already records that
      // project's version, which renamed the title. `ordo apply` stages the
      // new check there rather than add it, so the step from that version
      // (the check alone) runs there first.
      const run = dev.ordo(['evolve', '--from', from, '--to', to, '--run', ...door()]);
      const ahead = /records version `([0-9.]+)`, and this step starts from/.exec(`${run.out}${run.err}`)?.[1];
      if (run.code !== 0 && ahead) {
        ordo(['evolve', '--from', ahead, '--to', to]);
        ordo(['evolve', '--from', ahead, '--to', to, '--run']);
      } else {
        expect(run.code, `ordo evolve --run\n${run.out}\n${run.err}`).toBe(0);
      }
      ordo(['versions', 'set', 'backfill.chunk_rows', '1', '--reason', 'one row per turn, to type during it']);
      ordo(['versions', 'set', 'backfill.budget', '1', '--reason', 'one chunk per turn, to type during it']);
      // 11b part 2: the 3 tenants' moves run at once, one per database.
      expect(ordo(['versions', 'set', 'backfill.concurrency', '3', '--reason', 'the 3 move at once'])).toMatch(
        /concurrency +3 moves/,
      );
      expect(ordo(['versions', 'set', 'backfill.per_database', '1', '--reason', 'one move per database'])).toMatch(
        /per database +1 moves/,
      );
      // Out of range: refused, naming the setting.
      const bad = dev.ordo(['versions', 'set', 'backfill.concurrency', '0', '--reason', 'none at once', ...door()]);
      expect(bad.code).not.toBe(0);
      expect(`${bad.out}${bad.err}`).toContain('backfill.concurrency');
    });

    await test.step('the rollout starts each tenant on its stored step', async () => {
      const started = ordo(['rollout', 'start', to, '--strategy', 'group', '--group', tenants.join(','), '--canary-wait', '1d', '--batch-interval', '1s']);
      expect(started).toContain(`to ${to}`);
      // Each is in a move with dual writes, copying one row per turn.
      await expect
        .poll(() => ordo(['rollout', 'status']), { timeout: 60_000, intervals: [1_000] })
        .toMatch(new RegExp(`moving +${tenants[0]}: backfilling`));
    });

    const during = people.map(() => unique('retitled-during-'));
    const added = people.map(() => unique('added-during-'));
    await test.step('each person retitles a note and adds one during the copy', async () => {
      for (const [i, p] of people.entries()) {
        await retitle(p.page, titles[i]![5]!, during[i]!);
        await addNote(p.page, added[i]!, 'Added while the rollout copied.');
      }
    });

    const soaked = people.map(() => unique('retitled-in-soak-'));
    await test.step(`the 3 move to ${to} and soak; each reads what they typed`, async () => {
      await expect
        .poll(() => ordo(['rollout', 'status']), { timeout: 240_000, intervals: [2_000] })
        .toMatch(/tenants +3 moved, \d+ queued, 0 waiting, 0 moving, 0 failed/);
      for (const [i, p] of people.entries()) {
        await p.page.reload();
        await signedIn(p.page);
        for (const t of titles[i]!.slice(0, 5)) await holds(p.page, t);
        await holds(p.page, during[i]!);
        await holds(p.page, added[i]!);
        await retitle(p.page, titles[i]![0]!, soaked[i]!);
      }
    });

    await test.step(`in the soak, a title ${to}'s check refuses is refused by name; the check waits for the close`, async () => {
      const p = people[2]!.page;
      await noteInList(p, titles[2]![1]!).click();
      await p.getByRole('textbox', { name: 'Title', exact: true }).fill(REFUSED);
      await p.getByRole('button', { name: 'Save', exact: true }).click();
      await expect(p.getByRole('alert')).toContainText('holds the check', { timeout: 30_000 });
      await expect(p.getByRole('alert')).toContainText(to);
      await p.getByRole('button', { name: 'Close', exact: true }).click();
      await p.reload();
      await signedIn(p);
      await holds(p, titles[2]![1]!);
      await expect(noteInList(p, REFUSED)).toHaveCount(0);
    });

    await test.step(`the rollout stops; the dry run names each as kept`, () => {
      expect(ordo(['rollout', 'abort', '--reason', 'roll back inside the soak'])).toContain('aborted');
      const dry = ordo(['rollback', '--version', from, '--dry-run']);
      for (const t of tenants) {
        expect(dry).toMatch(new RegExp(`tenant +${t} on ${to.replace(/\./g, '\\.')}: in its soak`));
      }
      // The rollout was aborted, so `from` is still active and no other
      // tenant moves: none loses a write.
      expect(dry).not.toContain('LOSES');
    });

    await test.step(`ordo rollback puts the 3 back on ${from}`, () => {
      const back = ordo(['rollback', '--version', from]);
      for (const t of tenants) expect(back).toContain(`reverted   ${t} back on ${from} in its soak`);
    });

    await test.step(`Ben's share, closed by Ann's move, stays closed after the rollback; shared again, he reads it`, async () => {
      await ben.reload();
      await signedIn(ben);
      await ben.getByRole('button', { name: 'Shared with me', exact: true }).click();
      await expect(sharedList(ben).getByText('This share was closed.')).toBeVisible({ timeout: 60_000 });
      await expect(sharedList(ben).getByText(sharedTitle)).toHaveCount(0);
      const again = await invitationFor(ann, sharedTitle);
      await ann.getByRole('button', { name: 'Close', exact: true }).click();
      await acceptInvitation(ben, again, 'Ben', sharedTitle);
      await openV2(ben);
    });

    await test.step(`after a reload on ${from}, every title each person typed is there`, async () => {
      for (const [i, p] of people.entries()) {
        await p.page.reload();
        await signedIn(p.page);
        await holds(p.page, added[i]!);
        await holds(p.page, during[i]!);
        await holds(p.page, soaked[i]!);
        await expect(p.page.getByRole('alert')).toHaveCount(0);
      }
    });
  } finally {
    // The settings are the deployment's; the projects after this one run
    // with the defaults.
    for (const s of ['backfill.chunk_rows', 'backfill.budget', 'backfill.concurrency', 'backfill.per_database']) {
      dev.ordo(['versions', 'set', s, '--clear', '--reason', 'back to the default', ...door()]);
    }
    await dev.close();
    for (const p of people) await p.close();
  }
});
