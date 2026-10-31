/**
 * What every flow shares: where the stack is, a virtual passkey, the mail
 * catcher, and the page's few common steps.
 */

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { chromium, expect, type Browser, type BrowserContext, type Page } from '@playwright/test';

export interface Stack {
  readonly door: string;
  readonly page: string;
  readonly pageV2: string;
  readonly programV2: string;
  readonly provider: string;
  readonly mail: string;
  readonly program: string;
  readonly pg: { readonly host: string; readonly port: string; readonly user: string; readonly db: string };
  readonly ordo: string;
  readonly logs: string;
}

let cached: Stack | null = null;

/** The running stack, as stack.sh wrote it to stack.json. */
export function stack(): Stack {
  if (cached) return cached;
  const target = process.env['E2E_TARGET'] ?? 'throwaway';
  if (target !== 'throwaway') {
    throw new Error(
      `E2E_TARGET=${target}: only the throwaway stack is built. A staging target with real credentials is phase 2.`,
    );
  }
  const dir = process.env['E2E_STATE'] ?? fileURLToPath(new URL('../.stack', import.meta.url));
  cached = JSON.parse(readFileSync(`${dir}/stack.json`, 'utf8')) as Stack;
  return cached;
}

/** A name no other test or run uses. */
export function unique(word: string): string {
  return `${word}${Date.now().toString(36)}${Math.floor(Math.random() * 1e6).toString(36)}`;
}

/**
 * A virtual passkey device on this page (Chrome DevTools' WebAuthn domain).
 * It holds discoverable credentials, verifies the user, and answers every
 * ceremony at once, as a person touching the sensor would.
 */
export async function passkeyDevice(page: Page): Promise<string> {
  const cdp = await page.context().newCDPSession(page);
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
  return authenticatorId;
}

/** A fresh browser profile with a passkey device: one person. */
export async function person(browser: Browser): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext();
  const page = await context.newPage();
  await passkeyDevice(page);
  return { context, page };
}

/**
 * How long a new person's tenant may take to set up: the SDK's own wait
 * (`DEFAULT_TENANT_WAIT_MS`, two minutes). The page shows "Setting up your
 * account" until then.
 */
export const SETTING_UP_MS = 120_000;

/**
 * The signed-in screen is up: the new-note box shows. A new person's page
 * first shows "Setting up your account" while their tenant is made; that is
 * waited out on its own, longer, deadline.
 */
export async function signedIn(page: Page): Promise<void> {
  const notes = page.getByLabel('New note', { exact: true });
  const settingUp = page.getByRole('heading', { name: 'Setting up your account' });
  await expect(notes.or(settingUp)).toBeVisible({ timeout: 30_000 });
  await expect(settingUp).toBeHidden({ timeout: SETTING_UP_MS });
  await expect(notes).toBeVisible({ timeout: 30_000 });
}

/** Open the page and sign up with a passkey under `display`. */
export async function signUp(page: Page, display: string): Promise<void> {
  await page.goto('/');
  await page.getByLabel('Your name').fill(display);
  await page.getByRole('button', { name: 'Sign up with a passkey' }).click();
  await signedIn(page);
}

/** Sign in with the passkey this page's device already holds. */
export async function signIn(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await signedIn(page);
}

export async function signOut(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('heading', { name: 'Sign up or sign in' })).toBeVisible();
}

/** Write a note and wait for it in the list. */
export async function addNote(page: Page, title: string, body = '', tags = ''): Promise<void> {
  await page.getByLabel('New note', { exact: true }).fill(title);
  if (body) await page.getByLabel('New note text').fill(body);
  if (tags) await page.getByLabel('Tags', { exact: true }).fill(tags);
  await page.getByRole('button', { name: 'Add note' }).click();
  await expect(noteInList(page, title)).toBeVisible();
}

/**
 * A note's button in the person's own list. It is the button that opens the
 * note; v2 puts Pin and Archive buttons beside it that name the note too.
 */
export function noteInList(page: Page, title: string) {
  return page.getByRole('region', { name: 'Your notes' }).locator('button.note').filter({ hasText: title });
}

