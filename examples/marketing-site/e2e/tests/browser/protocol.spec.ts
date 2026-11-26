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
