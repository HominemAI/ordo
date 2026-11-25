/**
 * Notes v3 splits an entity (forward-gaps ask 8, owner: "declared
 * correspondence tool based on prev"). The new version moves `notes.starred`
 * into an entity of its own, `stars (prev: notes)`, one star row per note,
 * linked by `note -> notes owned unique`. Its `starNote` and `starredNotes`
 * read and write `stars`; the page reads `notes` as before. The developer,
 * signed in with `ordo login`:
 *
 * 1. Stores the version and asks for its step with no rule. It is refused,
 *    naming `--correspond stars.note`.
 * 2. Gives the rule. The step stores as B13, with a preview on 3 rows.
 * 3. `ordo evolve --tenant <Uma>`: the copy, verify, the switch, a soak.
 *    On the new version every note reads, the note starred before the move
 *    is starred, and Uma stars another and adds a note.
 * 4. `--revert` in the soak: on the version before, both stars read and the
 *    new note is there.
 * 5. The move switches again and `--close-soak` ends it: `notes.starred` is
 *    orphaned, and every note and both stars still read.
 *
 * A regression looks like a refused step that names no option, a star lost
 * by the copy, a star set on the new version missing after the revert, or
 * a note missing after the split.
 *
 * It stores 0.3.10, above `split`'s 0.3.9 and below `branch`'s 0.5.x,
 * so it runs between them. Chromium only.
 */

import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import { door, developer } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, ensureServing, servingVersion, signedIn, stack } from '../support.js';
import { holds, openV2 } from './moves.js';

test.describe.configure({ mode: 'serial' });

const TO = '0.3.10';

