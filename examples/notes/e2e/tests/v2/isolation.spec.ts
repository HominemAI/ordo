import { readFile } from 'node:fs/promises';
import { expect, test as base, type APIResponse, type Page } from '@playwright/test';
import { owner, signInByEmail } from '../people.js';
import { addNote, applyProgram, idbRows, liveEvents, noteInList, signIn, signedIn, signOut, stack, unique } from '../support.js';

type Actor = Awaited<ReturnType<typeof owner>> & { headers: Record<string, string>; events: () => Promise<string[]> };
async function actor(name: string): Promise<Actor> {
  const person = await owner(unique(name));
  const headers: Record<string, string> = { Origin: stack().page };
  const events = await liveEvents(person.page);
  person.page.on('request', request => {
    const h = request.headers();
    if (new URL(request.url()).pathname.startsWith('/op/') && h.authorization && h['x-ordo-space']) {
      headers.Authorization = h.authorization;
      headers['X-Ordo-Space'] = h['x-ordo-space'];
    }
  });
  await person.page.goto(stack().pageV2);
  const passkey = person.page.getByRole('button', { name: 'Sign in with a passkey' });
  await expect(person.page.getByLabel('New note', { exact: true }).or(passkey)).toBeVisible();
  if (await passkey.isVisible()) await signIn(person.page);
  await signedIn(person.page);
  await expect.poll(() => headers.Authorization).toBeTruthy();
  return { ...person, headers, events };
}
const test = base.extend<{ ada: Actor; bo: Actor }>({
  ada: async ({}, use) => { const a = await actor('Ada'); try { await use(a); } finally { await a.close(); } },
  bo: async ({}, use) => { const b = await actor('Bo'); try { await use(b); } finally { await b.close(); } },
});
test.beforeAll(() => { test.setTimeout(300_000); applyProgram(stack().programV2); });

const stamp = () => new Date().toISOString().replace(/\.\d+Z$/, 'Z');
const unknownId = (id: string) => id.slice(0, id.indexOf('_') + 1) + '0'.repeat(id.length - id.indexOf('_') - 1);
async function read(a: Actor, op: string, params: Record<string, string> = {}, space = a.headers['X-Ordo-Space']) {
  return a.context.request.get(`${stack().door}/op/${op}`, { headers: { ...a.headers, 'X-Ordo-Space': space }, params });
}
async function write(a: Actor, op: string, data: object, space = a.headers['X-Ordo-Space']) {
  return a.context.request.post(`${stack().door}/op/${op}`, {
    headers: { ...a.headers, 'X-Ordo-Space': space, 'Idempotency-Key': unique('isolation-') }, data,
  });
}
async function rows(response: APIResponse): Promise<Record<string, any>[]> {
  expect(response.status(), await response.text()).toBe(200);
  return (await response.json()).rows;
}
async function note(a: Actor, title = unique('note-')) {
  await addNote(a.page, title);
  const found = (await rows(await read(a, 'allNotes'))).find(row => row.title === title);
  expect(found).toBeTruthy();
  return { id: found!.id as string, title };
}
async function sameAnswer(a: APIResponse, b: APIResponse) {
  expect(a.status()).toBe(b.status());
  // Keep the body byte comparison apart from the per-request identifier.
  const normalize = (body: string) => body.replace(/("(?:request_id|requestId)"\s*:\s*)"[^"]*"/g, '$1"<request>"');
  expect(normalize(await a.text())).toBe(normalize(await b.text()));
}
async function refused(response: APIResponse, status: number, code: string) {
  expect(response.status(), await response.text()).toBe(status);
  expect((await response.json()).error.code).toBe(code);
}
async function attachment(a: Actor, n: { id: string; title: string }) {
  const name = unique('private-') + '.txt';
  await noteInList(a.page, n.title).click();
  await a.page.getByLabel('Attach images or files').setInputFiles({ name, mimeType: 'text/plain', buffer: Buffer.from(name) });
  await expect(a.page.getByRole('button', { name: `Download ${name}` })).toBeVisible();
  const file = (await rows(await read(a, 'myFiles'))).find(row => row.name === name)!;
  expect(file.file).toBeTruthy();
  return { name, handle: file.file as string };
}

test('a note id from another tenant is not found, exactly as an id that does not exist', async ({ ada, bo }) => {
  const n = await note(ada);
  const foreign = await read(bo, 'oneNote', { id: n.id });
  await sameAnswer(foreign, await read(bo, 'oneNote', { id: unknownId(n.id) }));
  expect(await rows(foreign)).toEqual([]);
});

test("a refinement cannot reach another tenant's rows", async ({ ada, bo }) => {
  const n = await note(ada);
  for (const [field, value] of [['id', n.id], ['title', n.title]]) {
    expect(await rows(await read(bo, 'myNotes', { $refine: JSON.stringify({ where: [{ field, op: 'eq', value }] }) }))).toEqual([]);
  }
});

test("a write in one tenant never shows on another tenant's open feed", async ({ ada, bo }) => {
  await expect.poll(async () => (await ada.events()).join('\n')).toContain('load');
  const marker = await ada.page.evaluate(() => {
    const w = window as unknown as { __e2eMarker: string };
    return w.__e2eMarker = crypto.randomUUID();
  });
  const foreign = await note(bo, unique('bo-'));
  const sentinel = await note(ada, unique('ada-sentinel-'));
  await expect.poll(async () => (await ada.events()).join('\n')).toContain(sentinel.title);
  expect((await ada.events()).join('\n')).not.toContain(foreign.title);
  await expect(noteInList(ada.page, foreign.title)).toHaveCount(0);
  expect(await ada.page.evaluate(() => (window as unknown as { __e2eMarker: string }).__e2eMarker)).toBe(marker);
});

