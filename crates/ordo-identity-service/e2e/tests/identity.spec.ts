/**
 * The identity service's page in a real browser with a virtual passkey:
 * sign-up by link, the account, sign-out and sign-in, a recovery code on a
 * new device, CLI approval and denial, and what a person sees when
 * something is wrong. Every page is checked for CSP refusals, page errors,
 * and sideways scrolling at phone width.
 */

import { createHash, randomBytes } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { fileURLToPath } from 'node:url';
import { expect, test, type Browser, type Page } from '@playwright/test';

interface Stack {
  origin: string;
  console: string;
  data: string;
  bin: string;
  env: Record<string, string>;
}

function stack(): Stack {
  const dir = process.env['E2E_STATE'] ?? fileURLToPath(new URL('../.stack', import.meta.url));
  return JSON.parse(readFileSync(`${dir}/stack.json`, 'utf8')) as Stack;
}

/** A fresh invite link, as the host command makes one. */
function invite(label: string): string {
  const s = stack();
  const out = execFileSync(s.bin, ['invite', '--label', label], {
    env: { ...process.env, ...s.env },
    encoding: 'utf8',
  });
  const m = /https?:\/\/\S+\/join#\S+/.exec(out);
  if (!m) throw new Error(`no link in: ${out}`);
  return m[0];
}

/** A browser profile with its own virtual passkey device. */
async function person(browser: Browser) {
  const context = await browser.newContext();
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: {
      protocol: 'ctap2',
      transport: 'internal',
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  const problems: string[] = [];
  page.on('console', (m) => {
    if (/Content Security Policy|Refused to|TrustedHTML|Trusted Type/i.test(m.text())) problems.push(m.text());
  });
  page.on('pageerror', (e) => problems.push(`page error: ${e.message}`));
  const credentials = async () => (await cdp.send('WebAuthn.getCredentials', { authenticatorId })).credentials;
  const count = async () => (await credentials()).length;
  /** Take the device's passkeys off it, as if it were a new device; `back` puts them back. */
  const setAside = async () => {
    const held = await credentials();
    await cdp.send('WebAuthn.clearCredentials', { authenticatorId });
    return {
      back: async () => {
        const now = await credentials();
        await cdp.send('WebAuthn.clearCredentials', { authenticatorId });
        for (const credential of held) await cdp.send('WebAuthn.addCredential', { authenticatorId, credential });
        return now;
      },
    };
  };
  return { context, page, problems, count, setAside };
}

/** Nothing scrolls sideways, at the current size and at phone width. */
async function fits(page: Page): Promise<void> {
  const size = page.viewportSize();
  for (const width of [size?.width ?? 1280, 360]) {
    await page.setViewportSize({ width, height: 740 });
    const [scroll, inner] = await page.evaluate(() => [document.documentElement.scrollWidth, window.innerWidth]);
    expect(scroll, `the page scrolls sideways at ${width}px`).toBeLessThanOrEqual(inner);
  }
  if (size) await page.setViewportSize(size);
}

async function saveCodes(page: Page): Promise<string[]> {
  const list = page.getByRole('list', { name: 'Recovery codes' });
  await expect(list.getByRole('listitem')).toHaveCount(10);
  const codes = await list.getByRole('listitem').allInnerTexts();
  const go = page.getByRole('button', { name: 'Continue' });
  await expect(go).toBeDisabled();
  await page.getByLabel('I saved these codes somewhere safe').check();
  await go.click();
  return codes;
}

async function join(page: Page, link: string, name: string): Promise<string[]> {
  await page.goto(link);
  await expect(page.getByRole('heading', { name: 'Join this deployment' })).toBeVisible();
  await fits(page);
  await page.getByRole('button', { name: 'Create a passkey' }).click();
  await expect(page.getByLabel('Your name')).toHaveAttribute('aria-invalid', 'true');
  await page.getByLabel('Your name').fill(name);
  await page.getByRole('button', { name: 'Create a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Save your recovery codes' })).toBeVisible();
  expect(new URL(page.url()).hash, 'the invite secret leaves the address bar').toBe('');
  await fits(page);
  const codes = await saveCodes(page);
  await expect(page).toHaveURL(/\/account$/);
  await expect(page.getByText(`Signed in as ${name}`)).toBeVisible();
  return codes;
}

/** Run SQL on the stack's database with psql (ORDO_PSQL names the binary). */
function sql(statement: string): string {
  const brew = '/opt/homebrew/opt/postgresql@17/bin/psql';
  const psql = process.env['ORDO_PSQL'] ?? (existsSync(brew) ? brew : 'psql');
  const s = stack();
  const url = s.env['ORDO_IDENTITY_DATABASE_URL']!;
  const password = /password=(\S+)/.exec(url)?.[1];
  return execFileSync(psql, [url, '-t', '-A', '-c', statement], {
    env: { ...process.env, ...(password ? { PGPASSWORD: password } : {}) },
    encoding: 'utf8',
  }).trim();
}

