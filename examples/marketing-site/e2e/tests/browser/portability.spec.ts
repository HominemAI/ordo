import { readFileSync } from 'node:fs';
import { cp, mkdir } from 'node:fs/promises';
import { resolve, sep } from 'node:path';
import { randomUUID } from 'node:crypto';
import { expect } from '@playwright/test';
import { test } from '../owner-session.js';
import { state, stack, rows, refused } from '../support.js';

test('a tenant exports real data and imports a staged bundle without duplicating it on retry', async ({ ownerPage, request }) => {
  const saved = await ownerPage.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const headers = { Origin: stack().page, Authorization: `Bearer ${saved.token}`, 'X-Ordo-Space': 'system' };
  const principal = readFileSync(`${state}/browser-owner-principal`, 'utf8').trim();
  const tenants: string[] = [];
  const scoped = (space: string) => ({ ...headers, 'X-Ordo-Space': space });
  try {
    for (const role of ['source', 'destination']) {
      const result = await rows(await request.post(`${stack().door}/op/std.space.create`, {
        headers: { ...headers, 'Idempotency-Key': randomUUID() }, data: { name: `Portability ${role} ${randomUUID()}`, owner: principal },
      }));
      expect(typeof result[0].space).toBe('string');
      tenants.push(result[0].space);
    }
    const [source, destination] = tenants;
    const title = `Portable ${randomUUID()}`;
    const created = await rows(await request.post(`${stack().door}/op/createCampaign`, {
      headers: scoped(source), data: { title, brief: 'Portable brief', tags: ['export'], budget: '42.75', due: '2027-02-03',
        created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z') },
    }));
    const id = created[0].created;
    await ownerPage.reload();
    await ownerPage.getByRole('combobox', { name: 'Workspace', exact: true }).selectOption(source);
    await ownerPage.getByRole('button', { name: 'Open workspace' }).click();
    await ownerPage.getByRole('button', { name: 'Data', exact: true }).click();
    const [exportResponse] = await Promise.all([
      ownerPage.waitForResponse(r => r.url().endsWith('/op/std.space.export') && r.status() === 200),
      ownerPage.getByRole('button', { name: 'Prepare workspace export' }).click(),
    ]);
    const exported = (await exportResponse.json()).rows[0];
    await expect(ownerPage.getByRole('status')).toContainText('Export prepared.');
    const path = resolve(exported.bundle);
    expect(path.startsWith(`${resolve(state, 'exports', source)}${sep}`)).toBe(true);
    const stage = resolve(state, 'imports', destination, 'roundtrip');
    await mkdir(resolve(stage, '..'), { recursive: true });
    // This is the documented operator staging boundary; export and import
    // themselves are real application calls, never internal executor calls.
    await cp(path, stage, { recursive: true, errorOnExist: true, force: false });
    const input = { bundle: 'roundtrip', name: `roundtrip-${randomUUID()}` };
    await ownerPage.getByRole('combobox', { name: 'Workspace', exact: true }).selectOption(destination);
    await ownerPage.getByRole('button', { name: 'Open workspace' }).click();
    await ownerPage.getByRole('button', { name: 'Data', exact: true }).click();
    await ownerPage.getByLabel('Staged bundle name (required)').fill(input.bundle);
    await ownerPage.getByLabel('Import name (required)').fill(input.name);
    await ownerPage.getByRole('button', { name: 'Import staged bundle' }).click();
    await expect(ownerPage.getByRole('status')).toContainText('Import complete.');
    const read = async (space: string) => rows(await request.get(`${stack().door}/op/campaignList`, { headers: scoped(space) }));
    expect(await read(destination)).toEqual(expect.arrayContaining([expect.objectContaining({ id, title, brief: 'Portable brief', budget: '42.75', due: '2027-02-03', tags: ['export'] })]));
    const before = await read(destination);
    await rows(await request.post(`${stack().doorB}/op/std.space.import`, { headers: scoped(destination), data: input }));
    expect(await read(destination)).toEqual(before);
    for (const bundle of ['../source', '/absolute', 'missing-bundle']) {
      await refused(await request.post(`${stack().door}/op/std.space.import`, {
        headers: scoped(destination), data: { bundle, name: `refusal-${randomUUID()}` },
      }), 409, 'import_refused');
      expect(await read(destination)).toEqual(before);
      expect((await read(source)).find(row => row.id === id)).toMatchObject({ title });
    }
  } finally {
    for (const tenant of tenants.reverse()) {
      await rows(await request.delete(`${stack().door}/op/std.space.delete`, { headers: scoped(tenant), params: { confirm: tenant } }));
    }
  }
});
