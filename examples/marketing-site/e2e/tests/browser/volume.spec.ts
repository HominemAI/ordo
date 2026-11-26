import { randomUUID } from 'node:crypto';
import { expect } from '@playwright/test';
import { test } from '../owner-session.js';
import { stack, rows } from '../support.js';

// More than two normal pages. This checks completeness, not a latency SLO.
test('the workspace reads every page of a large campaign list and filters without losing rows', async ({ ownerPage, request }) => {
  test.setTimeout(360_000);
  const saved = await ownerPage.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const headers = { Origin: stack().page, Authorization: `Bearer ${saved.token}`, 'X-Ordo-Space': 'system' };
  const tag = `volume-${randomUUID()}`;
  const count = 257;
  const ids: string[] = [];
  let next = 0;
  try {
    // Bounded concurrent writers use the actual HTTP write path.
    const writes = await Promise.allSettled(Array.from({ length: 3 }, async () => {
      while (next < count) {
        const index = next++;
        const result = await rows(await request.post(`${stack().door}/op/createCampaign`, {
          headers: { ...headers, 'Idempotency-Key': `${tag}-${index}` },
          data: { title: `${tag}-${String(index).padStart(3, '0')}`, brief: `Record ${index}`, tags: [tag, index % 2 ? 'odd' : 'even'],
            budget: `${index}.25`, due: null, created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z') },
        }));
        ids.push(result[0].created);
      }
    }));
    for (const result of writes) if (result.status === 'rejected') throw result.reason;
    await ownerPage.getByRole('button', { name: 'Campaigns', exact: true }).click();
    await ownerPage.getByLabel('Filter by tag').fill(tag);
    await ownerPage.getByRole('button', { name: 'Filter campaigns', exact: true }).click();
    await expect(ownerPage.getByRole('row').filter({ hasText: tag })).toHaveCount(count);
    await expect(ownerPage.getByRole('row').filter({ hasText: `${tag}-000` })).toBeVisible();
    await expect(ownerPage.getByRole('row').filter({ hasText: `${tag}-256` })).toBeVisible();
    // A late write changes an already loaded list, retaining all earlier pages.
    await rows(await request.delete(`${stack().doorB}/op/deleteCampaign`, { headers, params: { id: ids[0] } }));
    ids.shift();
    await expect(ownerPage.getByRole('row').filter({ hasText: tag })).toHaveCount(count - 1);
  } finally {
    let at = 0;
    await Promise.all(Array.from({ length: 3 }, async () => {
      while (at < ids.length) {
        const id = ids[at++];
        const response = await request.delete(`${stack().door}/op/deleteCampaign`, { headers, params: { id } });
        expect.soft(response.status(), 'volume journey cleanup').toBe(200);
      }
    }));
  }
});
