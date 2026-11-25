/**
 * notes takes `notesAccess` off its `reader` line (plan-evolution-federation.md
 * §7, test 5; owner ruling 2026-11-23, Q2).
 *
 * notes moves to 0.3.4, whose line reads `reader note_viewer reads notes`.
 * The change takes effect when the version is applied: Postgres refuses the
 * viewer's role `notesAccess` at once, and still serves `notes`. The
 * viewer's next start is refused, and the refusal quotes the line notes
 * must carry for it to start.
 *
 * A regression looks like the viewer's role still reading `notesAccess`
 * after the apply, a viewer that starts without its grant, or a refusal
 * that does not say what to add.
 *
 * It narrows the grants for the whole stack, so it runs last.
 */

import { spawnSync } from 'node:child_process';
import { copyFileSync, cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { applyNotes, envOf, sqlAs, stack } from '../support.js';

/** The 0.3.4 program: 0.3.3 (fixtures/notes-no-tags) with `notesAccess` off the line. */
function narrowed(): string {
  const dir = mkdtempSync(join(tmpdir(), 'notes-narrowed-'));
  copyFileSync(
    fileURLToPath(new URL('../../fixtures/notes-no-tags.ordo.fixture', import.meta.url)),
    join(dir, 'notes.ordo'),
  );
  const line = 'reader note_viewer reads notes, notesAccess';
  const source = readFileSync(join(dir, 'notes.ordo'), 'utf8');
  expect(source).toContain(line);
  writeFileSync(
    join(dir, 'notes.ordo'),
    source.replace('domain notes @ 0.3.3', 'domain notes @ 0.3.4').replace(line, 'reader note_viewer reads notes'),
  );
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

test('notes takes `notesAccess` off its line: the grant ends at the apply, and the viewer cannot start', () => {
  test.setTimeout(300_000);
  sqlAs('viewer_app', 'SELECT count(*) FROM notes."notesAccess"');

  const applied = applyNotes(narrowed());
  expect(applied).toContain('0.3.4');

  // Postgres is the control: the table off the line is refused at once.
  expect(() => sqlAs('viewer_app', 'SELECT count(*) FROM notes."notesAccess"')).toThrow(/permission denied/);
  sqlAs('viewer_app', 'SELECT id, heading FROM notes.notes LIMIT 1');

  // A second viewer node starts as the first would after a restart, on a
  // port of its own. It is refused, and says what notes must add.
  const env = { ...envOf('viewer'), ORDO_HOST_ADDR: '127.0.0.1:4759' };
  const started = spawnSync(stack().host, [], { env, encoding: 'utf8', timeout: 120_000 });
  const said = `${started.stdout ?? ''}${started.stderr ?? ''}`;
  expect(started.signal, `the node was still running:\n${said}`).toBeNull();
  expect(started.status, said).not.toBe(0);
  expect(said).toContain('cannot read `notesAccess`');
  expect(said).toContain('reader <name> reads notes, notesAccess');
  expect(said).toContain("notes's definition file");
});
