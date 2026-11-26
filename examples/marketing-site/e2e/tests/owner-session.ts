import { readFileSync, writeFileSync } from 'node:fs';
import { test as base, expect, type Page, type CDPSession } from '@playwright/test';
import { state } from './support.js';
const devices = new WeakMap<Page, { cdp: CDPSession; authenticatorId: string }>();
export const test = base.extend<{ ownerPage: Page }>({
  ownerPage: async ({ page }, use) => {
    await signIn(page);
    try { await use(page); }
    finally {
      const device = devices.get(page)!;
      const { credentials } = await device.cdp.send('WebAuthn.getCredentials', { authenticatorId: device.authenticatorId });
      writeFileSync(`${state}/browser-owner-credentials.json`, JSON.stringify(credentials), { mode: 0o600 });
    }
  },
});
export async function signIn(page: Page, options: { state?: string; url?: string } = {}) {
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
  for (const credential of JSON.parse(readFileSync(`${options.state ?? state}/browser-owner-credentials.json`, 'utf8'))) {
    await cdp.send('WebAuthn.addCredential', { authenticatorId, credential });
  }
  devices.set(page, { cdp, authenticatorId });
  await page.goto(`${options.url ?? ''}/studio`);
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Enquiry inbox' })).toBeVisible();
}
