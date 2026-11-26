import { randomUUID } from 'node:crypto';
import { test, expect } from '@playwright/test';
import { owner, rows, refused, stack } from '../support.js';

test('file inspection runs as the caller, records outcomes and refuses a file from another campaign', async ({ request }) => {
  const headers = await owner(request);
  const ids: string[] = [];
  const text = `Private brief ${randomUUID()}`;
  try {
    for (const label of ['primary', 'other']) {
      ids.push((await rows(await request.post(`${stack().door}/op/createCampaign`, { headers, data: {
        title: `Inspection ${label} ${randomUUID()}`, brief: 'Inspect the attached brief', tags: [], budget: '1.00', due: null,
        created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
      } })))[0].created);
    }
    const fileId = (await rows(await request.post(`${stack().door}/op/addAsset`, { headers,
      data: { campaign: ids[0], name: 'private.txt', kind: 'text/plain', size: Buffer.byteLength(text) },
    })))[0].created;
    const upload = await request.put(`${stack().door}/blob/assets/file/${fileId}`, {
      headers: { ...headers, 'Content-Type': 'text/plain' }, data: Buffer.from(text),
    });
    expect(upload.status()).toBe(200);
    const file = (await rows(await request.get(`${stack().door}/op/campaignFiles`, { headers, params: { campaign: ids[0] } })))[0].file;
    const url = `${stack().door}/fn/inspectCampaignFile`;
    const good = await request.post(url, { headers, data: { campaign: ids[0], file } });
    expect(good.status(), await good.text()).toBe(200);
    expect((await good.json()).result).toEqual({ name: 'private.txt', type: 'text/plain', size: Buffer.byteLength(text) });
    expect(await good.text()).not.toContain(text);
    await refused(await request.post(url, { headers, data: { campaign: ids[1], file } }), 500, 'function_error');
    await refused(await request.post(url, { data: { campaign: ids[0], file } }), 403, 'access_denied');
    const recorded = await rows(await request.get(`${stack().door}/op/std.deployment.functionRuns`, {
      headers, params: { function: 'inspectCampaignFile', tenant: 'system', limit: 2 },
    }));
    expect(recorded).toHaveLength(2);
    expect(recorded[0].seq).toBeGreaterThan(recorded[1].seq);
    expect(recorded[1]).toMatchObject({ function: 'inspectCampaignFile', outcome: 'ok', wake: 'call' });
    expect(recorded[0].outcome).not.toBe('ok');
    for (const run of recorded) {
      const detail = await request.get(`${stack().door}/op/std.deployment.functionRun`, { headers, params: { seq: run.seq } });
      expect((await rows(detail))[0]).toMatchObject({ seq: run.seq, function: 'inspectCampaignFile' });
      expect(await detail.text()).not.toContain(text);
    }
    await refused(await request.get(`${stack().door}/op/std.deployment.functionRuns`), 403, 'access_denied');
    await refused(await request.get(`${stack().door}/op/std.deployment.functionRun`, { headers, params: { seq: '999999999' } }), 404, 'not_found');
  } finally {
    for (const id of ids) await rows(await request.delete(`${stack().door}/op/deleteCampaign`, { headers, params: { id } }));
  }
});
