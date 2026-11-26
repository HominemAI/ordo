import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { expect } from '@playwright/test';
import { test } from '../owner-session.js';
import { stack, state, rows, refused } from '../support.js';
const exec = promisify(execFile);

test('file custody changes preserve old files and an S3 outage refuses until the store recovers', async ({ ownerPage, request }) => {
  test.setTimeout(300_000);
  const endpoint = process.env.E2E_S3_ENDPOINT;
  test.skip(!endpoint && !process.env.CI, 'Start s3-fixture.sh and set E2E_S3_ENDPOINT for this integration profile');
  expect(endpoint, 'The full CI profile must configure its real S3 fixture').toBeTruthy();
  expect(process.env.E2E_S3_CONTAINER).toBe('ordo-marketing-s3');
  const saved = await ownerPage.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const admin = { Origin: stack().page, Authorization: `Bearer ${saved.token}`, 'X-Ordo-Space': 'system' };
  const principal = readFileSync(`${state}/browser-owner-principal`, 'utf8').trim();
  const tenant = (await rows(await request.post(`${stack().door}/op/std.space.create`, {
    headers: { ...admin, 'Idempotency-Key': randomUUID() }, data: { name: `Storage ${randomUUID()}`, owner: principal },
  })))[0].space as string;
  const headers = { ...admin, 'X-Ordo-Space': tenant };
  let stopped = false;
  try {
    const campaign = (await rows(await request.post(`${stack().door}/op/createCampaign`, { headers,
      data: { title: 'Storage migration', brief: 'Keep every acknowledged file', tags: [], budget: '1.00', due: null,
        created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z') },
    })))[0].created;
    async function upload(name: string, body: string, door = stack().door) {
      const id = (await rows(await request.post(`${stack().door}/op/addAsset`, {
        headers, data: { campaign, name, kind: 'text/plain', size: Buffer.byteLength(body) },
      })))[0].created;
      const response = await request.put(`${door}/blob/assets/file/${id}`, {
        headers: { ...headers, 'Content-Type': 'text/plain' }, data: Buffer.from(body),
      });
      expect(response.status(), await response.text()).toBe(200);
      const handle = (await rows(await request.get(`${stack().door}/op/campaignFiles`, {
        headers, params: { campaign },
      }))).find(row => row.id === id)!.file as string;
      return { id, handle, body };
    }
    const local = await upload('before.txt', 'written before attachment');
    await ownerPage.reload();
    await ownerPage.getByRole('combobox', { name: 'Workspace', exact: true }).selectOption(tenant);
    await ownerPage.getByRole('button', { name: 'Open workspace' }).click();
    await ownerPage.getByRole('button', { name: 'Data', exact: true }).click();
    const attachment = { endpoint: endpoint!, region: 'us-east-1', bucket: 'marketing-attachments', credential: 'journey-s3', platformSealed: true };
    for (const [label, value] of [
      ['Storage endpoint (required)', attachment.endpoint], ['Storage region (required)', attachment.region],
      ['Bucket name (required)', attachment.bucket], ['Operator credential label (required)', attachment.credential],
    ]) await ownerPage.getByLabel(label).fill(value);
    const [attached] = await Promise.all([
      ownerPage.waitForResponse(r => r.url().endsWith('/op/std.space.attachBucket') && r.status() === 200),
      ownerPage.getByRole('button', { name: 'Attach bucket for new files' }).click(),
    ]);
    expect((await attached.json()).rows[0]).toMatchObject({ custody: 'platform-sealed', filesStayingPut: 1 });
    const again = await rows(await request.post(`${stack().door}/op/std.space.attachBucket`, { headers, data: attachment }));
    expect(again[0]).toMatchObject({ custody: 'platform-sealed', outcome: 'unchanged' });
    // B has not resolved this tenant's write store before attachment, so its
    // first upload observes the attachment without waiting out A's 30s cache.
    const remote = await upload('after.txt', 'written in the attached bucket', stack().doorB);
    const read = (door: string, file: typeof local) => request.get(`${door}/blob/${file.handle}`, { headers });
    for (const door of [stack().door, stack().doorB]) {
      for (const file of [local, remote]) {
        const response = await read(door, file);
        expect(response.status()).toBe(200);
        expect(await response.text()).toBe(file.body);
      }
      await refused(await request.get(`${door}/blob/${remote.handle}`, {
        headers: { 'X-Ordo-Space': tenant },
      }), 403, 'access_denied');
    }
    // Only this named fixture container is controlled. No deployment process
    // or shared service is stopped, and the local-store control remains live.
    await exec('docker', ['stop', '--time', '2', 'ordo-marketing-s3'], { timeout: 20_000 });
    stopped = true;
    await refused(await read(stack().door, remote), 503, 'server_busy');
    expect(await (await read(stack().door, local)).text()).toBe(local.body);
    await rows(await request.get(`${stack().doorB}/op/campaignList`, { headers: admin }));
    await exec('docker', ['start', 'ordo-marketing-s3'], { timeout: 20_000 });
    stopped = false;
    await expect.poll(async () => (await read(stack().door, remote)).status(), { timeout: 60_000 }).toBe(200);
    expect(await (await read(stack().door, remote)).text()).toBe(remote.body);
    const [detached] = await Promise.all([
      ownerPage.waitForResponse(r => r.url().endsWith('/op/std.space.detachBucket') && r.status() === 200),
      ownerPage.getByRole('button', { name: 'Use deployment storage for new files' }).click(),
    ]);
    expect((await detached.json()).rows[0]).toMatchObject({ custody: 'platform', filesStayingPut: 2 });
    const returned = await upload('returned.txt', 'back in deployment storage');
    for (const door of [stack().door, stack().doorB]) {
      for (const file of [local, remote, returned]) expect(await (await read(door, file)).text()).toBe(file.body);
    }
    await rows(await request.post(`${stack().door}/op/std.space.detachBucket`, { headers, data: {} }));
    await refused(await request.post(`${stack().door}/op/std.space.attachBucket`, {
      headers, data: { ...attachment, credential: 'missing-credential' },
    }), 400, 'bucket_refused');
    for (const file of [local, remote, returned]) expect(await (await read(stack().door, file)).text()).toBe(file.body);
  } finally {
    if (stopped) await exec('docker', ['start', 'ordo-marketing-s3'], { timeout: 20_000 });
    await rows(await request.delete(`${stack().door}/op/std.space.delete`, { headers, params: { confirm: tenant } }));
  }
});