test("another tenant's file is not found, exactly as a file that does not exist", async ({ ada, bo }) => {
  const file = await attachment(ada, await note(ada));
  const fetch = (handle: string) => bo.context.request.get(`${stack().door}/blob/${handle}`, { headers: bo.headers });
  const foreign = await fetch(file.handle);
  await refused(foreign, 404, 'not_found');
  await sameAnswer(foreign, await fetch('0'.repeat(file.handle.length)));
});

test('a private file is never served without a token', async ({ ada, request }) => {
  const file = await attachment(ada, await note(ada));
  const fetch = (handle: string) => request.get(`${stack().door}/blob/${handle}`);
  const foreign = await fetch(file.handle);
  await refused(foreign, 403, 'access_denied');
  await sameAnswer(foreign, await fetch('0'.repeat(file.handle.length)));
});

function unzip(data: Buffer): Map<string, Buffer> {
  const entries = new Map<string, Buffer>();
  let at = 0;
  while (data.readUInt32LE(at) === 0x04034b50) {
    const size = data.readUInt32LE(at + 18), nameLength = data.readUInt16LE(at + 26), extra = data.readUInt16LE(at + 28);
    const name = data.toString('utf8', at + 30, at + 30 + nameLength);
    const start = at + 30 + nameLength + extra;
    entries.set(name, data.subarray(start, start + size));
    at = start + size;
  }
  return entries;
}

test("a person's own download holds only their tenant's rows", async ({ ada, bo }) => {
  const a = [await note(ada), await note(ada)];
  const b = [await note(bo), await note(bo)];
  const af = await attachment(ada, a[0]);
  const bf = await attachment(bo, b[0]);
  await bo.page.getByRole('button', { name: 'Close', exact: true }).click();
  const [download] = await Promise.all([
    bo.page.waitForEvent('download'), bo.page.getByRole('button', { name: 'Download all my notes' }).click(),
  ]);
  const entries = unzip(await readFile((await download.path())!));
  const notes = JSON.parse(entries.get('notes.json')!.toString());
  expect(notes.map((row: { title: string }) => row.title).sort()).toEqual(b.map(n => n.title).sort());
  const files = entries.get('attachments.json')!.toString();
  expect(files).toContain(bf.name);
  expect(files).not.toContain(af.name);
  const all = [...entries.values()].map(bytes => bytes.toString()).join('\n');
  for (const n of a) expect(all).not.toContain(n.title);
});

test("only the owner exports a tenant, and another tenant's export is refused", async ({ ada, bo }) => {
  const own = await write(ada, 'std.space.export', {});
  expect(own.status(), await own.text()).toBe(200);
  const foreign = await write(bo, 'std.space.export', {}, ada.headers['X-Ordo-Space']);
  await refused(foreign, 403, 'access_denied');
  await sameAnswer(foreign, await write(bo, 'std.space.export', {}, unique('space-')));
});

test('search never finds a note from another tenant', async ({ ada, bo }) => {
  const word = unique('zebra');
  const foreign = await note(ada, word);
  const own = await note(bo, `${word}-sentinel`);
  await bo.page.getByRole('button', { name: 'Search', exact: true }).click();
  await bo.page.getByLabel('Search your notes and notes shared with you').fill(word);
  const found = bo.page.getByRole('region', { name: 'Search', exact: true });
  await expect(found.getByText(own.title, { exact: true })).toBeVisible();
  await expect(found.getByRole('status')).toHaveText('1 note here matches.');
  await expect(found.getByText(foreign.title, { exact: true })).toHaveCount(0);
});

test("a write cannot point at another tenant's row", async ({ ada, bo }) => {
  const n = await note(ada);
  const result = await write(bo, 'addAttachment', { note: n.id, name: unique('intruder'), kind: 'text/plain', size: 1, created: stamp() });
  expect(result.status(), await result.text()).toBe(404);
  expect((await result.json()).error.code).toBe('not_found');
  expect(await rows(await read(ada, 'myFiles'))).toEqual([]);
});

test('the space header naming another tenant is refused like an unknown op', async ({ ada, bo }) => {
  const foreign = await read(bo, 'myNotes', {}, ada.headers['X-Ordo-Space']);
  await refused(foreign, 403, 'access_denied');
  await sameAnswer(foreign, await read(bo, 'noSuchOp'));
});

test('signing out erases the device copy, and the next person in the same browser sees none of it', async ({ ada }) => {
  const notes = [await note(ada), await note(ada), await note(ada)];
  for (const n of notes) await expect.poll(async () => JSON.stringify(await idbRows(ada.page, 'ordo-notes'))).toContain(n.title);
  await signOut(ada.page);
  await expect.poll(() => idbRows(ada.page, 'ordo-notes')).toEqual([]);
  await signInByEmail(ada.page, `${unique('bo')}@example.org`, 0);
  const sentinel = unique('bo-own-');
  await addNote(ada.page, sentinel);
  await expect.poll(async () => JSON.stringify(await idbRows(ada.page, 'ordo-notes'))).toContain(sentinel);
  for (const n of notes) {
    await expect(noteInList(ada.page, n.title)).toHaveCount(0);
    expect(JSON.stringify(await idbRows(ada.page, 'ordo-notes'))).not.toContain(n.title);
  }
});
