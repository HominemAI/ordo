import { randomUUID } from 'node:crypto';
import { test, expect, type APIRequestContext } from '@playwright/test';
import { stack, owner, rows, refused, sameRefusal, normalize } from '../support.js';
const unique = (word: string) => `${word}-${randomUUID()}`;
const contact = () => ({ name: unique('Visitor'), email: `${randomUUID()}@example.org`, company: 'Public test', message: unique('Original message') });
const read = (request: APIRequestContext, op: string) => request.get(`${stack().door}/op/${op}`);

test('a visitor reads the catalogue, pricing and reviews and never a private field', async ({ request }) => {
  for (const [op, count, first] of [
    ['catalogue', 10, { slug: 'aurora-lamp', name: 'Aurora Lamp' }],
    ['pricing', 3, { name: 'Starter' }],
    ['reviews', 8, { author: 'Mara Ellison' }],
    ['product?slug=aurora-lamp', 1, { name: 'Aurora Lamp' }],
  ] as const) {
    const response = await read(request, op);
    const data = await rows(response);
    expect(data).toHaveLength(count);
    expect(data[0]).toMatchObject(first);
    expect(await response.text()).not.toMatch(/internalNote|INTERNAL-/);
  }
});

test('a visitor cannot call a private op, and it looks like an op that does not exist', async ({ request }) => {
  await sameRefusal(await read(request, 'enquiryList'), await read(request, 'noSuchOp'));
});

test('a visitor cannot refine a public read', async ({ request }) => {
  const answers = [];
  for (const field of ['name', 'internalNote']) {
    const response = await request.get(`${stack().door}/op/catalogue`, {
      params: { $refine: JSON.stringify({ where: [{ field, op: 'eq', value: 'INTERNAL-aurora-lamp' }] }) },
    });
    await refused(response, 400, 'invalid_refinement');
    answers.push(normalize(await response.text()));
  }
  expect(answers[1]).toBe(answers[0]);
});

test('a visitor with no token cannot open a live stream', async ({ request }) => {
  // APIRequestContext waits for the body: an accidental SSE stream times out.
  const response = await request.get(`${stack().door}/op/catalogue?live=1`, { timeout: 10_000 });
  await refused(response, 400, 'not_live');
  expect(response.headers()['content-type']).not.toContain('text/event-stream');
});

test('a visitor cannot name a tenant space', async ({ request }, testInfo) => {
  const plain = await read(request, 'catalogue');
  const responses = [];
  for (const space of ['system', unique('not-a-space')]) {
    const headers = { 'X-Ordo-Space': space };
    const response = await request.get(`${stack().door}/op/catalogue`, { headers });
    responses.push(response);
    if (response.status() === 200) expect(await rows(response)).toEqual(await rows(plain));
    else await refused(response, 403, 'access_denied');
    await sameRefusal(await request.get(`${stack().door}/op/enquiryList`, { headers }),
      await request.get(`${stack().door}/op/noSuchOp`, { headers }));
  }
  expect(responses[1].status()).toBe(responses[0].status());
  expect(normalize(await responses[1].text())).toBe(normalize(await responses[0].text()));
  testInfo.annotations.push({ type: 'space-header', description: responses[0].status() === 200 ? 'Served as the plain public read' : 'Both refused' });
});

test("a visitor's contact write creates a row and can change nothing", async ({ request }) => {
  const data = contact();
  const headers = await owner(request);
  const created = await rows(await request.post(`${stack().door}/op/contact`, { data }));
  expect(created).toHaveLength(1);
  const id = created[0].created;
  expect(id).toEqual(expect.any(String));
  expect(id.length).toBeGreaterThan(0);
  const response = await request.post(`${stack().door}/op/contact`, { data: { ...data, id, message: 'Overwritten' } });
  await refused(response, 400, 'unknown_param');
  const enquiries = await rows(await request.get(`${stack().door}/op/enquiryList`, { headers }));
  expect(enquiries.filter(row => row.email === data.email)).toEqual([{ ...data, id }]);
  await sameRefusal(await request.post(`${stack().door}/op/addProduct`, { data: {} }),
    await request.post(`${stack().door}/op/noSuchOp`, { data: {} }));
});

test('a signed-in owner calling a public op gets the ordinary path', async ({ request }) => {
  const response = await request.get(`${stack().door}/op/catalogue`, { headers: await owner(request) });
  expect(response.headers()['x-ordo-cache']).toBeUndefined();
  expect(await rows(response)).toEqual(await rows(await read(request, 'catalogue')));
});

// One worker, no retries, and a unique product argument keep counter deltas
// isolated. This suite deliberately does not consume the 1000-write quota.
test('public requests are counted, and an unknown op mints no series', async ({ request }) => {
  const headers = await owner(request);
  const metrics = async () => {
    const response = await request.get(`${stack().door}/metrics`, { headers });
    expect(response.status(), await response.text()).toBe(200);
    return response.text();
  };
  const count = (text: string, op: string, kind: string, outcome: string) => {
    const line = text.split('\n').find(line => line.startsWith('ordo_public_requests_total{') &&
      line.includes(`op="${op}"`) && line.includes(`kind="${kind}"`) && line.includes(`outcome="${outcome}"`));
    return line ? Number(line.slice(line.lastIndexOf(' ') + 1)) : 0;
  };
  // The catalogue was warmed by earlier tests. Check a hit on it, and a
  // guaranteed miss + hit on one fresh argument of the public product op.
  await rows(await read(request, 'catalogue'));
  const before = await metrics();
  await rows(await read(request, 'catalogue'));
  const path = `product?slug=${unique('not-seeded')}`;
  expect(await rows(await read(request, path))).toEqual([]);
  expect(await rows(await read(request, path))).toEqual([]);
  await rows(await request.post(`${stack().door}/op/contact`, { data: contact() }));
  const prefix = unique('probe');
  for (let i = 0; i < 20; i++) await refused(await read(request, `${prefix}-${i}`), 403, 'access_denied');
  const after = await metrics();
  for (const [op, kind, outcome] of [['catalogue', 'read', 'hit'], ['product', 'read', 'miss'], ['product', 'read', 'hit'], ['contact', 'write', 'written']]) {
    expect(count(after, op, kind, outcome) - count(before, op, kind, outcome), `${op} ${outcome}`).toBe(1);
  }
  expect(after.split('\n').filter(line => line.startsWith('ordo_public_requests_total{')).join('\n')).not.toContain(prefix);
});
