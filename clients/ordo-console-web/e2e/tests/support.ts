/**
 * What the console flows share: where the stack is, a virtual passkey
 * device, the setup code, and the page checks every step makes.
 */

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, type Browser, type BrowserContext, type Page } from '@playwright/test';

export interface Stack {
  readonly console: string;
  readonly data: string;
  readonly databaseUrl: string;
  readonly ordo: string;
  /** The supervised fake host's own address. */
  readonly host: string;
  /** The deployment's door the console links to (examples/e2e_door.rs). */
  readonly door: string;
  /** The file each fake host start appends `<pid> <ms>` to. */
  readonly starts: string;
  readonly logs: string;
}

let cached: Stack | null = null;

/** The running stack, as stack.sh wrote it. */
export function stack(): Stack {
  if (cached) return cached;
  const dir = process.env['E2E_STATE'] ?? fileURLToPath(new URL('../.stack', import.meta.url));
  cached = JSON.parse(readFileSync(`${dir}/stack.json`, 'utf8')) as Stack;
  return cached;
}

/** The first-run setup code the console wrote to its data folder. */
export function setupCode(): string {
  return readFileSync(`${stack().data}/setup-code`, 'utf8').trim();
}

const CODE = /[0-9A-Z]{4}(?:-[0-9A-Z]{4}){5}/;

/** `ordo console recover`, as a deployment owner runs it inside the image. */
export function recoverCode(): string {
  const s = stack();
  const out = execFileSync(s.ordo, ['console', 'recover'], {
    env: { ...process.env, ORDO_CONSOLE_DATABASE_URL: s.databaseUrl, ORDO_CONSOLE_DEPLOYMENT: 'main' },
    encoding: 'utf8',
  });
  const m = CODE.exec(out);
  if (!m) throw new Error(`no code in the output of ordo console recover: ${out}`);
  return m[0];
}

/**
 * A virtual passkey device on this page (Chrome DevTools' WebAuthn domain).
 * It holds discoverable credentials and answers at once, as a person
 * touching the sensor would. `lose()` wipes it: a lost passkey.
 */
export interface PasskeyDevice {
  lose: () => Promise<void>;
  count: () => Promise<number>;
  /**
   * Run `act` with only the passkey `id` (base64url) on the device, then
   * put the others back. A sign-in with no user named offers every
   * passkey for the relying party; this picks one, as a person would.
   */
  only: (id: string, act: () => Promise<void>) => Promise<void>;
}

export async function passkeyDevice(page: Page): Promise<PasskeyDevice> {
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
  return {
    lose: async () => {
      await cdp.send('WebAuthn.clearCredentials', { authenticatorId });
    },
    count: async () => (await cdp.send('WebAuthn.getCredentials', { authenticatorId })).credentials.length,
    only: async (id, act) => {
      const b64 = (u: string) => u.replace(/-/g, '+').replace(/_/g, '/').replace(/=+$/, '');
      const { credentials } = await cdp.send('WebAuthn.getCredentials', { authenticatorId });
      const others = credentials.filter((c) => b64(c.credentialId) !== b64(id));
      for (const c of others) {
        await cdp.send('WebAuthn.removeCredential', { authenticatorId, credentialId: c.credentialId });
      }
      try {
        await act();
      } finally {
        for (const c of others) await cdp.send('WebAuthn.addCredential', { authenticatorId, credential: c });
      }
    },
  };
}

/** One person: a fresh browser profile with a passkey device. */
export async function person(browser: Browser) {
  const context: BrowserContext = await browser.newContext();
  const page = await context.newPage();
  const device = await passkeyDevice(page);
  const problems = watch(page);
  return { context, page, device, problems };
}

/**
 * Collect what must never happen on the page: a CSP refusal, an uncaught
 * error. CSP is `default-src 'self'`, so an inline script or style shows here.
 */
