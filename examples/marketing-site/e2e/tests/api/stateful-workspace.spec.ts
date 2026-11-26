import { randomUUID } from 'node:crypto';
import { test, expect } from '@playwright/test';
import { owner, rows, refused, stack } from '../support.js';

// Seeded data and action selection; actual concurrent requests choose the winner.
// All product actions enter through HTTP against the real two-replica stack.
test('seeded edits preserve winners and unrelated campaigns across replicas', async ({ request }, info) => {
  const seed = Number(process.env['ORDO_JOURNEY_SEED'] ?? 20261124);
  expect(Number.isInteger(seed) && seed >= 0 && seed <= 0xffffffff).toBe(true);
  let random = seed >>> 0;
  const next = () => (random = (Math.imul(random, 1664525) + 1013904223) >>> 0);
  const headers = await owner(request);
  const doors = [stack().door, stack().doorB];
  const tag = `sequence-${randomUUID()}`;
  const ids: string[] = [];
  const actions: Record<string, unknown>[] = [];
  const outcomes: Record<string, unknown>[] = [];
  async function outcome(id: string, action: () => Promise<void>) {
    const observation = { id, result: 'running' };
    outcomes.push(observation);
    try {
      await test.step(id, action);
      observation.result = 'passed';
    } catch (error) {
      observation.result = 'failed';
      throw error;
    }
  }
  const read = async (door: string, id: string) =>
    (await rows(await request.get(`${door}/op/campaignsTagged`, { headers, params: { tag } }))).find(r => r.id === id);
  try {
    await outcome('sequence.create', async () => {
      for (let index = 0; index < 3; index++) {
        const data = { title: `${tag}-${index}`, brief: 'Initial', tags: [tag], budget: '15.25', due: null,
          created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z') };
        const id = (await rows(await request.post(`${doors[0]}/op/createCampaign`, { headers, data })))[0].created;
        expect(typeof id).toBe('string');
        ids.push(id);
        actions.push({ action: 'create', index, id });
      }
    });
    for (let turn = 0; turn < 8; turn++) {
      const index = next() % 2;
      const id = ids[index];
      const before = await read(doors[0], id);
      const control = await read(doors[0], ids[2]);
      expect(before).toBeDefined();
      expect(control).toBeDefined();
      const common = { id, rev: before!.rev, title: before!.title, tags: [tag], budget: '15.25', due: null, status: 'draft' };
      const candidates = [0, 1].map(replica => ({ ...common, brief: `seed ${seed}, turn ${turn}, replica ${replica}` }));
      await outcome('sequence.cas-race', async () => {
        const responses = await Promise.all(doors.map((door, replica) => request.post(`${door}/op/editCampaign`, {
          headers, data: candidates[replica],
        })));
        const statuses = responses.map(response => response.status());
        actions.push({ action: 'concurrent-edit', turn, index, rev: before!.rev, candidates, statuses });
        expect([...statuses].sort()).toEqual([200, 409]);
        const winner = statuses.indexOf(200);
        await refused(responses[1 - winner], 409, 'cas_miss');
        for (const door of doors) {
          const observed = await read(door, id);
          expect(observed).toMatchObject({ brief: candidates[winner].brief });
          expect(observed!.rev).not.toBe(before!.rev);
          expect(await read(door, ids[2])).toEqual(control);
        }
      });
      await outcome('sequence.stale-retry', async () => {
        const committed = await read(doors[0], id);
        await refused(await request.post(`${doors[1]}/op/editCampaign`, { headers, data: candidates[0] }), 409, 'cas_miss');
        for (const door of doors) expect(await read(door, id)).toEqual(committed);
      });
    }
    let handle = '';
    await outcome('sequence.upload-read', async () => {
      const asset = (await rows(await request.post(`${doors[0]}/op/addAsset`, {
        headers, data: { campaign: ids[0], name: 'sequence.txt', kind: 'text/plain', size: 13 },
      })))[0].created;
      const response = await request.put(`${doors[1]}/blob/assets/file/${asset}`, {
        headers: { ...headers, 'Content-Type': 'text/plain' }, data: Buffer.from('Sequence file'),
      });
      expect(response.ok(), await response.text()).toBe(true);
      const files = await rows(await request.get(`${doors[0]}/op/campaignFiles`, { headers, params: { campaign: ids[0] } }));
      handle = files.find(row => row.id === asset)!.file;
      expect(handle).toBeTruthy();
      for (const door of doors) {
        const download = await request.get(`${door}/blob/${handle}`, { headers });
        expect(download.status()).toBe(200);
        expect(await download.text()).toBe('Sequence file');
      }
    });
    await outcome('sequence.anonymous-file-refused', async () => {
      await refused(await request.get(`${doors[1]}/blob/${handle}`), 403, 'access_denied');
    });
    await outcome('sequence.delete-cascade', async () => {
      const control = await read(doors[0], ids[2]);
      await rows(await request.delete(`${doors[1]}/op/deleteCampaign`, { headers, params: { id: ids[0] } }));
      for (const door of doors) {
        expect(await read(door, ids[0])).toBeUndefined();
        expect(await rows(await request.get(`${door}/op/campaignFiles`, { headers, params: { campaign: ids[0] } }))).toEqual([]);
        await refused(await request.get(`${door}/blob/${handle}`, { headers }), 404, 'not_found');
        expect(await read(door, ids[2])).toEqual(control);
      }
      actions.push({ action: 'delete', id: ids[0] });
      ids.shift();
    });
  } finally {
    await info.attach('stateful-outcomes', {
      body: JSON.stringify({ version: 1, seed, profile: 'real-two-hosts-local-storage',
        kind: 'http-application-journey', coveragePhase: 'mixed-setup-and-application', actions, outcomes }, null, 2),
      contentType: 'application/json',
    });
    for (const id of ids) {
      const response = await request.delete(`${doors[0]}/op/deleteCampaign`, { headers, params: { id } });
      expect.soft(response.status(), 'cleanup campaign').toBe(200);
    }
  }
});
