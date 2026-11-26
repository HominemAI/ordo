import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { expect } from '@playwright/test';
import { test } from '../owner-session.js';
import { stack, state, rows, refused } from '../support.js';

// Owner authority comes from an actual WebAuthn ceremony, not a minted test token.
// These requests use supported application operations against production hosts.
test('an owner creates a tenant, pauses it, preserves data and resumes both replicas', async ({ ownerPage, request }) => {
  const saved = await ownerPage.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const headers = { Origin: stack().page, Authorization: `Bearer ${saved.token}`, 'X-Ordo-Space': 'system' };
  const principal = readFileSync(`${state}/browser-owner-principal`, 'utf8').trim();
  const created = (await rows(await request.post(`${stack().door}/op/std.space.create`, {
    headers: { ...headers, 'Idempotency-Key': randomUUID() }, data: { name: `Lifecycle ${randomUUID()}`, owner: principal },
  })))[0];
  const tenant = created.space;
  expect(typeof tenant).toBe('string');
  expect(tenant).not.toBe('system');
  const scoped = { ...headers, 'X-Ordo-Space': tenant };
  const doors = [stack().door, stack().doorB];
  const title = `Preserved ${randomUUID()}`;
  const campaign = (await rows(await request.post(`${doors[0]}/op/createCampaign`, {
    headers: scoped, data: { title, brief: 'Kept across pause', tags: [], budget: '12.34', due: null,
      created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z') },
  })))[0].created;
  const read = (door: string) => request.get(`${door}/op/oneCampaign`, { headers: scoped, params: { id: campaign } });
  try {
    await test.step('tenant session policy changes are read back and malformed changes preserve it', async () => {
      const result = await rows(await request.post(`${doors[0]}/op/std.space.setSessionPolicy`, {
        headers: scoped, data: { tokenSecs: '120' },
      }));
      expect(result[0]).toMatchObject({ tokenSecs: 120 });
      await refused(await request.post(`${doors[0]}/op/std.space.setSessionPolicy`, {
        headers: scoped, data: { tokenSecs: 'not-a-duration' },
      }), 400, 'tenant_refused');
      for (const door of doors) {
        const current = await rows(await request.get(`${door}/op/std.space.sessionPolicy`, { headers: scoped }));
        expect(current[0]).toMatchObject({ tokenSecs: 120 });
      }
      await rows(await request.post(`${doors[0]}/op/std.space.setSessionPolicy`, { headers: scoped, data: { tokenSecs: 'default' } }));
    });
    await test.step('active tenants cannot be reaped', async () => {
      await refused(await request.post(`${doors[0]}/op/std.space.reap`, { headers, data: { tenant } }), 400, 'tenant_refused');
      expect((await rows(await read(doors[0])))[0]).toMatchObject({ title });
    });
    await test.step('the last owner cannot leave', async () => {
      await refused(await request.post(`${doors[0]}/op/std.space.leave`, { headers: scoped, data: {} }), 400, 'tenant_refused');
      expect((await rows(await read(doors[0])))[0]).toMatchObject({ title });
    });
    await test.step('a colleague leaves while the owner and acknowledged data remain', async () => {
      const joined = await request.post(`${doors[0]}/auth/complete`, {
        headers: { Origin: stack().page }, data: { method: 'guest', kind: 'new', display: `Leaving ${randomUUID()}` },
      });
      expect(joined.status()).toBe(200);
      const token = (await joined.json()).token as string;
      const colleague = Buffer.from(token.split('.')[0], 'base64url').toString().split('|')[1];
      const member = (await rows(await request.post(`${doors[0]}/op/std.space.admit`, {
        headers: scoped, data: { principal: colleague, role: 'viewer' },
      })))[0].member;
      const colleagueHeaders = { ...scoped, Authorization: `Bearer ${token}` };
      const me = (door: string) => request.get(`${door}/op/me`, { headers: colleagueHeaders });
      for (const door of doors) await expect.poll(async () => (await me(door)).status()).toBe(200);
      const left = await rows(await request.post(`${doors[0]}/op/std.space.leave`, { headers: colleagueHeaders, data: {} }));
      expect(left[0]).toMatchObject({ member, status: 'left' });
      for (const door of doors) {
        await expect.poll(async () => (await me(door)).status()).toBe(403);
        await refused(await me(door), 403, 'access_denied');
        expect((await rows(await read(door)))[0]).toMatchObject({ title });
      }
    });
    await rows(await request.post(`${doors[0]}/op/std.deployment.pauseTenant`, { headers, data: { tenant, note: 'billing' } }));
    for (const door of doors) {
      await expect.poll(async () => (await read(door)).status()).toBe(423);
      await refused(await read(door), 423, 'space_paused');
      await refused(await request.post(`${door}/op/editCampaign`, { headers: scoped, data: {
        id: campaign, rev: 1, title: 'Must not land', brief: '', tags: [], budget: '0.00', due: null, status: 'draft',
      } }), 423, 'space_paused');
      // The unaffected home space remains available during the tenant pause.
      await rows(await request.get(`${door}/op/campaignList`, { headers }));
    }
    await refused(await request.post(`${doors[0]}/op/std.space.reap`, { headers, data: { tenant } }), 400, 'tenant_refused');
    await rows(await request.post(`${doors[1]}/op/std.deployment.resumeTenant`, { headers, data: { tenant } }));
    for (const door of doors) {
      await expect.poll(async () => (await read(door)).status()).toBe(200);
      expect((await rows(await read(door)))[0]).toMatchObject({ title, brief: 'Kept across pause' });
    }
    await test.step('wrong deletion confirmation preserves the tenant', async () => {
      await refused(await request.delete(`${doors[0]}/op/std.space.delete`, { headers: scoped, params: { confirm: 'wrong' } }), 400, 'not_confirmed');
      expect((await rows(await read(doors[0])))[0]).toMatchObject({ title });
    });
  } finally {
    // Only the disposable tenant minted by this test can be erased.
    await rows(await request.delete(`${doors[0]}/op/std.space.delete`, { headers: scoped, params: { confirm: tenant } }));
  }
  for (const door of doors) {
    await expect.poll(async () => (await read(door)).status()).toBe(403);
    await refused(await read(door), 403, 'access_denied');
    await rows(await request.get(`${door}/op/campaignList`, { headers }));
  }
});

test('secret replacement increments its version and never reveals the value', async ({ ownerPage, request }) => {
  const saved = await ownerPage.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const headers = { Origin: stack().page, Authorization: `Bearer ${saved.token}`, 'X-Ordo-Space': 'system' };
  const name = `journey${randomUUID().replaceAll('-', '')}`;
  const values = [`private-first-${randomUUID()}`, `private-replacement-${randomUUID()}`];
  let installed = false;
  try {
    for (let index = 0; index < values.length; index++) {
      const response = await request.post(`${stack().door}/op/std.deployment.setSecret`, { headers, data: { name, value: values[index] } });
      const result = await rows(response);
      installed = true;
      expect(result[0]).toMatchObject({ name, version: index + 1 });
      for (const value of values) expect(await response.text()).not.toContain(value);
      for (const door of [stack().door, stack().doorB]) {
        const listing = await request.get(`${door}/op/std.deployment.secrets`, { headers });
        expect((await rows(listing)).find(row => row.name === name)).toMatchObject({ version: index + 1 });
        for (const value of values) expect(await listing.text()).not.toContain(value);
        await refused(await request.get(`${door}/op/std.deployment.secrets`), 403, 'access_denied');
      }
    }
  } finally {
    if (installed) await rows(await request.post(`${stack().door}/op/std.deployment.removeSecret`, { headers, data: { name } }));
  }
  await refused(await request.post(`${stack().door}/op/std.deployment.removeSecret`, { headers, data: { name } }), 404, 'not_found');
  for (const door of [stack().door, stack().doorB]) {
    expect((await rows(await request.get(`${door}/op/std.deployment.secrets`, { headers }))).some(row => row.name === name)).toBe(false);
  }
});

test('operational reads disclose data to the owner and refuse anonymous callers', async ({ ownerPage, request }) => {
  const saved = await ownerPage.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const headers = { Origin: stack().page, Authorization: `Bearer ${saved.token}`, 'X-Ordo-Space': 'system' };
  const now = Math.floor(Date.now() / 1000);
  for (const door of [stack().door, stack().doorB]) {
    for (const path of ['/health/live', '/health/ready']) {
      const response = await request.get(`${door}${path}`);
      expect(response.status()).toBe(200);
    }
    for (const path of ['/health/deep', '/metrics', '/security/alerts',
      `/usage?space=system&from=${now - 3600}&to=${now + 1}`,
      '/security/events?space=system&from=0&limit=1']) {
      const response = await request.get(`${door}${path}`, { headers });
      expect(response.status(), `${path}: ${await response.text()}`).toBe(200);
      await refused(await request.get(`${door}${path}`), 403, 'access_denied');
    }
    for (const op of ['std.deployment.observedCost', 'std.deployment.quotas', 'std.deployment.securityEvents']) {
      const response = await request.get(`${door}/op/${op}`, { headers });
      expect(Array.isArray(await rows(response))).toBe(true);
      await refused(await request.get(`${door}/op/${op}`), 403, 'access_denied');
    }
    await refused(await request.get(`${door}/usage`, { headers }), 400, 'invalid_param');
    await refused(await request.get(`${door}/security/events`, { headers, params: { space: 'system', limit: '-1' } }), 400, 'invalid_param');
  }
});


test('live quota changes warn, refuse, recover and preserve tenant isolation on both replicas', async ({ ownerPage, request }) => {
  const saved = await ownerPage.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const headers = { Origin: stack().page, Authorization: `Bearer ${saved.token}`, 'X-Ordo-Space': 'system' };
  const principal = readFileSync(`${state}/browser-owner-principal`, 'utf8').trim();
  const tenant = (await rows(await request.post(`${stack().door}/op/std.space.create`, {
    headers: { ...headers, 'Idempotency-Key': randomUUID() }, data: { name: `Quota ${randomUUID()}`, owner: principal },
  })))[0].space;
  const scoped = { ...headers, 'X-Ordo-Space': tenant };
  const doors = [stack().door, stack().doorB];
  const set = (limit: number, grace: number, extra: Record<string, unknown> = {}) => request.post(`${doors[0]}/op/std.deployment.setQuota`, {
    headers, data: { space: tenant, quantity: 'requests', limit, grace, periodSecs: 86400, ...extra },
  });
  const read = (door: string) => request.get(`${door}/op/campaignList`, { headers: scoped });
  try {
    const campaign = (await rows(await request.post(`${doors[0]}/op/createCampaign`, { headers: scoped, data: {
      title: 'Retained through quota refusal', brief: 'Acknowledged before the limit', tags: [], budget: '2.00', due: null,
      created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    } })))[0].created;
    // Above the limit but within its grace still serves. Observe durable use,
    // not only a setter response, before testing the warning branch.
    expect((await rows(await set(0, 100000)))[0]).toMatchObject({ enforced: true, limit: 0, grace: 100000 });
    for (const door of doors) await rows(await read(door));
    await expect.poll(async () => (await rows(await request.get(`${doors[0]}/op/std.deployment.quotas`, {
      headers, params: { space: tenant },
    })))[0].used, { timeout: 30_000 }).toBeGreaterThan(0);
    for (const door of doors) expect((await rows(await read(door)))[0]).toMatchObject({ id: campaign });
    for (const extra of [{ quantity: 'writes' }, { periodSecs: 0 }, { limit: -1 }]) {
      await refused(await set(0, 100000, extra), 400, 'tenant_refused');
    }
    expect((await rows(await request.get(`${doors[1]}/op/std.deployment.quotas`, { headers, params: { space: tenant } })))[0])
      .toMatchObject({ limit: 0, grace: 100000, periodSecs: 86400 });
    await rows(await set(0, 0));
    for (const door of doors) {
      await expect.poll(async () => (await read(door)).status(), { timeout: 30_000 }).toBe(429);
      await refused(await read(door), 429, 'quota_exceeded');
      await refused(await request.get(`${door}/op/campaignList`, { headers: { 'X-Ordo-Space': tenant } }), 403, 'access_denied');
      await rows(await request.get(`${door}/op/campaignList`, { headers }));
    }
    await rows(await set(100000, 0));
    for (const door of doors) {
      await expect.poll(async () => (await read(door)).status(), { timeout: 30_000 }).toBe(200);
      expect((await rows(await read(door)))[0]).toMatchObject({ id: campaign, title: 'Retained through quota refusal' });
    }
  } finally {
    // Lift the test limit before tenant deletion; never alter another tenant.
    await rows(await set(100000, 0));
    for (const door of doors) await expect.poll(async () => (await read(door)).status(), { timeout: 30_000 }).toBe(200);
    await rows(await request.delete(`${doors[0]}/op/std.space.delete`, { headers: scoped, params: { confirm: tenant } }));
  }
});
