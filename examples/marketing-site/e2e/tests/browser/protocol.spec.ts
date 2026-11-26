import { createServer } from 'node:http';
import { expect } from '@playwright/test';
import { test } from '../owner-session.js';
import { stack, rows, refused } from '../support.js';

// The second origin is an actual HTTP page, not a mocked Ordo response.
test('the browser permits an allowed origin and blocks private calls from an unlisted origin', async ({ ownerPage, context }) => {
  const saved = await ownerPage.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const door = stack().door;
  const headers = { Authorization: `Bearer ${saved.token}`, 'X-Ordo-Space': 'system' };
  const allowed = await ownerPage.evaluate(async ({ door, headers }) => {
    const r = await fetch(`${door}/op/campaignList`, { headers, credentials: 'omit' });
    return { status: r.status, body: await r.json() };
  }, { door, headers });
  expect(allowed.status).toBe(200);
  expect(Array.isArray(allowed.body.rows)).toBe(true);
  const server = createServer((_req, res) => { res.setHeader('Content-Type', 'text/html'); res.end('<!doctype html><title>Unlisted origin</title><h1>External page</h1>'); });
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('No external page port');
  const page = await context.newPage();
  try {
    await page.goto(`http://127.0.0.1:${address.port}`);
    const result = await page.evaluate(async ({ door, headers }) => {
      const publicResponse = await fetch(`${door}/op/catalogue`, { credentials: 'omit' });
      let privateBlocked = false;
      try { await fetch(`${door}/op/campaignList`, { headers, credentials: 'omit' }); }
      catch (error) { privateBlocked = error instanceof TypeError; }
      return { publicStatus: publicResponse.status, privateBlocked };
    }, { door, headers });
    expect(result).toEqual({ publicStatus: 200, privateBlocked: true });
    // Credentialed fetch of a cookie-free op must also fail: only auth paths
    // opt into Access-Control-Allow-Credentials, even on an allowed origin.
    expect(await ownerPage.evaluate(async ({ door, headers }) => {
      try { await fetch(`${door}/op/campaignList`, { headers, credentials: 'include' }); return false; }
      catch (error) { return error instanceof TypeError; }
    }, { door, headers })).toBe(true);
  } finally {
    await page.close();
    await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});

test('concurrent bearer renewal keeps one session and logout revokes every renewed token', async ({ ownerPage, request }) => {
  const saved = await ownerPage.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const door = stack().door;
  const renewed = await ownerPage.evaluate(async ({ door, token }) => Promise.all([0, 1, 2, 3].map(async () => {
    const r = await fetch(`${door}/auth/refresh`, { method: 'POST', headers: { Authorization: `Bearer ${token}` }, credentials: 'omit' });
    return { status: r.status, body: await r.json() };
  })), { door, token: saved.token });
  for (const result of renewed) {
    expect(result.status).toBe(200);
    expect(typeof result.body.token).toBe('string');
    await rows(await request.get(`${door}/op/campaignList`, {
      headers: { Authorization: `Bearer ${result.body.token}`, 'X-Ordo-Space': 'system' },
    }));
  }
  await ownerPage.getByRole('button', { name: 'Sign out', exact: true }).click();
  await expect(ownerPage.getByRole('button', { name: 'Sign in with a passkey' })).toBeVisible();
  for (const token of [saved.token, ...renewed.map(r => r.body.token)]) {
    for (const host of [door, stack().doorB]) {
      const headers = { Authorization: `Bearer ${token}`, 'X-Ordo-Space': 'system' };
      await expect.poll(async () => (await request.get(`${host}/op/campaignList`, { headers })).status()).toBe(403);
      await refused(await request.get(`${host}/op/campaignList`, { headers }), 403, 'access_denied');
    }
  }
});

test('renewal cookies are scoped, reject missing origin and revoke the family after replay grace', async ({ ownerPage, context, request }) => {
  const door = stack().door;
  const cookie = (await context.cookies(`${door}/auth/refresh`)).find(c => c.name === 'ordo_renew');
  expect(cookie).toBeDefined();
  expect(cookie).toMatchObject({ path: '/auth/refresh', httpOnly: true, sameSite: 'Strict' });
  expect((await context.cookies(`${door}/op/campaignList`)).some(c => c.name === 'ordo_renew')).toBe(false);
  expect(await ownerPage.evaluate(() => document.cookie.includes('ordo_renew'))).toBe(false);
  const oldCookie = `${cookie!.name}=${cookie!.value}`;
  for (const origin of [undefined, 'https://unlisted.example']) {
    const headers: Record<string, string> = { Cookie: oldCookie };
    if (origin) headers.Origin = origin;
    const response = await request.post(`${door}/auth/refresh`, { headers });
    expect(response.status()).toBe(403);
    expect((await response.json()).error.code).toBe('remember_off');
    expect(response.headers()['set-cookie']).toBeUndefined();
  }
  const renewed = await ownerPage.evaluate(async door => {
    const response = await fetch(`${door}/auth/refresh`, { method: 'POST', credentials: 'include' });
    return { status: response.status, body: await response.json() };
  }, door);
  expect(renewed.status).toBe(200);
  expect((await context.cookies(`${door}/auth/refresh`)).find(c => c.name === 'ordo_renew')!.value).not.toBe(cookie!.value);
  // Two tabs have 10 seconds of grace. Poll the old cookie until the real
  // clock ends that grace; no SQL clock changes or fabricated server errors.
  await expect.poll(async () => (await request.post(`${door}/auth/refresh`, {
    headers: { Origin: stack().page, Cookie: oldCookie },
  })).status(), { timeout: 20_000, intervals: [1000] }).toBe(401);
  for (const host of [door, stack().doorB]) {
    const headers = { Authorization: `Bearer ${renewed.body.token}`, 'X-Ordo-Space': 'system' };
    await expect.poll(async () => (await request.get(`${host}/op/campaignList`, { headers })).status()).toBe(403);
    await refused(await request.get(`${host}/op/campaignList`, { headers }), 403, 'access_denied');
  }
});