/** The page has no sideways scroll: nothing is pushed off the right edge. */
export async function fitsWidth(page: Page): Promise<void> {
  const { scroll, inner } = await page.evaluate(() => ({
    scroll: document.documentElement.scrollWidth,
    inner: window.innerWidth,
  }));
  expect(scroll, `the page is ${scroll}px wide in a ${inner}px window`).toBeLessThanOrEqual(inner);
}

/** The button is on screen, inside the window sideways, and enabled. */
export async function reachable(page: Page, name: string | RegExp): Promise<void> {
  const button = page.getByRole('button', { name, exact: typeof name === 'string' }).first();
  await expect(button).toBeVisible();
  // The page can redraw the button just after it appears, for example when
  // the account's setup finishes. A scroll that lands on the old element
  // fails as "not attached", so it is tried again on the new one.
  await expect(async () => button.scrollIntoViewIfNeeded({ timeout: 2_000 })).toPass({ timeout: 10_000 });
  const box = await button.boundingBox();
  const width = await page.evaluate(() => window.innerWidth);
  expect(box, `${String(name)} has a box`).not.toBeNull();
  expect(box!.x, `${String(name)} starts on screen`).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width, `${String(name)} ends on screen`).toBeLessThanOrEqual(width);
  await expect(button).toBeEnabled();
}

/** The account id the page shows under "About this account". */
export async function accountId(page: Page): Promise<string> {
  await page.getByText('About this account').click();
  const id = (await page.locator('footer code').first().textContent()) ?? '';
  await page.getByText('About this account').click();
  expect(id).toMatch(/^usr_/);
  return id;
}

// ── The mail catcher ──────────────────────────────────────────────────────

interface Mail {
  readonly to: readonly string[];
  readonly data: string;
  readonly at: number;
}

export async function mailFor(address: string): Promise<Mail[]> {
  const res = await fetch(`${stack().mail}/messages?to=${encodeURIComponent(address)}`);
  return (await res.json()) as Mail[];
}

/** Wait for the `n`th (from 0) message to `address`, and read the code in it. */
export async function codeFor(address: string, n: number): Promise<string> {
  let found: string | null = null;
  await expect
    .poll(
      async () => {
        const mail = await mailFor(address);
        const m = mail[n] ? /code is (\d+)/.exec(mail[n].data) : null;
        found = m?.[1] ?? null;
        return found;
      },
      { message: `an emailed code for ${address}`, timeout: 20_000 },
    )
    .not.toBeNull();
  return found as unknown as string;
}

// ── For an upgrade flow (notes v2 over a live v1) ─────────────────────────

/**
 * Apply another program to the running stack, as a deployment owner applies
 * a new version over a live one. A flow that writes data, applies v2 here,
 * and reads the data back is the upgrade check. The page is not rebuilt.
 *
 * `ordo apply` returns once the door serves the version: the swap is in, the
 * access engine is rebuilt, the routines are switched and every tenant's
 * database is brought to it (`$nodes`, docs/deploying.md). So a page may call
 * the new ops, and a routine the version adds runs, as soon as this returns.
 * Past `--wait-secs` the apply exits 1 and this throws, naming what the door
 * was still doing.
 */
export function applyProgram(dir: string): string {
  const s = stack();
  return execFileSync(s.ordo, ['apply', dir, '--as', 'notes-e2e', '--wait-secs', '240'], {
    env: {
      ...process.env,
      ORDO_PG_HOST: s.pg.host,
      ORDO_PG_PORT: s.pg.port,
      ORDO_PG_USER: s.pg.user,
      ORDO_PG_DB: s.pg.db,
      ORDO_SPACE: 'system',
      ORDO_KEY_PROVIDER: 'local',
      ORDO_MASTER_KEY: 'notes-e2e-master',
      ORDO_DEV_UNSAFE_TEST_KEYS: '1',
      ORDO_TOKEN_KEY: '1'.repeat(64),
      ORDO_PAGE_KEY: '2'.repeat(64),
    },
    encoding: 'utf8',
  });
}
