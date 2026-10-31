/**
 * The supervisor's screens in Chromium, in order: Health, then Runtime,
 * with the stack's fake host as the deployment. Each change is one test:
 * a good one, one the settings check refuses, one that crashes and rolls
 * back, a roll back from the list, a secret, and a killed process.
 */

import { readFileSync } from 'node:fs';
import { expect, test, type APIRequestContext, type BrowserContext, type Page } from '@playwright/test';
import { person, recoverCode, savedAndContinue, shownCodes, stack } from './support';

test.describe.configure({ mode: 'serial' });

let ctx: BrowserContext;
let page: Page;
let problems: string[];

const SECRET = 'e2e-sealed-7f3a9c41';

interface RuntimeBody {
  status: { state: string; pid: number | null; liveRevision: number; restarts: number };
  current: { revision: number; settings: Record<string, string> };
}

async function runtime(request: APIRequestContext): Promise<RuntimeBody> {
  const res = await request.get('/d/main/api/runtime');
  expect(res.status()).toBe(200);
  return (await res.json()) as RuntimeBody;
}

/** Fill the first setting row of the change form. */
async function setSetting(name: string, value: string, reason: string): Promise<void> {
  await page.getByLabel('Setting name', { exact: true }).fill(name);
  await page.getByLabel('Value', { exact: true }).fill(value);
  await page.getByLabel('Reason, for the history', { exact: true }).fill(reason);
}

const state = () => page.locator('.state [data-state]');
const live = () => page.getByTestId('live-revision');

test.beforeAll(async ({ browser }) => {
  ({ context: ctx, page, problems } = await person(browser));
  // The owner from the first-run flow, back in with `ordo console recover`.
  await page.goto('/');
  await page.getByRole('button', { name: 'Have an invite code?' }).click();
  await page.getByLabel('Code').fill(recoverCode());
  await page.getByRole('button', { name: 'Continue with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Your new passkey is ready' })).toBeVisible();
  await shownCodes(page);
  await savedAndContinue(page);
});

test.afterAll(async () => {
  await ctx.close();
});

test.afterEach(() => {
  expect(problems, 'CSP refusals or page errors').toEqual([]);
});

test('Health leads with the deployment: ready, on revision 1', async () => {
  await expect(page.getByRole('heading', { name: 'Health' })).toBeVisible();
  await expect(state()).toHaveAttribute('data-state', 'ready', { timeout: 20_000 });
  await expect(live()).toHaveText('1');
  const ready = await page.request.get('/health/ready');
  expect(ready.status()).toBe(200);
  expect((await ready.json()) as unknown).toMatchObject({ deployment: 'ready' });
});

test('Runtime shows the state, revision 1 and its settings', async () => {
  await page.getByRole('link', { name: 'Runtime' }).click();
  expect(new URL(page.url()).pathname).toBe('/d/main/runtime');
  await expect(page.getByRole('heading', { name: 'Runtime', level: 1 })).toBeVisible();
  await expect(state()).toHaveAttribute('data-state', 'ready');
  await expect(page.getByText('runs revision 1 of its settings')).toBeVisible();
  await expect(page.getByRole('list', { name: 'Settings' })).toContainText('ORDO_HOST_ADDR');
  await expect(page.locator('li[data-revision="1"]')).toContainText('Came up');
});

test('a setting change brings up revision 2', async () => {
  await setSetting('ORDO_FAKE_NOTE', 'two', 'e2e: a good change');
  await expect(page.getByText('Add ORDO_FAKE_NOTE = "two"')).toBeVisible();
  await page.getByRole('button', { name: 'Apply' }).click();
  const status = page.getByRole('status');
  await expect(status).toContainText('Revision 2 is live and ready.', { timeout: 60_000 });
  await expect(status).toContainText('Added ORDO_FAKE_NOTE = "two"');
  await expect(live()).toHaveText('2');
  await expect(page.getByRole('list', { name: 'Settings' })).toContainText('ORDO_FAKE_NOTE = two');
  await expect(page.locator('li[data-revision="2"]')).toContainText('e2e: a good change');
});

test('settings the check refuses touch nothing running', async () => {
  const before = await runtime(page.request);
  await setSetting('ORDO_FAKE_CHECK', 'refuse', 'e2e: refused');
  await page.getByRole('button', { name: 'Apply' }).click();
  const alert = page.getByRole('alert');
  await expect(alert).toContainText('was refused by the settings check. Nothing running was touched.', {
    timeout: 60_000,
  });
  await expect(alert).toContainText('ORDO_FAKE_CHECK');
  await expect(live()).toHaveText('2');
  const after = await runtime(page.request);
  expect(after.status.pid).toBe(before.status.pid);
  expect(after.status.liveRevision).toBe(2);
  expect(after.current.settings['ORDO_FAKE_CHECK']).toBeUndefined();
});

