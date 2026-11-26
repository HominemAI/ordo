import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { expect } from '@playwright/test';
import { test } from '../owner-session.js';
import { stack, state, rows, refused } from '../support.js';

test('a tenant owner adds, rotates and retires a sign-in connection without exposing its secret', async ({ ownerPage, request }) => {
  const saved = await ownerPage.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const admin = { Origin: stack().page, Authorization: `Bearer ${saved.token}`, 'X-Ordo-Space': 'system' };
  const principal = readFileSync(`${state}/browser-owner-principal`, 'utf8').trim();
  const tenant = (await rows(await request.post(`${stack().door}/op/std.space.create`, {
    headers: { ...admin, 'Idempotency-Key': randomUUID() }, data: { name: `Identity ${randomUUID()}`, owner: principal },
  })))[0].space as string;
  const headers = { ...admin, 'X-Ordo-Space': tenant };
  const name = `sso-${randomUUID().slice(0, 8)}`;
  const first = `first-${randomUUID()}`;
  const replacement = `replacement-${randomUUID()}`;
  // This journey validates stored connection lifecycle. Successful federation
  // needs the separate real-provider profile; no sign-in is claimed here.
  const params = { name, kind: 'oidc', issuer: 'https://identity.example.invalid', clientId: 'studio', clientSecret: first, role: 'viewer' };
  try {
    await ownerPage.reload();
    await ownerPage.getByRole('combobox', { name: 'Workspace', exact: true }).selectOption(tenant);
    await ownerPage.getByRole('button', { name: 'Open workspace' }).click();
    await ownerPage.getByRole('button', { name: 'Team', exact: true }).click();
    await ownerPage.getByText('Sign-in providers', { exact: true }).click();
    await ownerPage.getByLabel('Connection name (required)').fill(name);
    await ownerPage.getByLabel('Issuer URL (required)').fill(params.issuer);
    await ownerPage.getByLabel('Client ID (required)').fill(params.clientId);
    await ownerPage.getByLabel('Client secret (required)').fill(first);
    const [created] = await Promise.all([
      ownerPage.waitForResponse(r => r.url().endsWith('/op/std.space.addConnection') && r.status() === 200),
      ownerPage.getByRole('button', { name: 'Add sign-in provider', exact: true }).click(),
    ]);
    const originalKey = (await created.json()).rows[0].key;
    const item = ownerPage.getByRole('list', { name: 'Sign-in connections' }).getByRole('listitem').filter({ hasText: name });
    await expect(item).toContainText('active');
    expect(await created.text()).not.toContain(first);
    await expect(ownerPage.getByLabel('Client secret (required)')).toHaveValue('');
    await refused(await request.post(`${stack().door}/op/std.space.addConnection`, { headers, data: params }), 400, 'tenant_refused');
    await ownerPage.getByLabel(`New secret for ${name}`).fill(replacement);
    const [rotated] = await Promise.all([
      ownerPage.waitForResponse(r => r.url().endsWith('/op/std.space.updateConnection') && r.status() === 200),
      ownerPage.getByRole('button', { name: `Save new secret for ${name}` }).click(),
    ]);
    expect(await rotated.text()).not.toContain(replacement);
    await expect(ownerPage.getByLabel(`New secret for ${name}`)).toHaveValue('');
    await refused(await request.post(`${stack().door}/op/std.space.updateConnection`, {
      headers, data: { name, issuer: 'https://another.example.invalid' },
    }), 400, 'tenant_refused');
    for (const door of [stack().door, stack().doorB]) {
      const listing = await request.get(`${door}/op/std.space.connections`, { headers });
      expect((await rows(listing)).find(row => row.name === name)).toMatchObject({ key: originalKey, state: 'active', issuer: params.issuer });
      for (const secret of [first, replacement]) expect(await listing.text()).not.toContain(secret);
      await refused(await request.get(`${door}/op/std.space.connections`, { headers: { 'X-Ordo-Space': tenant } }), 403, 'access_denied');
      const foreign = await request.post(`${door}/auth/begin`, {
        headers: { Origin: stack().page }, data: { method: name, tenant: 'system', return_to: `${stack().page}/studio` },
      });
      expect(foreign.status()).toBe(401);
      expect((await foreign.json()).error.code).toBe('authentication_failed');
    }
    await ownerPage.getByRole('button', { name: `Retire ${name}`, exact: true }).click();
    await ownerPage.getByRole('button', { name: `Confirm retire ${name}`, exact: true }).click();
    await expect(item).toContainText('retired');
    for (const door of [stack().door, stack().doorB]) {
      const denied = await request.post(`${door}/auth/begin`, {
        headers: { Origin: stack().page }, data: { method: name, tenant, return_to: `${stack().page}/studio` },
      });
      expect(denied.status()).toBe(401);
      expect((await denied.json()).error.code).toBe('authentication_failed');
    }
    await refused(await request.post(`${stack().door}/op/std.space.updateConnection`, { headers, data: { name, clientSecret: first } }), 404, 'not_found');
    const addedAgain = await rows(await request.post(`${stack().door}/op/std.space.addConnection`, { headers, data: params }));
    expect(addedAgain[0].key).not.toBe(originalKey);
    expect(addedAgain[0].state).toBe('active');
  } finally {
    await rows(await request.delete(`${stack().door}/op/std.space.delete`, { headers, params: { confirm: tenant } }));
  }
});
