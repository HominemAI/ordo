import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { expect, test, type APIRequestContext, type APIResponse } from '@playwright/test';
const unique = (word: string) => `${word}-${randomUUID()}`;
const stamp = () => new Date().toISOString().replace(/\.\d+Z$/, 'Z');
const stack = (): { door: string; page: string; mail: string } => JSON.parse(readFileSync(
  `${process.env['E2E_STATE'] ?? fileURLToPath(new URL('../../.stack', import.meta.url))}/stack.json`, 'utf8',
));
interface Person { token: string; space: string }
const headers = (who: Person) => ({ Origin: stack().page, Authorization: `Bearer ${who.token}`, 'X-Ordo-Space': who.space });
async function read(request: APIRequestContext, who: Person, op: string, params: Record<string, string> = {}) {
  return request.get(`${stack().door}/op/${op}`, { headers: headers(who), params });
}
async function rows(response: APIResponse): Promise<Record<string, any>[]> {
  expect(response.status(), await response.text()).toBe(200);
  return (await response.json()).rows;
}
async function write(request: APIRequestContext, who: Person, op: string, data: object) {
  return rows(await request.post(`${stack().door}/op/${op}`, { headers: { ...headers(who), 'Idempotency-Key': unique('write') }, data }));
}
async function founder(request: APIRequestContext, name: string): Promise<Person> {
  const address = `${unique(name)}@example.org`;
  const auth = (path: string, data: object) => request.post(`${stack().door}/auth/${path}`, { headers: { Origin: stack().page }, data });
  const begun = await auth('begin', { method: 'email', identifier: address });
  expect(begun.status(), await begun.text()).toBe(200);
  const { challenge, binder } = await begun.json();
  let code = '';
  await expect.poll(async () => {
    const response = await request.get(`${stack().mail}/messages`, { params: { to: address } });
    const mail = await response.json();
    code = /code is (\d+)/.exec(mail[0]?.data ?? '')?.[1] ?? '';
    return code;
  }).not.toBe('');
  const response = await auth('complete', { method: 'email', kind: 'email_code', challenge, binder, code });
  expect(response.status(), await response.text()).toBe(200);
  const done = await response.json();
  const who = { token: done.token as string, space: (done.space ?? '') as string };
  if (!who.space) {
    await expect.poll(async () => {
      const response = await request.get(`${stack().door}/auth/provisioning`, { headers: { Origin: stack().page, Authorization: `Bearer ${who.token}` } });
      const status = await response.json();
      expect(status.state, JSON.stringify(status)).not.toBe('failed');
      who.space = status.space ?? '';
      return status.state;
    }, { timeout: 120_000 }).toBe('ready');
  }
  await expect.poll(async () => (await read(request, who, 'myProjects')).status()).toBe(200);
  return who;
}
async function project(request: APIRequestContext, who: Person) {
  const created = await write(request, who, 'std.projects.create', { name: unique('project'), summary: '', added: stamp() });
  const id = created[0].created as string;
  expect(id).toBeTruthy();
  const me = (await rows(await read(request, who, 'me')))[0].id;
  await write(request, who, 'std.team.create', { project: id, member: me });
  await write(request, who, 'createTask', { project: id, title: unique('task'), added: stamp() });
  expect(await rows(await read(request, who, 'board', { project: id }))).toHaveLength(1);
  return id;
}
async function identicalRefusal(a: APIResponse, b: APIResponse) {
  expect(a.status(), await a.text()).toBe(403);
  expect(b.status(), await b.text()).toBe(403);
  expect((await a.json()).error.code).toBe('access_denied');
  expect((await b.json()).error.code).toBe('access_denied');
  const normalize = (text: string) => text.replace(/("(?:request_id|requestId)"\s*:\s*)"[^"]*"/g, '$1"<request>"');
  expect(normalize(await a.text())).toBe(normalize(await b.text()));
}

test("a founder cannot read another team's board, team or report", async ({ request }) => {
  const fay = await founder(request, 'fay'), gus = await founder(request, 'gus');
  expect(fay.space).not.toBe(gus.space);
  const id = await project(request, fay);
  for (const op of ['board', 'projectTeam', 'reportByProject']) {
    const params: Record<string, string> = op === 'reportByProject'
      ? { today: stamp().slice(0, 10), $refine: JSON.stringify({ where: [{ field: 'id', op: 'eq', value: id }] }) }
      : { project: id };
    expect(await rows(await read(request, gus, op, params))).toEqual([]);
    await identicalRefusal(await read(request, { ...gus, space: fay.space }, op, params), await read(request, gus, 'noSuchOp'));
  }
});

test('an undeclared op and a forbidden op look the same', async ({ request }) => {
  const fay = await founder(request, 'fay'), gus = await founder(request, 'gus');
  await project(request, fay);
  for (const token of ['', gus.token]) {
    const h = { Origin: stack().page, 'X-Ordo-Space': fay.space, ...(token ? { Authorization: `Bearer ${token}` } : {}) };
    const fetch = (op: string) => request.get(`${stack().door}/op/${op}`, { headers: h, params: { today: stamp().slice(0, 10) } });
    await identicalRefusal(await fetch('reportByProject'), await fetch('noSuchOp'));
  }
});
