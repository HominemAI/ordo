import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { expect, type Browser, type Page } from '@playwright/test';

/** What stack.sh wrote to $E2E_STATE/stack.json. */
export interface Stack {
  readonly issuer: string;
  readonly notesDoor: string;
  readonly viewerDoor: string;
  readonly notesPage: string;
  readonly viewerPage: string;
  readonly pg: {
    readonly host: string;
    readonly port: string;
    readonly db: string;
    readonly admin: string;
    readonly rolePassword: string;
  };
  readonly logs: string;
}

export function stack(): Stack {
  const state = process.env['E2E_STATE'] ?? new URL('../.stack', import.meta.url).pathname;
  return JSON.parse(readFileSync(`${state}/stack.json`, 'utf8')) as Stack;
}

export function unique(word: string): string {
  return `${word}-${Date.now().toString(36)}${Math.floor(Math.random() * 1e6).toString(36)}`;
}

/**
 * Run SQL as one of the stack's roles with psql (ORDO_PSQL names the
 * binary). Answers stdout, or throws with what psql said.
 */
export function sqlAs(role: string, statement: string): string {
  const brew = '/opt/homebrew/opt/postgresql@17/bin/psql';
  const psql = process.env['ORDO_PSQL'] ?? (existsSync(brew) ? brew : 'psql');
  const s = stack();
  return execFileSync(
    psql,
    ['-h', s.pg.host, '-p', s.pg.port, '-U', role, '-d', s.pg.db, '-t', '-A', '-v', 'ON_ERROR_STOP=1', '-c', statement],
    { env: { ...process.env, PGPASSWORD: s.pg.rolePassword }, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] },
  ).trim();
}

/** A browser profile with its own virtual passkey device: one person. */
export async function person(browser: Browser) {
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
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
  const passkeys = async () => (await cdp.send('WebAuthn.getCredentials', { authenticatorId })).credentials.length;
  return { context, page, passkeys };
}

/**
 * At the identity service's sign-in page: make an account with a passkey,
 * save the recovery codes, and go on. The service then sends the browser back
 * to the app that asked.
 */
export async function makeAccount(page: Page, name: string): Promise<void> {
  await page.getByRole('link', { name: 'Make an account' }).click();
  await expect(page.getByRole('heading', { name: 'Create an account' })).toBeVisible();
  await page.getByLabel('Your name').fill(name);
  await page.getByRole('button', { name: 'Create a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Save your recovery codes' })).toBeVisible();
  await expect(page.getByRole('list', { name: 'Recovery codes' }).getByRole('listitem')).toHaveCount(10);
  await page.getByLabel('I saved these codes somewhere safe').check();
  await page.getByRole('button', { name: 'Continue' }).click();
}

/** Open notes v2 and sign up there through the identity service. */
export async function joinNotes(page: Page, name: string): Promise<void> {
  await page.goto(stack().notesPage);
  await page.getByRole('button', { name: 'Continue with accounts' }).click();
  await expect(page).toHaveURL(new RegExp(`^${stack().issuer}/`));
  await makeAccount(page, name);
  await expect(page.getByLabel('New note', { exact: true })).toBeVisible({ timeout: 30_000 });
}

/** Add a note in notes v2, with these tags (none when empty). */
export async function addNote(page: Page, title: string, body: string, tags = ''): Promise<void> {
  await page.getByLabel('New note', { exact: true }).fill(title);
  await page.getByLabel('New note text').fill(body);
  await page.getByLabel('Tags', { exact: true }).fill(tags);
  await page.getByRole('button', { name: 'Add note' }).click();
  await expect(
    page.getByRole('region', { name: 'Your notes' }).locator('button.note').filter({ hasText: title }),
  ).toBeVisible();
}

/** A list on the viewer's page, by its label. */
export function list(page: Page, name: 'Public notes' | 'Your notes') {
  return page.getByRole('list', { name });
}

/**
 * Wait until the viewer's page shows `title` in the list, reloading: notes v2
 * writes on the device first and sends the note to its door a moment later.
 */
export async function shows(page: Page, name: 'Public notes' | 'Your notes', title: string): Promise<void> {
  await expect(async () => {
    await page.reload();
    await expect(list(page, name).getByRole('heading', { name: title })).toBeVisible({ timeout: 3_000 });
  }).toPass({ timeout: 60_000 });
}