function passkeyItems(page: Page) {
  return page.getByRole('list', { name: 'Passkeys' }).getByRole('listitem');
}

test('sign up by link, manage passkeys and codes, sign out and in, recover on a new device', async ({ browser }) => {
  const dana = await person(browser);
  const { page } = dana;
  const first = readFileSync(`${stack().data}/first-invite`, 'utf8').trim();
  let codes = await join(page, first, 'Dana');

  await test.step('the account lists one passkey and offers no way to remove it', async () => {
    await expect(passkeyItems(page)).toHaveCount(1);
    await expect(page.getByRole('button', { name: /^Remove/ })).toHaveCount(0);
    await expect(page.getByText('This is your only passkey.')).toBeVisible();
    await expect(page.getByText('You have 10 unused codes left.')).toBeVisible();
    await fits(page);
  });

  await test.step('a second passkey is added, then removed after a confirmation', async () => {
    // The same device cannot hold two: the browser refuses, and the page says why.
    await page.getByRole('button', { name: 'Add a passkey' }).click();
    await expect(page.getByRole('alert')).toHaveText(/already holds a passkey for your account/);
    // Another device: this one with its passkey set aside.
    const aside = await dana.setAside();
    await page.getByRole('button', { name: 'Add a passkey' }).click();
    await expect(page.getByRole('status')).toHaveText('Added a passkey.');
    await expect(passkeyItems(page)).toHaveCount(2);
    expect(await aside.back()).toHaveLength(1);
    expect(await dana.count()).toBe(1);
    const remove = page.getByRole('button', { name: /^Remove Passkey 2/ });
    await remove.click();
    const dialog = page.getByRole('dialog', { name: 'Remove this passkey?' });
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Keep it' }).click();
    await expect(passkeyItems(page)).toHaveCount(2);
    await remove.click();
    await dialog.getByRole('button', { name: 'Remove' }).click();
    await expect(page.getByRole('status')).toHaveText('Removed Passkey 2.');
    await expect(passkeyItems(page)).toHaveCount(1);
  });

  await test.step('new recovery codes replace the old ones', async () => {
    await page.getByRole('button', { name: 'Make new recovery codes' }).click();
    await expect(page.getByRole('heading', { name: 'Save your new recovery codes' })).toBeVisible();
    const fresh = await saveCodes(page);
    expect(fresh).not.toEqual(codes);
    codes = fresh;
    await expect(page.getByText('You have 10 unused codes left.')).toBeVisible();
  });

  await test.step('sign out, then sign in with the passkey', async () => {
    await page.getByRole('button', { name: 'Sign out' }).click();
    await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
    await fits(page);
    await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
    await expect(page).toHaveURL(/\/account$/);
    await expect(page.getByText('Signed in as Dana')).toBeVisible();
  });

  await test.step('a return address on another site is not followed', async () => {
    await page.getByRole('button', { name: 'Sign out' }).click();
    await page.goto('/sign-in?return=https%3A%2F%2Fevil.example%2F');
    await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
    await expect(page).toHaveURL(/\/account$/);
  });

  await test.step('a recovery code on a new device makes a new passkey and nothing else', async () => {
    const laptop = await person(browser);
    const p = laptop.page;
    await p.goto('/sign-in');
    await p.getByText('Lost your passkey? Use a recovery code').click();
    await p.getByRole('textbox', { name: 'Recovery code' }).fill('0000-0000-0000-0000-0000-0000');
    await p.getByRole('button', { name: 'Use this code' }).click();
    await expect(p.getByRole('alert')).toHaveText(/That recovery code does not work/);
    await p.getByRole('textbox', { name: 'Recovery code' }).fill(codes[0]!.toLowerCase());
    await p.getByRole('button', { name: 'Use this code' }).click();
    await expect(p.getByRole('heading', { name: 'Make a new passkey' })).toBeVisible();
    // The session may do nothing else yet: the account sends it back here.
    await p.goto('/account');
    await expect(p.getByRole('heading', { name: 'Make a new passkey' })).toBeVisible();
    await p.getByRole('button', { name: 'Make a new passkey' }).click();
    await expect(p.getByRole('heading', { name: 'Save your new recovery codes' })).toBeVisible();
    await saveCodes(p);
    await expect(p).toHaveURL(/\/account$/);
    await expect(passkeyItems(p)).toHaveCount(1);
    expect(laptop.problems).toEqual([]);
    await laptop.context.close();
  });

  await test.step('the old device passkey no longer signs in', async () => {
    await page.goto('/account');
    await page.getByRole('button', { name: 'Sign out' }).click();
    await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
    await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
    await expect(page.getByRole('alert')).toHaveText(/not accepted/);
  });

  expect(dana.problems).toEqual([]);
  await dana.context.close();
});

