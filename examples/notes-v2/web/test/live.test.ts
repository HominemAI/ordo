/**
 * The device replica and the guest's window, against a running deployment.
 *
 * Skipped unless `NOTES_LIVE` is set. `crates/ordo-host/tests/notes_app.rs`
 * boots the real `ordo-host` binary, signs two people up through the
 * software authenticator, and runs this file with their tokens in
 * `NOTES_LIVE`, so the feed, the replica and the revocation below are the
 * SDK's own code talking to the server's own routes.
 *
 * What it shows:
 *
 * - a person's notes reach the device through the sync feed, and are read
 *   from the device with the network off;
 * - a raw dump of the IndexedDB holds no title, body, id or tenant;
 * - a write with no network is refused and changes nothing;
 * - a note shared with a second person appears on that person's device and
 *   leaves it when the sharer unshares it, and the whole window is erased
 *   from the device when the sharer closes it;
 * - a share link reads one note with no session at all, is read-only, and is
 *   refused, on a client already open and on a new open, once the owner
 *   revokes it;
 * - signing out erases everything.
 *
 * The page's own calls are the hooks' (`useCommit`, `useOpQuery`), which run
 * the same client methods used here (`commitOnce`, `fetchOp`), and the device
 * copy they share (`asDeviceCopy(device)`). No React is rendered: this is Node.
 */

import { asDeviceCopy, commitOnce, OrdoRefusal, type DescriptorClient as OrdoClient } from '@hominemai/ordo-react';
import { IDBFactory } from 'fake-indexeddb';
import { beforeAll, describe, expect, it } from 'vitest';
import { IndexedDbBackend } from '@hominemai/ordo-sdk/replica/backend.js';
import type { OrdoDevice } from '@hominemai/ordo-sdk/replica/device.js';
import { openNotesDevice } from '../src/device.js';
import { linkSecret } from '../src/hooks.js';
import { INTERFACE } from '../src/interface.js';
import {
  linkFor,
  linkOfShare,
  madeLinkOf,
  newestFirst,
  noteOf,
  readLinksOf,
  readersIn,
  shareLinkParams,
  shareParams,
  stamp,
  type Note,
  type ReadLink,
} from '../src/notes.js';
import { PasskeyError, Session } from '../src/session.js';
import { accept, guestSpaces, linkInto, windowInto } from '../src/shared.js';

interface Person {
  readonly token: string;
  readonly space: string;
  readonly principal: string;
}
interface Live {
  readonly url: string;
  readonly ada: Person;
  readonly bo: Person;
}

const raw = process.env['NOTES_LIVE'];
const live: Live | null = raw ? (JSON.parse(raw) as Live) : null;

let offline = false;
const fetchWhenReachable: typeof globalThis.fetch = async (input, init) => {
  if (offline) throw new TypeError('Failed to fetch');
  return globalThis.fetch(input, init);
};

/** Poll until `probe` answers, or fail with what was being waited for. */
async function until<T>(what: string, probe: () => Promise<T | null | false>, ms = 10_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const got = await probe();
    if (got) return got;
    if (Date.now() > end) throw new Error(`still waiting after ${ms} ms: ${what}`);
    await new Promise((r) => setTimeout(r, 40));
  }
}

// What the page runs through `useCommit` and `useOpQuery`, made here with the
// hooks' own client against the real door. A refusal is thrown, so a test can
// `rejects` on it.
async function commit(client: OrdoClient, op: string, values: Record<string, string | string[]>) {
  const out = await commitOnce(client, op, { values });
  if (out.status === 'refused') throw new OrdoRefusal(out.refusal);
  if (out.status === 'conflict') throw new OrdoRefusal(out.conflict.refusal);
  return out.response;
}

const addNote = (client: OrdoClient, title: string, body: string, now?: Date) =>
  commit(client, 'createNote', { title, body, tags: [], created: stamp(now) });

const rowsOf = async (client: OrdoClient, op: string) => (await client.fetchOp(op)).rows;

const readersOf = async (client: OrdoClient, note: Note) =>
  readersIn(await rowsOf(client, 'std.notesAccess.list'), note);

