/**
 * note_viewer becomes a consumer of notes v2's door, as a deployment owner
 * makes it one (plan-federation-grants.md §4 and §7, step G8).
 *
 * 1. Before its key is registered, the viewer's apply through its door is
 *    refused, naming `std.peer.registerKey`.
 * 2. The developer, a member of `viewer` at the identity service, registers
 *    the viewer's consumer key with `std.peer.registerKey`, passing their
 *    own token for `viewer`.
 * 3. The viewer's apply through its door checks each op of its `peer notes`
 *    block against notes v2's grant, and records the use at notes v2.
 *
 * A regression looks like an apply that passes with no key, a key the
 * service does not take, or an apply that does not record its use.
 */

import { expect, test } from '@playwright/test';
import { fileURLToPath } from 'node:url';
import { developer, door } from '../developer.js';
import { stack } from '../support.js';

const viewerDir = fileURLToPath(new URL('../../..', import.meta.url));

test('the viewer registers its key and its apply is checked against notes v2’s grant', async ({ browser }) => {
  test.setTimeout(240_000);
  const dev = await developer(browser);
  try {
    const before = dev.ordo('viewer', ['apply', viewerDir, ...door('viewer')]);
    const saidBefore = `${before.out}${before.err}`;
    expect(before.code, saidBefore).not.toBe(0);
    expect(saidBefore).toContain('std.peer.registerKey');

    const token = dev.token('viewer');
    const res = await fetch(`${stack().viewerDoor}/op/std.peer.registerKey`, {
      method: 'POST',
      headers: {
        authorization: `Bearer ${token}`,
        'content-type': 'application/json',
        'x-ordo-space': 'system',
      },
      body: JSON.stringify({ token }),
    });
    const body = await res.text();
    expect(res.status, body).toBe(200);
    expect(body).toContain('"handle":"viewer"');

    const applied = dev.ordo('viewer', ['apply', viewerDir, ...door('viewer')]);
    expect(applied.code, `${applied.out}\n${applied.err}`).toBe(0);
    expect(applied.out).toContain('peer        notes: 2 op(s) granted');
    expect(applied.out).toContain('peer        notes: told it this version calls myNotes, publicNotes');
  } finally {
    await dev.close();
  }
});