test('a spent or broken invite link says what to do', async ({ browser }) => {
  const eli = await person(browser);
  const link = invite('Eli');
  await join(eli.page, link, 'Eli');
  const other = await person(browser);
  await other.page.goto(link);
  await other.page.getByLabel('Your name').fill('Someone');
  await other.page.getByRole('button', { name: 'Create a passkey' }).click();
  await expect(other.page.getByRole('alert')).toHaveText(/This link does not work/);
  await other.page.goto('/join');
  await expect(other.page.getByRole('heading', { name: 'This link is not complete' })).toBeVisible();
  expect([...eli.problems, ...other.problems]).toEqual([]);
  await eli.context.close();
  await other.context.close();
});

test('the CLI is approved with a passkey, the code reaches the loopback port, and deny sends nothing', async ({
  browser,
}) => {
  const got: string[] = [];
  const server = createServer((req, res) => {
    got.push(req.url ?? '');
    res.writeHead(200, { 'content-type': 'text/plain' });
    res.end('ordo login: you can close this tab');
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  const port = (server.address() as AddressInfo).port;
  try {
    const fay = await person(browser);
    const { page } = fay;
    await join(page, invite('Fay'), 'Fay');

    const verifier = randomBytes(32).toString('base64url');
    const challenge = createHash('sha256').update(verifier).digest('base64url');
    const state = randomBytes(16).toString('base64url');

    await page.goto(`/cli?port=${port}&state=${state}&code_challenge=${challenge}`);
    await expect(page.getByRole('heading', { name: 'Sign in the ordo command?' })).toBeVisible();
    await expect(page.getByText('Only approve a request you just started.')).toBeVisible();
    await expect(page.getByText(`port ${port} (127.0.0.1)`)).toBeVisible();
    await expect(page.getByText(/Fay \(p_/)).toBeVisible();
    await fits(page);
    await page.getByRole('button', { name: 'Deny' }).click();
    await expect(page.getByRole('heading', { name: 'Denied' })).toBeVisible();
    expect(got).toEqual([]);

    await page.goto(`/cli?port=${port}&state=${state}&code_challenge=${challenge}`);
    await page.getByRole('button', { name: 'Approve with my passkey' }).click();
    await expect.poll(() => got.length).toBe(1);
    const back = new URL(got[0]!, 'http://127.0.0.1');
    expect(back.pathname).toBe('/callback');
    expect(back.searchParams.get('state')).toBe(state);
    const code = back.searchParams.get('code');
    expect(code).toBeTruthy();

    const redeemed = await fetch(`${stack().origin}/cli/redeem`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ code, verifier }),
    });
    expect(redeemed.status).toBe(200);
    const body = (await redeemed.json()) as { session?: string; person?: string };
    expect(body.session).toBeTruthy();

    await page.goto('/cli?port=80&state=x&code_challenge=nope');
    await expect(page.getByRole('heading', { name: 'This request is not complete' })).toBeVisible();
    expect(fay.problems).toEqual([]);
    await fay.context.close();
  } finally {
    server.close();
  }
});

test('a change that needs a fresh touch asks for one, then succeeds', async ({ browser }) => {
  const gil = await person(browser);
  const { page } = gil;
  const codes = await join(page, invite('Gil'), 'Gil');
  // Age every live session's touch past the 5 minute window.
  sql('UPDATE sessions SET touched_at = 1 WHERE ended_at IS NULL');
  const refusals: string[] = [];
  page.on('response', async (r) => {
    if (r.status() === 403 && r.url().endsWith('/recovery-codes')) refusals.push(((await r.json()) as { error?: string }).error ?? '');
  });
  await page.getByRole('button', { name: 'Make new recovery codes' }).click();
  await expect(page.getByRole('heading', { name: 'Save your new recovery codes' })).toBeVisible();
  expect(refusals).toEqual(['presence_required']);
  const fresh = await saveCodes(page);
  expect(fresh).not.toEqual(codes);
  await expect(page.getByText('You have 10 unused codes left.')).toBeVisible();
  expect(Number(sql("SELECT max(touched_at) FROM sessions WHERE ended_at IS NULL"))).toBeGreaterThan(1_000_000);
  expect(gil.problems).toEqual([]);
  await gil.context.close();
});

test('the page follows a dark system theme', async ({ browser }) => {
  const context = await browser.newContext({ colorScheme: 'dark' });
  const page = await context.newPage();
  await page.goto('/sign-in');
  const dark = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  await page.emulateMedia({ colorScheme: 'light' });
  const light = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  expect(dark).not.toBe(light);
  await context.close();
});
