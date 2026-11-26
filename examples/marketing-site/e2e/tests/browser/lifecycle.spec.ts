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
    headers, data: { name: `Lifecycle ${randomUUID()}`, owner: principal },
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
    await test.step('active tenants cannot be reaped', async () => {
      await refused(await request.post(`${doors[0]}/op/std.space.reap`, { headers, data: { tenant } }), 400, 'tenant_refused');
      expect((await rows(await read(doors[0])))[0]).toMatchObject({ title });
    });
    await test.step('the last owner cannot leave', async () => {
      await refused(await request.post(`${doors[0]}/op/std.space.leave`, { headers: scoped, data: {} }), 400, 'tenant_refused');
      expect((await rows(await read(doors[0])))[0]).toMatchObject({ title });
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
