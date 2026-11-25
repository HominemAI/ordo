/**
 * Notes v3 merges an entity (forward-gaps asks 7 and 8, owner: "dev named",
 * "declared correspondence tool based on prev"). A version adds `labels`,
 * one label row per note, linked by `note -> notes owned unique`. The next
 * merges it back: `label: text = "" prev(labels.label)` in `notes`, and
 * `labels` removed. Its `labelNote` and `labelledNotes` read and write
 * `notes`; the page reads `notes` as before. The developer, signed in with
 * `ordo login`:
 *
 * 1. Stores 0.3.11, which adds `labels`, and moves Vera's tenant to it. She
 *    labels a note there.
 * 2. Stores 0.3.12 and asks for its step with no rule. It is refused,
 *    naming `--correspond labels.note`.
 * 3. Gives the rule. The step stores as B14, with a preview on 3 rows.
 * 4. `ordo evolve --tenant <Vera>`: the copy, verify, the switch, a soak.
 *    On the new version every note reads, the label merged, and Vera
 *    labels another note and adds one.
 * 5. `--revert` in the soak: on the version before, both labels read and
 *    the new note is there.
 * 6. The move switches again and `--close-soak` ends it: `labels` is
 *    orphaned, and every note and both labels still read.
 *
 * 7. A named rule (forward-gaps ask 7): 0.3.13 adds `marks`, any number
 *    per note (`note -> notes owned`, not unique), and Vera marks a note
 *    twice. 0.3.14 moves `mark` into `notes`. Its step is refused until
 *    the rule names which mark gives the value; `--correspond "marks.note
 *    latest by at" --one-way` stores it, and the move gives the note its
 *    latest mark. The close orphans `marks.mark`.
 *
 * A regression looks like a refused step that names no option, a label
 * lost by the copy, a label set on the new version missing after the
 * revert, a note missing after the merge, or a note holding another mark
 * than its latest.
 *
 * It stores 0.3.11 to 0.3.14, above `entity-split`'s 0.3.10 and below
 * `branch`'s 0.5.x, so it runs between them. Chromium only.
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

const SPLIT = '0.3.11';
const TO = '0.3.12';
const MARKS = '0.3.13';
const MARKED = '0.3.14';

/**
 * Notes v3 at `version`, made from v3's own file, off the reader line:
 * with `labels` beside `notes` (`merged` false), or merged into it.
 */
