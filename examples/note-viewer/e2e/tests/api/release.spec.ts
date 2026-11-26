/**
 * The release check's peers and consumers, through the door
 * (plan-evolution-federation.md §5, step F6; owner ruling 2026-11-23, Q16;
 * plan-federation-grants.md §6, step G8).
 *
 * The viewer's apply through its door (tests/door) recorded its use at
 * notes v2 with the Ordo release it runs, and kept the release notes v2
 * answered. `ordo release-check --direct` run as note_viewer names its peer
 * `notes` and that release; run as notes v2 it names its consumer
 * note_viewer (by the grant tied to `viewer`) and its release. Neither
 * fails the check, and neither app reads the other's database for it.
 *
 * A regression looks like a release check that leaves out the peer or the
 * consumer, or names no release for either.
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

test('the viewer’s release check names its peer notes and the release it answered', () => {
  const report = neighbours('viewer');
  const peer = report.neighbours.find((n) => n.kind === 'peer' && n.name === 'notes');
  expect(peer, JSON.stringify(report.neighbours)).toBeDefined();
  expect(peer?.at).toBe('notes');
  // One build runs both, so the peer is on this release.
  expect(peer?.release).toBe(report.release);
  expect(peer?.otherRelease).toBe(false);
});

test('notes v2’s release check names its consumer note_viewer and the release it sent', () => {
  const report = neighbours('notes');
  const consumer = report.neighbours.find((n) => n.kind === 'consumer');
  expect(consumer, JSON.stringify(report.neighbours)).toMatchObject({ name: 'note_viewer', at: 'viewer' });
  expect(consumer?.release).toBe(report.release);
  expect(consumer?.otherRelease).toBe(false);
});

test('neither role is granted the other’s release table', () => {
  expect(() => sqlAs('viewer_app', 'SELECT release FROM notes."$ordorelease"')).toThrow(/permission denied|does not exist/);
  expect(() => sqlAs('notes_app', 'SELECT release FROM viewer."$ordorelease"')).toThrow(/permission denied|does not exist/);
});
