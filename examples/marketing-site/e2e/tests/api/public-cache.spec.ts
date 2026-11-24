import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, writeFile, rm, cp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test as base, expect, type APIRequestContext, type APIResponse, type TestInfo } from '@playwright/test';
import { owner, rows, stack } from '../support.js';
import { tableReads } from '../reads.js';
import { withDeployment } from '../deployment.js';
const exec = promisify(execFile);
type Plans = { headers: Record<string, string>; add: () => Promise<{ name: string; id: string }> };
const test = base.extend<{ plans: Plans }>({
  plans: async ({ request }, use) => {
    const headers = await owner(request);
    const ids: string[] = [];
    try {
      await use({ headers, add: async () => {
        const name = `plan-${randomUUID()}`;
        const result = await rows(await request.post(`${stack().door}/op/addPlan`, {
          headers: { ...headers, 'Idempotency-Key': randomUUID() },
          data: { name, tagline: 'Cache test', price: '17.00', period: 'month', features: [], highlighted: false, rank: 50 },
        }));
        const id = result[0].created as string;
        expect(id).toBeTruthy();
        ids.push(id);
        return { name, id };
      } });
    } finally {
      for (const id of ids) await rows(await request.delete(`${stack().door}/op/std.plans.delete`, {
        headers: { ...headers, 'Idempotency-Key': randomUUID() }, params: { id },
      }));
    }
  },
});
const pricing = (request: APIRequestContext, door = stack().door) => request.get(`${door}/op/pricing`);
async function cached(response: APIResponse, cache: string) {
  const result = await rows(response);
  expect(response.headers()['x-ordo-cache']).toBe(cache);
  return result;
}
async function warm(request: APIRequestContext, door: string) {
  let answer: Record<string, any>[] = [];
  await expect.poll(async () => {
    const response = await pricing(request, door);
    answer = await rows(response);
    return response.headers()['x-ordo-cache'];
  }, { intervals: [100], message: `Warm pricing on ${door}` }).toBe('hit');
  return answer;
}

test('a cache hit reads nothing from the database', async ({ request, plans }) => {
  await plans.add(); // Force a miss even when another test warmed pricing.
  const miss = await cached(await pricing(request), 'miss');
  const before = await tableReads('plans');
  for (let i = 0; i < 20; i++) expect(await cached(await pricing(request), 'hit')).toEqual(miss);
  expect(await tableReads('plans')).toBe(before);
  // Settle the write's own index scans before measuring the miss control.
  const added = await plans.add();
  const beforeMiss = await tableReads('plans');
  expect(await cached(await pricing(request), 'miss')).toEqual(expect.arrayContaining([expect.objectContaining({ name: added.name })]));
  expect(await tableReads('plans')).toBeGreaterThan(beforeMiss);
});

test('a write drops the cached answer, and the next read is fresh', async ({ request, plans }) => {
  const before = await warm(request, stack().door);
  const added = await plans.add();
  expect(before.some(row => row.name === added.name)).toBe(false);
  expect(await cached(await pricing(request), 'miss')).toEqual(expect.arrayContaining([expect.objectContaining({ name: added.name })]));
});

test('a write on one node drops the cached answer on the other node', async ({ request, plans }, testInfo) => {
  const before = await warm(request, stack().doorB);
  const added = await plans.add();
  expect(before.some(row => row.name === added.name)).toBe(false);
  const observed: { cache: string | undefined; fresh: boolean }[] = [];
  try {
    await expect.poll(async () => {
      const response = await pricing(request, stack().doorB);
      const data = await rows(response);
      const item = { cache: response.headers()['x-ordo-cache'], fresh: data.some(row => row.name === added.name) };
      observed.push(item);
      return item.cache === 'miss' && item.fresh;
    }, { intervals: [100], timeout: 20_000 }).toBe(true);
  } finally {
    await testInfo.attach('answers-after-commit', { body: JSON.stringify(observed, null, 2), contentType: 'application/json' });
    expect.soft(observed.filter(item => item.cache === 'hit' && !item.fresh), 'No stale cache hit after the write committed').toEqual([]);
  }
});

async function checkApply(request: APIRequestContext, testInfo: TestInfo, source: string, s = stack()) {
  for (const door of [s.door, s.doorB]) await warm(request, door);
  const folder = await mkdtemp(join(tmpdir(), 'ordo-marketing-deploy-'));
  try {
    await writeFile(join(folder, 'site.ordo'), source);
    await cp(join(s.program, '..', 'fn'), join(folder, 'fn'), { recursive: true });
    const result = await exec(s.ordo, ['apply', folder, '--direct', '--as', 'marketing-cache-e2e', '--wait-secs', '240'], {
      timeout: 260_000,
      env: { ...process.env, ORDO_PG_HOST: s.pg.host, ORDO_PG_PORT: s.pg.port, ORDO_PG_USER: s.pg.user, ORDO_PG_DB: s.pg.db,
        ORDO_SPACE: 'system', ORDO_KEY_PROVIDER: 'local', ORDO_MASTER_KEY: 'marketing-e2e-master', ORDO_DEV_UNSAFE_TEST_KEYS: '1',
        ORDO_TOKEN_KEY: '1'.repeat(64), ORDO_PAGE_KEY: '2'.repeat(64) },
    });
    await testInfo.attach('apply-output', { body: result.stdout + result.stderr, contentType: 'text/plain' });
    for (const door of [s.door, s.doorB]) {
      const response = await pricing(request, door);
      await rows(response);
      expect.soft(response.headers()['x-ordo-cache'], `First read after apply on ${door}`).toBe('miss');
    }
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
}

test('a comment-only apply drops every cached answer as the plan requires', async ({ request }, testInfo) => {
  test.setTimeout(300_000);
  await checkApply(request, testInfo, `${await readFile(stack().program, 'utf8')}\n# Cache deploy ${randomUUID()}\n`);
});

test('a new version drops every cached answer on both nodes', async ({ request }, testInfo) => {
  test.setTimeout(420_000);
  const source = await readFile(stack().program, 'utf8');
  expect(source).toContain('domain marketing @ 0.3.0');
  await withDeployment(request, testInfo, s => checkApply(request, testInfo, source.replace('domain marketing @ 0.3.0', 'domain marketing @ 0.3.1'), s));
});

test('two members never share one cached answer', async ({ request }) => {
  const headers = [await owner(request), await owner(request, 'cache-owner')];
  const principal = (h: Record<string, string>) => Buffer.from(h.Authorization.slice(7).split('.')[0], 'base64url').toString().split('|')[1];
  expect(principal(headers[0])).not.toBe(principal(headers[1]));
  // Warm a visitor's answer first: neither member may use it or each other's.
  const publicRows = await rows(await request.get(`${stack().door}/op/catalogue`));
  let before = await tableReads('products');
  for (const member of headers) {
    const response = await request.get(`${stack().door}/op/catalogue`, { headers: member });
    expect(await rows(response)).toEqual(publicRows);
    expect(response.headers()['x-ordo-cache']).toBeUndefined();
    const after = await tableReads('products');
    expect(after).toBeGreaterThan(before);
    before = after;
  }
});
