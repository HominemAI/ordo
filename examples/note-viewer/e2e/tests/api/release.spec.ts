/**
 * The sign-in contract and the release check's peers and readers
 * (plan-evolution-federation.md §5, step F6; owner ruling 2026-11-23, Q16).
 *
 * Each deployment records the Ordo release it started under in
 * `$ordorelease`, and notes v2 grants its `release` column to note_viewer's
 * role with the sign-in columns, and nothing else of that row.
 * `ordo release-check --direct` run as note_viewer names its peer `notes`
 * and the release it runs; run as notes v2 it names its reader
 * `note_viewer` by its role. Neither fails the check.
 *
 * A regression looks like the viewer's role refused the release or granted
 * the rest of the row, or a release check that leaves out the peer or the
 * reader.
 */

import { expect, test } from '@playwright/test';
import { ordoAs, sqlAs } from '../support.js';

interface Neighbour {
  kind: string;
  name: string;
  at: string;
  release: string | null;
  otherRelease: boolean;
}

function neighbours(app: 'notes' | 'viewer'): { release: string; neighbours: Neighbour[] } {
  const r = ordoAs(app, ['release-check', '--direct', '--json']);
  expect([0, 1], `${r.out}\n${r.err}`).toContain(r.code);
  return JSON.parse(r.out) as { release: string; neighbours: Neighbour[] };
}

test('the viewer’s role reads the release notes v2 runs, and no other column of that row', () => {
  expect(sqlAs('viewer_app', 'SELECT release FROM notes."$ordorelease"')).toMatch(/^\d+\.\d+\.\d+/);
  expect(() => sqlAs('viewer_app', 'SELECT started_at FROM notes."$ordorelease"')).toThrow(/permission denied/);
});

test('the viewer’s release check names its peer notes and the release it runs', () => {
  const report = neighbours('viewer');
  const peer = report.neighbours.find((n) => n.kind === 'peer' && n.name === 'notes');
  expect(peer, JSON.stringify(report.neighbours)).toBeDefined();
  expect(peer?.at).toBe('notes');
  // One build runs both, so the peer is on this release.
  expect(peer?.release).toBe(report.release);
  expect(peer?.otherRelease).toBe(false);
});

test('notes v2’s release check names its reader note_viewer by its role', () => {
  const report = neighbours('notes');
  const reader = report.neighbours.find((n) => n.kind === 'reader');
  expect(reader, JSON.stringify(report.neighbours)).toMatchObject({ name: 'note_viewer', at: 'viewer_app' });
});
