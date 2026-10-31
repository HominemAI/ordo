/**
 * The notes kept on this device.
 *
 * `myNotes` is a syncable read (`sync: notes.id` in the generated table), so
 * the SDK keeps its rows in a replica in IndexedDB: one replica per tenant and
 * per person, its rows sealed with a key the browser keeps and the page cannot
 * read. The list on screen is drawn from the replica, so it shows at once and
 * shows with the network off.
 *
 * The SDK's `openDevice` builds the replica and the sync manager, and
 * `<OrdoProvider device={...}>` takes what it answers. The hooks declare the
 * ops they read through it.
 *
 * What this does not do, plainly:
 *
 * - **No writes while offline.** There is no queue and no rollback. A write
 *   with no network is refused by the page, and the text stays in the editor.
 * - **The seal is not a lock.** The key lives in the same IndexedDB as the
 *   rows, so it keeps a raw dump of the database from being readable and does
 *   nothing against a script running on this origin. Keys under a passkey and
 *   a lock when idle are owed (checklist 22m).
 * - **No polling.** The server pushes each change down the open feed, a
 *   tenant made after the server booted included. The SDK reopens its feeds
 *   when the browser comes back online. This page runs no timer and listens
 *   for no focus event.
 */

import type { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import type { Backend } from '@hominemai/ordo-sdk/replica/backend.js';
import { openDevice, type OrdoDevice } from '@hominemai/ordo-sdk/replica/device.js';

/** The IndexedDB database the notes page has always used, so moving onto `openDevice` re-syncs nothing. */
export const NOTES_STORE = 'ordo-notes';

/** Open the device for `principal` in `space`. The hooks say what to keep in it. */
export function openNotesDevice(
  client: OrdoClient,
  space: string,
  principal: string,
  backend?: Backend,
): Promise<OrdoDevice> {
  return openDevice({ client, space, principal, store: NOTES_STORE, ...(backend ? { backend } : {}) });
}