/** Notes v3 at 0.3.10 with `starred` split off into `stars`, made from v3's own file. */
function splitStars(sealed: boolean): string {
  const v3 = readFileSync(join(stack().programV3, 'notes.ordo'), 'utf8');
  const swaps: [string, string][] = [
    ['domain notes @ 0.3.0', `domain notes @ ${TO}`],
    ['reader note_viewer reads notes, notesAccess', 'reader note_viewer reads notesAccess'],
    ['  starred: bool = false\n', ''],
    [
      'entity attachments {',
      'entity stars (key: star, prev: notes) {\n  note -> notes owned unique\n  starred: bool = false\n}\n\nentity attachments {',
    ],
    [
      'op starNote($id: id, $rev: int, $starred: bool) =\n  set notes { id: $id, rev: $rev, starred: $starred }',
      'op starNote($id: id, $starred: bool) =\n  set stars { note: $id, starred: $starred }',
    ],
    [
      '  notes(starred == true)[id, rev, title, created]\n  sort -created',
      '  stars(starred == true)[id, note]',
    ],
  ];
  if (sealed) swaps.push(['  body:  text = ""\n', '  body:  text sealed\n']);
  let source = v3;
  for (const [from, to] of swaps) {
    expect(source, `notes v3 still holds \`${from}\``).toContain(from);
    source = source.split(from).join(to);
  }
  const dir = mkdtempSync(join(tmpdir(), `notes-${TO}-`));
  writeFileSync(join(dir, 'notes.ordo'), source);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

type Who = { token: string; space: string };

/** The token and tenant the page sends to its door, as last seen. */
function watchSession(page: Page): () => Who {
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

type Row = { id: string; rev?: number; title?: string; note?: string };

/** One op in her tenant, as the page would call it: a read with no body. */
async function op(request: APIRequestContext, who: Who, name: string, body?: object): Promise<Row[]> {
  const headers = { 'X-Ordo-Space': who.space, Authorization: `Bearer ${who.token}`, Origin: stack().page };
  const r =
    body === undefined
      ? await request.get(`${stack().door}/op/${name}`, { headers })
      : await request.post(`${stack().door}/op/${name}`, {
          data: body,
          headers: { ...headers, 'Idempotency-Key': `e2e-${Date.now()}-${Math.random()}` },
        });
  const text = await r.text();
  expect(r.status(), `${name}: ${text}`).toBe(200);
  return body === undefined ? ((JSON.parse(text) as { rows: Row[] }).rows ?? []) : [];
}

/** The ids of the notes `starredNotes` answers, on either version. */
async function starred(request: APIRequestContext, who: Who): Promise<string[]> {
  const rows = await op(request, who, 'starredNotes');
  return rows.map((r) => r.note ?? r.id).sort();
}

test('notes v3 splits an entity: the rule is asked for, every star reads, and a revert in the soak keeps a new one', async ({
  browser,
  request,
}, testInfo) => {
  testInfo.setTimeout(420_000);
  // After `split`, production serves 0.3.4. Run alone, v3 is applied.
  if (!['0.3.0', '0.3.4'].includes(servingVersion() ?? '')) {
    ensureServing(stack().programV2, '0.2.0');
    applyProgram(stack().programV3);
  }
  const from = servingVersion() ?? '';
  const uma = await owner('Uma');
  const dev = await developer(browser);
  try {
    const page = uma.page;
    const session = watchSession(page);
    await openV2(page);
    const titles = ['Starred: the trip', 'Plain: groceries', 'Plain: the plan'];
    for (const t of titles) await addNote(page, t, `Written before the split: ${t}.`);
    const who = session();
    const notes = await op(request, who, 'allNotes');
    const byTitle = (t: string) => {
      const n = notes.find((x) => x.title === t);
      expect(n, `${t} is listed`).toBeTruthy();
      return n as Row;
    };
    const first = byTitle(titles[0]);
    const second = byTitle(titles[1]);
    await op(request, who, 'starNote', { id: first.id, rev: first.rev, starred: true });
    expect(await starred(request, who)).toEqual([first.id]);

    await test.step(`${TO} is stored, and its step is refused until the rule is given`, () => {
      const stored = dev.ordo(['apply', splitStars(from === '0.3.4'), '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      const refused = dev.ordo(['evolve', '--from', from, '--to', TO, ...door()]);
      expect(refused.code, refused.out).not.toBe(0);
      expect(refused.err).toContain('split_rule_required');
      expect(refused.err).toContain('--correspond stars.note');
    });

    await test.step('with the rule, the step stores as an entity split, with a preview', () => {
      const plan = dev.ordo(['evolve', '--from', from, '--to', TO, '--correspond', 'stars.note', ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      expect(plan.out).toContain('B13');
      expect(plan.out).toMatch(/entity `notes` is split: `stars` takes `notes\.starred`/);
      expect(plan.out).toContain('PREVIEW');
    });

    await test.step(`Uma's tenant moves to ${TO}, in a soak`, () => {
      const ran = dev.ordo(['evolve', '--tenant', who.space, '--to', TO, ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain(`pinned     ${TO} (was ${from})`);
      expect(ran.out).toContain(`move       ${from} to ${TO}: soaking`);
    });

    const added = 'Added: on the split version';
    await test.step(`on ${TO}, every note reads, the star moved, and Uma stars another`, async () => {
      await page.reload();
      await signedIn(page);
      for (const t of titles) await holds(page, t);
      expect(await starred(request, who)).toEqual([first.id]);
      await op(request, who, 'starNote', { id: second.id, starred: true });
      expect(await starred(request, who)).toEqual([first.id, second.id].sort());
      await addNote(page, added, 'Added on the split version.');
      await holds(page, added);
    });

    await test.step('reverted inside the soak, both stars and the new note are there', async () => {
      const back = dev.ordo(['evolve', '--tenant', who.space, '--revert', ...door()]);
      expect(back.code, `${back.out}\n${back.err}`).toBe(0);
      expect(back.out).toContain(`pinned     ${from} (was ${TO})`);
      await page.reload();
      await signedIn(page);
      for (const t of [...titles, added]) await holds(page, t);
      expect(await starred(request, who)).toEqual([first.id, second.id].sort());
      await expect(page.getByRole('alert')).toHaveCount(0);
    });

    await test.step('switched again and the soak closed: starred is orphaned, every star reads', async () => {
      const again = dev.ordo(['evolve', '--tenant', who.space, '--to', TO, ...door()]);
      expect(again.code, `${again.out}\n${again.err}`).toBe(0);
      const closed = dev.ordo(['evolve', '--tenant', who.space, '--close-soak', ...door()]);
      expect(closed.code, `${closed.out}\n${closed.err}`).toBe(0);
      expect(closed.out).toContain('orphaned: notes.starred');
      await page.reload();
      await signedIn(page);
      for (const t of [...titles, added]) await holds(page, t);
      expect(await starred(request, who)).toEqual([first.id, second.id].sort());
    });
  } finally {
    await dev.close();
    await uma.close();
  }
});
