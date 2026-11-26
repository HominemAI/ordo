import { test, expect } from '@playwright/test';
import { owner, rows, refused, stack } from '../support.js';

test('malformed campaign writes are refused without creating records', async ({ request }) => {
  const headers = await owner(request);
  const before = await rows(await request.get(`${stack().door}/op/campaignList`, { headers }));
  for (const body of ['{"title":', '[]', 'null', Buffer.from([0xff, 0xfe])]) {
    await refused(await request.post(`${stack().door}/op/createCampaign`, {
      headers: { ...headers, 'Content-Type': 'application/json' }, data: body,
    }), 400, 'invalid_param');
    for (const door of [stack().door, stack().doorB]) {
      expect(await rows(await request.get(`${door}/op/campaignList`, { headers }))).toEqual(before);
    }
  }
});

test('runtime discovery retains the actual catalogue and interface for each replica and actor', async ({ request }, info) => {
  const headers = await owner(request);
  const observations: unknown[] = [];
  for (const [replica, door] of [['A', stack().door], ['B', stack().doorB]]) {
    for (const [actor, auth] of [['owner', headers], ['anonymous', {}]] as const) {
      for (const path of ['/catalog', '/interface']) {
        const response = await request.get(`${door}${path}`, { headers: auth });
        expect(response.status()).toBe(200);
        const document = await response.json();
        expect(path === '/catalog' ? Array.isArray(document) : Array.isArray(document.ops)).toBe(true);
        observations.push({ replica, actor, path, status: response.status(), document });
      }
    }
  }
  await info.attach('runtime-surfaces', {
    contentType: 'application/json', body: JSON.stringify({
      profile: 'marketing-two-hosts-local-identity-local-storage',
      reconciliation: 'pending-source-and-configuration-review', observations,
    }, null, 2),
  });
});