const stopSharing = (client: OrdoClient, note: Note, reader: { member: string }) =>
  commit(client, 'unshareNote', { note: note.id, member: reader.member });

const listNotes = async (client: OrdoClient) => (await rowsOf(client, 'myNotes')).map((r) => noteOf({ ...r }));

const shareNote = async (client: OrdoClient, origin: string, note: Note) =>
  linkOfShare((await commit(client, 'std.space.share', shareParams(note))).rows[0], origin);

const makeReadLink = async (client: OrdoClient, origin: string, note: Note) =>
  madeLinkOf((await commit(client, 'std.space.shareLink', shareLinkParams(note))).rows[0], origin);

const linksOf = async (client: OrdoClient, note: Note) => readLinksOf(await rowsOf(client, 'std.space.links'), note);

const revokeReadLink = (client: OrdoClient, link: ReadLink) => commit(client, 'std.space.revokeLink', { link: link.id });

const removeReader = (client: OrdoClient, reader: { member: string }) =>
  commit(client, 'std.space.revokeShare', { seat: reader.member });

/** What the page does for a link: open it, read the note, and call a refusal a closed link. */
async function readViaLink(endpoint: { baseUrl: string }, secret: string) {
  try {
    const { hooks } = await linkInto(endpoint, secret);
    return { kind: 'notes' as const, notes: await listNotes(hooks) };
  } catch (e) {
    if ((e instanceof PasskeyError && e.failure === 'refused') || e instanceof OrdoRefusal) {
      return { kind: 'closed' as const };
    }
    throw e;
  }
}

/** What the device holds, newest first: the list the hooks draw. Empty once access was withdrawn. */
async function notesOn(device: OrdoDevice): Promise<Note[]> {
  const records = await device.replica.scan('notes');
  return newestFirst(records.map((r) => noteOf({ ...r.row })));
}

/** Open a device and declare `myNotes` on it, as the first hook that reads it does. */
async function openDevice(
  client: Parameters<typeof openNotesDevice>[0],
  space: string,
  principal: string,
  backend: Parameters<typeof openNotesDevice>[3],
) {
  const device = await openNotesDevice(client, space, principal, backend);
  asDeviceCopy(device).declare(INTERFACE.requireOp('myNotes'), {});
  return device;
}

const titles =(notes: readonly Note[]) => notes.map((n) => n.title);

/** Everything the page's IndexedDB holds, as a copied disk would show it. */
async function dump(factory: IDBFactory, name: string): Promise<{ text: string; records: number; keys: number }> {
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const req = factory.open(name);
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
  const all = async (store: string): Promise<{ keys: IDBValidKey[]; values: unknown[] }> =>
    new Promise((resolve, reject) => {
      const tx = db.transaction(store, 'readonly');
      const s = tx.objectStore(store);
      const keys = s.getAllKeys();
      const values = s.getAll();
      tx.oncomplete = () => resolve({ keys: keys.result, values: values.result });
      tx.onerror = () => reject(tx.error);
    });
  const records = await all('records');
  const keys = await all('keys');
  db.close();
  const text = (v: unknown): string =>
    v instanceof Uint8Array ? Buffer.from(v).toString('latin1') : JSON.stringify(v) ?? '';
  return {
    text: [...records.keys.map(String), ...records.values.map(text), ...keys.keys.map(String)].join('\n'),
    records: records.keys.length,
    keys: keys.keys.length,
  };
}

