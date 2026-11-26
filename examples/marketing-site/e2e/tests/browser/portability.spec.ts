import { readFileSync } from 'node:fs';
import { cp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
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
    const bytes = Buffer.from(`Portable attachment ${randomUUID()}`);
    const asset = (await rows(await request.post(`${stack().door}/op/addAsset`, { headers: scoped(source),
      data: { campaign: id, name: 'roundtrip.txt', kind: 'text/plain', size: bytes.length },
    })))[0].created;
    const uploaded = await request.put(`${stack().door}/blob/assets/file/${asset}`, {
      headers: { ...scoped(source), 'Content-Type': 'text/plain' }, data: bytes,
    });
    expect(uploaded.status()).toBe(200);
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
    // Import remaps row IDs. Verify values and remapped relationships rather
    // than assuming the destination reuses the source's primary keys.
    const imported = (await read(destination)).find(row => row.title === title)!;
    expect(imported).toMatchObject({ title, brief: 'Portable brief', budget: '42.75', due: '2027-02-03', tags: ['export'] });
    expect(imported.id).toEqual(expect.any(String));
    for (const door of [stack().door, stack().doorB]) {
      const files = await rows(await request.get(`${door}/op/campaignFiles`, {
        headers: scoped(destination), params: { campaign: imported.id },
      }));
      expect(files).toHaveLength(1);
      expect(files[0]).toMatchObject({ campaign: imported.id, name: 'roundtrip.txt', kind: 'text/plain', size: bytes.length });
      const downloaded = await request.get(`${door}/blob/${files[0].file}`, { headers: scoped(destination) });
      expect(downloaded.status()).toBe(200);
      expect(await downloaded.body()).toEqual(bytes);
      await refused(await request.get(`${door}/blob/${files[0].file}`, {
        headers: { 'X-Ordo-Space': destination },
      }), 403, 'access_denied');
    }
    const before = await read(destination);
    await rows(await request.post(`${stack().doorB}/op/std.space.import`, { headers: scoped(destination), data: input }));
    expect(await read(destination)).toEqual(before);
    // Model incomplete copying and an unsupported bundle format in the
    // operator staging area. Never mutate the live database or original export.
    for (const variant of ['incomplete', 'unknown-format']) {
      const broken = resolve(state, 'imports', destination, variant);
      await cp(path, broken, { recursive: true, errorOnExist: true, force: false });
      const manifestPath = resolve(broken, 'manifest.json');
      if (variant === 'incomplete') await rm(manifestPath);
      else {
        const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
        manifest.format = 'unsupported-release-journey-format';
        await writeFile(manifestPath, JSON.stringify(manifest));
      }
      await refused(await request.post(`${stack().doorB}/op/std.space.import`, {
        headers: scoped(destination), data: { bundle: variant, name: `refused-${randomUUID()}` },
      }), 409, 'import_refused');
      expect(await read(destination)).toEqual(before);
    }
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