export function watch(page: Page): string[] {
  const problems: string[] = [];
  page.on('console', (m) => {
    const t = m.text();
    if (/Content Security Policy|Refused to/i.test(t)) problems.push(t);
  });
  page.on('pageerror', (e) => problems.push(`page error: ${e.message}`));
  return problems;
}

/** The page fits the screen: nothing scrolls sideways. */
export async function fits(page: Page): Promise<void> {
  const [scroll, width] = await page.evaluate(() => [document.documentElement.scrollWidth, window.innerWidth]);
  expect(scroll, 'the page scrolls sideways').toBeLessThanOrEqual(width);
}

/** The ten recovery codes on the screen that shows them once. */
export async function shownCodes(page: Page): Promise<string[]> {
  const list = page.getByRole('list', { name: 'Recovery codes' });
  await expect(list.getByRole('listitem')).toHaveCount(10);
  return list.getByRole('listitem').allInnerTexts();
}

/** Say the codes are saved and go on to the console. */
export async function savedAndContinue(page: Page): Promise<void> {
  const go = page.getByRole('button', { name: 'Continue' });
  await expect(go).toBeDisabled();
  await page.getByLabel('I saved these codes').check();
  await go.click();
  await signedIn(page);
}

/**
 * The signed-in console is up. It opens on the screen the address names,
 * so this checks the bar, not a heading.
 */
export async function signedIn(page: Page): Promise<void> {
  await expect(page.getByRole('navigation', { name: 'Console' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible();
}

export async function signOut(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
}

/**
 * Register a new passkey at the deployment and have it named the owner,
 * as `ORDO_DEPLOYMENT_OWNERS` does at a real deployment, then link the
 * console by sign-in with it from the Deployment screen: the claim step
 * was removed with the claim code (2026-10-30). The ceremony runs in
 * `page`, whose origin the door lists; the two calls go to the door
 * directly. The console's own passkey shares the relying party, so the
 * link runs with only the deployment's passkey on the device.
 */
export async function linkAsNamedOwner(page: Page, device: PasskeyDevice): Promise<void> {
  const door = stack().door;
  const begun = await page.request.post(`${door}/auth/register`, {
    data: { method: 'passkey', display: 'Deployment owner' },
  });
  expect(begun.ok()).toBeTruthy();
  const options = (await begun.json()) as Record<string, unknown>;
  const credential = await page.evaluate(async (o) => {
    const from = (s: string): ArrayBuffer => {
      const b64 = s.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((s.length + 3) % 4);
      const bin = atob(b64);
      const out = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
      return out.buffer;
    };
    const to = (b: ArrayBuffer): string =>
      btoa(String.fromCharCode(...new Uint8Array(b)))
        .replace(/\+/g, '-')
        .replace(/\//g, '_')
        .replace(/=+$/, '');
    const user = o['user'] as { id: string; name: string; display_name: string };
    const cred = (await navigator.credentials.create({
      publicKey: {
        challenge: from(o['challenge'] as string),
        rp: { id: o['rp_id'] as string, name: o['rp_name'] as string },
        user: { id: from(user.id), name: user.name, displayName: user.display_name },
        pubKeyCredParams: (o['algorithms'] as number[]).map((alg) => ({ type: 'public-key' as const, alg })),
        authenticatorSelection: { residentKey: 'required', userVerification: 'preferred' },
        attestation: 'none',
      },
    })) as PublicKeyCredential;
    const r = cred.response as AuthenticatorAttestationResponse;
    return {
      id: to(cred.rawId),
      kind: 'passkey_registration',
      challenge: o['challenge'] as string,
      client_data_json: to(r.clientDataJSON),
      attestation_object: to(r.attestationObject),
    };
  }, options);
  const { id, ...body } = credential;
  const done = await page.request.post(`${door}/auth/complete`, {
    data: { method: 'passkey', ...body },
  });
  expect(done.ok()).toBeTruthy();
  await device.only(id, async () => {
    await page.getByRole('button', { name: 'Link with your passkey' }).click();
    await expect(page.getByRole('status')).toContainText('Linked.');
  });
}