describe.skipIf(live === null)('the notes device against a running deployment', () => {
  const l = live as Live;
  // Built in `beforeAll`: the body of a skipped `describe` still runs.
  let ada: Session;
  let bo: Session;
  let adaClient: OrdoClient;
  let adaSdk: ReturnType<Session['client']>;
  beforeAll(() => {
    const options = { baseUrl: l.url, fetch: fetchWhenReachable };
    ada = Session.fromToken(options, l.ada.token, l.ada.space);
    bo = Session.fromToken(options, l.bo.token, l.bo.space);
    adaClient = ada.hooksClient();
    adaSdk = ada.client();
  });

  const adaFactory = new IDBFactory();
  const boFactory = new IDBFactory();
  let adaDevice: OrdoDevice;
  let groceries: Note;

  it('keeps a person’s notes on the device through the sync feed, newest first', async () => {
    await addNote(adaClient, 'Groceries', 'milk', new Date('2026-09-29T10:00:00Z'));
    await addNote(adaClient, 'Trip', 'pack the tent', new Date('2026-09-29T10:00:01Z'));
    const backend = await IndexedDbBackend.open('ordo-notes', adaFactory);
    adaDevice = await openDevice(adaSdk, ada.space, ada.principal, backend);
    await adaDevice.manager.settled();
    expect(titles(await notesOn(adaDevice))).toEqual(['Trip', 'Groceries']);

    // A write made after the device loaded reaches it on the open feed, with
    // no reload and no nudge. Ada's tenant was made after the server booted,
    // and the server listens on it when the feed opens.
    await addNote(adaClient, 'Ideas', 'a notes app', new Date('2026-09-29T10:00:02Z'));
    await until('the feed to deliver the third note', async () => {
      const n = await notesOn(adaDevice);
      return n.length === 3 && n;
    });
    expect(titles(await notesOn(adaDevice))).toEqual(['Ideas', 'Trip', 'Groceries']);
    groceries = (await notesOn(adaDevice)).find((n) => n.title === 'Groceries') as Note;

    // What `useOpQuery('myNotes')` reads through the provider's device copy:
    // declared from the descriptor, read back from the replica in the order
    // the descriptor states, newest first. Committing a write through the
    // copy's hook (`afterCommit`) leaves it to the open feed.
    const handle = asDeviceCopy(adaDevice).declare(INTERFACE.requireOp('myNotes'), {});
    await asDeviceCopy(adaDevice).afterCommit();
    await until('the copy to answer', async () => {
      const rows = await asDeviceCopy(adaDevice).read(handle);
      return rows.length === 3 && rows;
    });
    expect((await asDeviceCopy(adaDevice).read(handle)).map((r) => r['title'])).toEqual(['Ideas', 'Trip', 'Groceries']);
  });

  it('holds nothing readable in the raw database', async () => {
    const d = await dump(adaFactory, 'ordo-notes');
    expect(d.records).toBeGreaterThanOrEqual(3);
    for (const plain of ['Groceries', 'Trip', 'Ideas', 'milk', 'pack the tent', 'note_', l.ada.space, l.ada.principal]) {
      expect(d.text).not.toContain(plain);
    }
  });

  it('reads the list with the network off, and refuses a write', async () => {
    offline = true;
    try {
      // A page reloaded with no network opens the replica it already holds.
      const backend = await IndexedDbBackend.open('ordo-notes', adaFactory);
      const reopened = await openNotesDevice(adaSdk, ada.space, ada.principal, backend);
      expect(titles(await notesOn(reopened))).toEqual(['Ideas', 'Trip', 'Groceries']);
      await reopened.close();

      await expect(addNote(adaClient, 'Written offline', 'x')).rejects.toMatchObject({ kind: 'unreachable' });
    } finally {
      offline = false;
    }
    // Nothing was written.
    await new Promise((r) => setTimeout(r, 300));
    expect(titles(await notesOn(adaDevice))).toEqual(['Ideas', 'Trip', 'Groceries']);
  });

  it('shows a shared note on the reader’s device, and only that note', async () => {
    const link = await shareNote(adaClient, 'http://localhost:5173', groceries);
    expect(link.url).toBe(linkFor('http://localhost:5173', link.ticket));

    const accepted = await accept(bo, link.ticket, 'Bo');
    expect(accepted.space).toBe(l.ada.space);
    const window = await windowInto(bo, l.ada.space);
    const backend = await IndexedDbBackend.open('ordo-notes', boFactory);
    const shared = await openDevice(window.sdk, l.ada.space, bo.principal, backend);
    await shared.manager.settled();
    expect(titles(await notesOn(shared))).toEqual(['Groceries']);
    (globalThis as Record<string, unknown>)['__boDevice'] = shared;

    // The door lists the tenant as one Bo is a guest in, once it has rebuilt.
    await until('the guest list to show the sharer', async () => {
      const spaces = await guestSpaces(bo);
      return spaces.some((s) => s.space === l.ada.space) && spaces;
    });
  });

  it('takes one note off the reader’s device when the sharer unshares it', async () => {
    const shared = (globalThis as Record<string, unknown>)['__boDevice'] as OrdoDevice;
    const readers = await readersOf(adaClient, groceries);
    expect(readers).toHaveLength(1);
    await stopSharing(adaClient, groceries, readers[0] as { member: string });
    await until('the note to leave the reader’s device', async () => (await notesOn(shared)).length === 0);
    expect(asDeviceCopy(shared).revoked).toBe(false);
  });

  it('erases the whole window from the reader’s device when the sharer closes it', async () => {
    const shared = (globalThis as Record<string, unknown>)['__boDevice'] as OrdoDevice;
    // Share again, so there is something to erase, and the same seat is reused.
    const link = await shareNote(adaClient, 'http://localhost:5173', groceries);
    await accept(bo, link.ticket, 'Bo');
    await until('the note to return to the reader’s device', async () => (await notesOn(shared)).length === 1);
    const before = await dump(boFactory, 'ordo-notes');
    expect(before.records).toBeGreaterThan(0);

    const readers = await readersOf(adaClient, groceries);
    await removeReader(adaClient, readers[0] as { member: string });
    // Closing a whole window is pushed to a feed already open on it, with no
    // reconnect and no nudge. The door then refuses the window, the manager
    // stops the feed, and the device erases what the window held.
    await until('the device to erase the closed window', async () => {
      const gone = asDeviceCopy(shared).revoked && (await notesOn(shared)).length === 0;
      return gone && (await dump(boFactory, 'ordo-notes')).records === 0;
    });
    // Reading the window is refused now, and a new one cannot be opened.
    await expect(windowInto(bo, l.ada.space)).rejects.toMatchObject({ failure: 'refused' });
    // The person's own notes are untouched.
    expect(titles(await notesOn(adaDevice))).toEqual(['Ideas', 'Trip', 'Groceries']);
  });

  it('lets a link read one note with no session, and refuses it after the owner revokes', async () => {
    const made = await makeReadLink(adaClient, 'http://localhost:5173', groceries);
    expect(made.url.startsWith('http://localhost:5173/#/link/')).toBe(true);
    const secret = linkSecret(made.url.slice('http://localhost:5173/'.length)) as string;
    expect(made.expiresAt).toBeGreaterThan(Math.floor(Date.now() / 1000));

    // The reader has no session and no token: the page's link route, as it runs.
    const endpoint = { baseUrl: l.url };
    const reading = await readViaLink(endpoint, secret);
    expect(reading.kind === 'notes' && titles(reading.notes)).toEqual(['Groceries']);
    // A client opened before the revoke, to see the door refuse it after.
    const { hooks: held } = await linkInto(endpoint, secret);
    expect(titles(await listNotes(held))).toEqual(['Groceries']);
    // A link is read-only.
    await expect(addNote(held, 'Written by a link', 'x')).rejects.toMatchObject({
      refusal: { class: 'access_denied' },
    });

    // The owner sees it live, and never the secret.
    const listed = await linksOf(adaClient, groceries);
    expect(listed).toHaveLength(1);
    expect(listed[0]).toMatchObject({ id: made.id, state: 'live' });
    expect(JSON.stringify(listed)).not.toContain(secret);

    await revokeReadLink(adaClient, listed[0] as ReadLink);
    expect((await linksOf(adaClient, groceries))[0]?.state).toBe('revoked');
    await expect(listNotes(held)).rejects.toMatchObject({ refusal: { class: 'access_denied' } });
    expect(await readViaLink(endpoint, secret)).toEqual({ kind: 'closed' });
    await expect(linkInto(endpoint, secret)).rejects.toMatchObject({ failure: 'refused' });
  });

  it('erases everything on sign-out', async () => {
    await adaDevice.purge();
    const d = await dump(adaFactory, 'ordo-notes');
    expect(d).toMatchObject({ records: 0, keys: 0 });
  });
});