function labelled(version: string, merged: boolean, sealed: boolean): string {
  const v3 = readFileSync(join(stack().programV3, 'notes.ordo'), 'utf8');
  const swaps: [string, string][] = [
    ['domain notes @ 0.3.0', `domain notes @ ${version}`],
    ['reader note_viewer reads notes, notesAccess', 'reader note_viewer reads notesAccess'],
  ];
  if (merged) {
    swaps.push(['  starred: bool = false\n', '  starred: bool = false\n  label: text = "" prev(labels.label)\n']);
  } else {
    swaps.push([
      'entity attachments {',
      'entity labels (key: lbl) {\n  note -> notes owned unique\n  label: text = ""\n}\n\nentity attachments {',
    ]);
  }
  if (sealed) swaps.push(['  body:  text = ""\n', '  body:  text sealed\n']);
  let source = v3;
  for (const [from, to] of swaps) {
    expect(source, `notes v3 still holds \`${from}\``).toContain(from);
    source = source.split(from).join(to);
  }
  source += merged
    ? '\nop labelNote($id: id, $rev: int, $label: text) =\n  set notes { id: $id, rev: $rev, label: $label }\n\n' +
      'op labelledNotes unbounded =\n  notes(label != "")[id, label]\n'
    : '\nop labelNote($id: id, $label: text) =\n  set labels { note: $id, label: $label }\n\n' +
      'op labelledNotes unbounded =\n  labels(label != "")[id, note, label]\n';
  const dir = mkdtempSync(join(tmpdir(), `notes-${version}-`));
  writeFileSync(join(dir, 'notes.ordo'), source);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

/**
 * Notes v3 merged (0.3.12's file) at `version`, with `marks` beside `notes`,
 * several per note (`moved` false), or its `mark` moved into `notes`.
 */
function marked(version: string, moved: boolean, sealed: boolean): string {
  let source = readFileSync(join(labelled(TO, true, sealed), 'notes.ordo'), 'utf8');
  const swaps: [string, string][] = [
    [`domain notes @ ${TO}`, `domain notes @ ${version}`],
    // The label landed in 0.3.12: its mark goes.
    [' prev(labels.label)', ''],
  ];
  if (moved) {
    swaps.push(['  label: text = ""\n', '  label: text = ""\n  mark: text = "" prev(marks.mark)\n']);
    swaps.push(['entity attachments {', 'entity marks (key: mk) {\n  note -> notes owned\n  at: int = 0\n}\n\nentity attachments {']);
  } else {
    swaps.push([
      'entity attachments {',
      'entity marks (key: mk) {\n  note -> notes owned\n  mark: text = ""\n  at: int = 0\n}\n\nentity attachments {',
    ]);
  }
  for (const [from, to] of swaps) {
    expect(source, `notes ${TO} still holds \`${from}\``).toContain(from);
    source = source.split(from).join(to);
  }
  source += moved
    ? '\nop markedNotes unbounded =\n  notes(mark != "")[id, mark]\n'
    : '\nop markNote($id: id, $mark: text, $at: int) =\n  set marks { note: $id, mark: $mark, at: $at }\n\n' +
      'op markedNotes unbounded =\n  marks(mark != "")[id, note, mark, at]\n';
  const dir = mkdtempSync(join(tmpdir(), `notes-${version}-`));
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

type Row = { id: string; rev?: number; title?: string; note?: string; label?: string; mark?: string };

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

/**
 * The first call of an op a version adds, a write with `body` or a read
 * without. The node rebuilds its door over every tenant once it loads a
 * version; until then it answers a member `server_busy` with a
 * `Retry-After`, which a client honours. Any other refusal fails at once:
 * `access_denied` here tells a person with access they have none (reported
 * fix new-ops-denied-during-rebuild).
 */
async function firstCall(request: APIRequestContext, who: () => Who, name: string, body?: object): Promise<Row[]> {
  const until = Date.now() + 90_000;
  for (;;) {
    const w = who();
    const headers = { 'X-Ordo-Space': w.space, Authorization: `Bearer ${w.token}`, Origin: stack().page };
    const r =
      body === undefined
        ? await request.get(`${stack().door}/op/${name}`, { headers })
        : await request.post(`${stack().door}/op/${name}`, {
            data: body,
            headers: { ...headers, 'Idempotency-Key': `e2e-${Date.now()}-${Math.random()}` },
          });
    const text = await r.text();
    if (r.status() === 200) return body === undefined ? ((JSON.parse(text) as { rows: Row[] }).rows ?? []) : [];
    expect(r.status(), `${name}: only server_busy is retried: ${text}`).toBe(503);
    expect(text, name).toContain('server_busy');
    expect(Date.now(), `${name}: still busy after 90 seconds`).toBeLessThan(until);
    const wait = Math.min(Math.max(Number(r.headers()['retry-after'] ?? '1'), 1), 5);
    await new Promise((done) => setTimeout(done, wait * 1_000));
  }
}

/** `note=label` for each labelled note, on either version. */
async function labels(request: APIRequestContext, who: Who): Promise<string[]> {
  const rows = await op(request, who, 'labelledNotes');
  return rows.map((r) => `${r.note ?? r.id}=${r.label}`).sort();
}

test('notes v3 merges an entity: the rule is asked for, every label reads, and a revert in the soak keeps a new one', async ({
  browser,
  request,
}, testInfo) => {
  testInfo.setTimeout(720_000);
  // After `entity-split`, production serves 0.3.4. Run alone, v3 is applied.
  if (!['0.3.0', '0.3.4'].includes(servingVersion() ?? '')) {
    ensureServing(stack().programV2, '0.2.0');
    applyProgram(stack().programV3);
  }
  const from = servingVersion() ?? '';
  const sealed = from === '0.3.4';
  const vera = await owner('Vera');
  const dev = await developer(browser);
  try {
    const page = vera.page;
    const session = watchSession(page);
    await openV2(page);
    const titles = ['Labelled: the trip', 'Plain: groceries', 'Plain: the plan'];
    for (const t of titles) await addNote(page, t, `Written before the merge: ${t}.`);
    // The token the page holds, read again after each reload: a token
    // names the ops of the version it was signed for.
    const who = session();
    const notes = await op(request, who, 'allNotes');
    const byTitle = (t: string) => {
      const n = notes.find((x) => x.title === t);
      expect(n, `${t} is listed`).toBeTruthy();
      return n as Row;
    };
    const first = byTitle(titles[0]);
    const second = byTitle(titles[1]);

    await test.step(`${SPLIT} adds labels beside notes, and Vera labels a note there`, async () => {
      const stored = dev.ordo(['apply', labelled(SPLIT, false, sealed), '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      const plan = dev.ordo(['evolve', '--from', from, '--to', SPLIT, ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      const ran = dev.ordo(['evolve', '--tenant', who.space, '--to', SPLIT, ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain(`pinned     ${SPLIT} (was ${from})`);
      await page.reload();
      await signedIn(page);
      for (const t of titles) await holds(page, t);
      await firstCall(request, session, 'labelNote', { id: first.id, label: 'travel' });
      expect(await labels(request, session())).toEqual([`${first.id}=travel`]);
    });

    await test.step(`${TO} is stored, and its step is refused until the rule is given`, () => {
      const stored = dev.ordo(['apply', labelled(TO, true, sealed), '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      const refused = dev.ordo(['evolve', '--from', SPLIT, '--to', TO, ...door()]);
      expect(refused.code, refused.out).not.toBe(0);
      expect(refused.err).toContain('move_rule_required');
      expect(refused.err).toContain('--correspond labels.note');
    });

    await test.step('with the rule, the step stores as an entity merge, with a preview', () => {
      const plan = dev.ordo(['evolve', '--from', SPLIT, '--to', TO, '--correspond', 'labels.note', ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      expect(plan.out).toContain('B14');
      expect(plan.out).toMatch(/entity `labels` is merged into `notes`/);
      expect(plan.out).toContain('PREVIEW');
    });

    await test.step(`Vera's tenant moves to ${TO}, in a soak`, () => {
      const ran = dev.ordo(['evolve', '--tenant', who.space, '--to', TO, ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain(`pinned     ${TO} (was ${SPLIT})`);
      expect(ran.out).toContain(`move       ${SPLIT} to ${TO}: soaking`);
    });

    const added = 'Added: on the merged version';
    await test.step(`on ${TO}, every note reads, the label merged, and Vera labels another`, async () => {
      await page.reload();
      await signedIn(page);
      for (const t of titles) await holds(page, t);
      // The first read of the version the tenant now serves may be told
      // to retry while the node's door rebuilds, as the first call may.
      const read = await firstCall(request, session, 'labelledNotes');
      expect(read.map((r) => `${r.note ?? r.id}=${r.label}`)).toEqual([`${first.id}=travel`]);
      const now = (await op(request, session(), 'allNotes')).find((x) => x.id === second.id);
      await firstCall(request, session, 'labelNote', { id: second.id, rev: now?.rev, label: 'shopping' });
      expect(await labels(request, session())).toEqual([`${first.id}=travel`, `${second.id}=shopping`].sort());
      await addNote(page, added, 'Added on the merged version.');
      await holds(page, added);
    });

    await test.step('reverted inside the soak, both labels and the new note are there', async () => {
      const back = dev.ordo(['evolve', '--tenant', who.space, '--revert', ...door()]);
      expect(back.code, `${back.out}\n${back.err}`).toBe(0);
      expect(back.out).toContain(`pinned     ${SPLIT} (was ${TO})`);
      await page.reload();
      await signedIn(page);
      for (const t of [...titles, added]) await holds(page, t);
      expect(await labels(request, session())).toEqual([`${first.id}=travel`, `${second.id}=shopping`].sort());
      await expect(page.getByRole('alert')).toHaveCount(0);
    });

    await test.step('switched again and the soak closed: labels is orphaned, every label reads', async () => {
      const again = dev.ordo(['evolve', '--tenant', who.space, '--to', TO, ...door()]);
      expect(again.code, `${again.out}\n${again.err}`).toBe(0);
      const closed = dev.ordo(['evolve', '--tenant', who.space, '--close-soak', ...door()]);
      expect(closed.code, `${closed.out}\n${closed.err}`).toBe(0);
      expect(closed.out).toMatch(/orphaned: .*\blabels\b/);
      await page.reload();
      await signedIn(page);
      for (const t of [...titles, added]) await holds(page, t);
      expect(await labels(request, session())).toEqual([`${first.id}=travel`, `${second.id}=shopping`].sort());
    });

    await test.step(`${MARKS} adds marks, several per note, and Vera marks a note twice`, async () => {
      const stored = dev.ordo(['apply', marked(MARKS, false, sealed), '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      const plan = dev.ordo(['evolve', '--from', TO, '--to', MARKS, ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      const ran = dev.ordo(['evolve', '--tenant', who.space, '--to', MARKS, ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain(`pinned     ${MARKS} (was ${TO})`);
      await page.reload();
      await signedIn(page);
      await firstCall(request, session, 'markNote', { id: first.id, mark: 'draft', at: 1 });
      await op(request, session(), 'markNote', { id: first.id, mark: 'final', at: 2 });
      const rows = await op(request, session(), 'markedNotes');
      expect(rows.map((r) => r.mark).sort()).toEqual(['draft', 'final']);
    });

    await test.step(`${MARKED} moves the mark into notes: refused until the rule names the latest`, () => {
      const stored = dev.ordo(['apply', marked(MARKED, true, sealed), '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      const refused = dev.ordo(['evolve', '--from', MARKS, '--to', MARKED, '--correspond', 'marks.note', ...door()]);
      expect(refused.code, refused.out).not.toBe(0);
      expect(refused.err).toContain('move_rule_required');
      expect(refused.err).toContain('latest by <field>');
      const plan = dev.ordo([
        'evolve',
        '--from',
        MARKS,
        '--to',
        MARKED,
        '--correspond',
        'marks.note latest by at',
        '--one-way',
        ...door(),
      ]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      expect(plan.out).toContain('B14');
      expect(plan.out).toContain('PREVIEW');
    });

    await test.step(`Vera's tenant moves to ${MARKED}: the note holds its latest mark`, async () => {
      const ran = dev.ordo(['evolve', '--tenant', who.space, '--to', MARKED, ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain(`pinned     ${MARKED} (was ${MARKS})`);
      await page.reload();
      await signedIn(page);
      for (const t of [...titles, added]) await holds(page, t);
      await expect
        .poll(async () => (await op(request, session(), 'markedNotes').catch(() => [])).map((r) => `${r.id}=${r.mark}`), {
          timeout: 90_000,
        })
        .toEqual([`${first.id}=final`]);
      const closed = dev.ordo(['evolve', '--tenant', who.space, '--close-soak', ...door()]);
      expect(closed.code, `${closed.out}\n${closed.err}`).toBe(0);
      expect(closed.out).toMatch(/orphaned: .*\bmarks\.mark\b/);
      expect((await op(request, session(), 'markedNotes')).map((r) => `${r.id}=${r.mark}`)).toEqual([`${first.id}=final`]);
    });
  } finally {
    await dev.close();
    await vera.close();
  }
});