test('a revision that crashes rolls back to the one before', async () => {
  await setSetting('ORDO_FAKE_MODE', 'crash', 'e2e: crashes');
  await page.getByRole('button', { name: 'Apply' }).click();
  await expect(page.getByRole('alert')).toContainText(/Revision \d+ did not come up\. Revision 2 runs again\./, {
    timeout: 60_000,
  });
  await expect(state()).toHaveAttribute('data-state', 'ready', { timeout: 20_000 });
  await expect(live()).toHaveText('2');
  expect((await runtime(page.request)).current.settings['ORDO_FAKE_MODE']).toBeUndefined();
});

test('rolls back to revision 1 from the list', async () => {
  // The form still holds the crashing change; clear it so nothing rides along.
  await page.getByLabel('Setting name', { exact: true }).fill('');
  const one = page.locator('li[data-revision="1"]');
  await one.getByRole('button', { name: 'Roll back to this' }).click();
  await one.getByLabel('Reason, for the history').fill('e2e: back to the start');
  await one.getByRole('button', { name: 'Roll back to revision 1' }).click();
  await expect(page.getByRole('status')).toContainText(/Rolled back\. Revision \d+ .*ready\./, { timeout: 60_000 });
  await expect(state()).toHaveAttribute('data-state', 'ready');
  await expect(page.getByRole('list', { name: 'Settings' })).not.toContainText('ORDO_FAKE_NOTE');
  expect((await runtime(page.request)).current.settings['ORDO_FAKE_NOTE']).toBeUndefined();
});

test('a managed secret is set, and no page or API answer shows its value', async () => {
  await page.getByLabel('Secret name').fill('ORDO_FAKE_TOKEN');
  await page.getByLabel('Secret value').fill(SECRET);
  await page.getByLabel('Reason, for the history', { exact: true }).fill('e2e: a secret');
  await expect(page.getByText('Set secret ORDO_FAKE_TOKEN')).toBeVisible();
  await page.getByRole('button', { name: 'Apply' }).click();
  const status = page.getByRole('status');
  await expect(status).toContainText(/Revision \d+ is live and ready\./, { timeout: 60_000 });
  await expect(status).toContainText('Secret ORDO_FAKE_TOKEN changed');
  const secrets = page.getByRole('list', { name: 'Secrets' });
  await expect(secrets).toContainText('ORDO_FAKE_TOKEN');
  // On the supervisor the console seals it and hands it over; the
  // supervisor holds it from then on (step 7).
  await expect(secrets).toContainText(/kept sealed by the (console|supervisor)/);
  await expect(page.getByLabel('Secret value')).toHaveValue('');

  expect(await page.content()).not.toContain(SECRET);
  for (const path of ['runtime', 'runtime/revisions']) {
    const text = await (await page.request.get(`/d/main/api/${path}`)).text();
    expect(text, path).toContain('ORDO_FAKE_TOKEN');
    expect(text, path).not.toContain(SECRET);
  }
  await page.reload();
  await expect(page.getByRole('list', { name: 'Secrets' })).toContainText('ORDO_FAKE_TOKEN');
  expect(await page.content()).not.toContain(SECRET);
});

test('a killed deployment is started again, and the console stays ready', async () => {
  const before = await runtime(page.request);
  expect(before.status.state).toBe('ready');
  const pid = before.status.pid;
  expect(pid).not.toBeNull();

  // The console answers /health/ready the whole time the child is down.
  const answers: number[] = [];
  let polling = true;
  const poller = (async () => {
    while (polling) {
      answers.push((await page.request.get('/health/ready')).status());
      await new Promise((r) => setTimeout(r, 200));
    }
  })();

  process.kill(pid as number, 'SIGKILL');
  await expect
    .poll(
      async () => {
        const s = (await runtime(page.request)).status;
        return s.state === 'ready' && s.pid !== pid && s.restarts > before.status.restarts;
      },
      { timeout: 30_000, intervals: [250] },
    )
    .toBe(true);
  polling = false;
  await poller;

  expect(answers.length).toBeGreaterThan(0);
  expect(answers.every((a) => a === 200), `health answers: ${answers.join(',')}`).toBe(true);
  const after = await runtime(page.request);
  await expect(page.getByTestId('restarts')).toHaveText(String(after.status.restarts), { timeout: 10_000 });
  await expect(state()).toHaveAttribute('data-state', 'ready');
  await expect(page.getByText('Stopped by signal 9 (SIGKILL)')).toBeVisible();
  // The fake host logged its own start.
  const starts = readFileSync(stack().starts, 'utf8').trim().split('\n');
  expect(starts.at(-1)?.split(' ')[0]).toBe(String(after.status.pid));
});
