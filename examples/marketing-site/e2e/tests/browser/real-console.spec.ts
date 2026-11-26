import { readFileSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { test, expect } from '@playwright/test';
import { state } from '../support.js';

// Real console -> real host -> actual tenant database. No e2e_door fixture.
test('the real console sets up, links, rotates its key and manages a disposable tenant', async ({ page, context }) => {
  const consoleState = JSON.parse(readFileSync(`${state}/console.json`, 'utf8'));
  const cdp = await context.newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', { options: {
    protocol: 'ctap2', transport: 'internal', hasResidentKey: true, hasUserVerification: true,
    isUserVerified: true, automaticPresenceSimulation: true,
  } });
  const credentials = JSON.parse(readFileSync(`${state}/browser-owner-credentials.json`, 'utf8'));
  const deploymentIds = new Set(credentials.map((c: { credentialId: string }) => c.credentialId));
  for (const credential of credentials) await cdp.send('WebAuthn.addCredential', { authenticatorId, credential });
  async function deploymentOnly(act: () => Promise<void>) {
    const all = (await cdp.send('WebAuthn.getCredentials', { authenticatorId })).credentials;
    const others = all.filter(c => !deploymentIds.has(c.credentialId));
    for (const c of others) await cdp.send('WebAuthn.removeCredential', { authenticatorId, credentialId: c.credentialId });
    try { await act(); } finally {
      for (const credential of others) await cdp.send('WebAuthn.addCredential', { authenticatorId, credential });
    }
  }
  async function open(name: string) {
    const group = name === 'Health' ? 'System' : 'Deployment';
    await page.getByRole('navigation', { name: 'Console', exact: true }).getByRole('link', { name: group, exact: true }).click();
    await page.getByRole('navigation', { name: `${group} pages` }).getByRole('link', { name, exact: true }).click();
  }
  try {
    await page.goto(consoleState.url);
    await expect(page.getByRole('heading', { name: 'Set up the console' })).toBeVisible();
    await page.getByLabel('Code').fill(readFileSync(`${consoleState.data}/setup-code`, 'utf8').trim());
    await page.getByLabel(/Name for this passkey/).fill('Real-stack owner');
    await page.getByRole('button', { name: 'Continue with a passkey' }).click();
    await expect(page.getByRole('heading', { name: 'You are the console owner' })).toBeVisible();
    await expect(page.getByRole('list', { name: 'Recovery codes' }).getByRole('listitem')).toHaveCount(10);
    await page.getByLabel('I saved these codes').check();
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    await open('Connection');
    await deploymentOnly(async () => {
      await page.getByRole('button', { name: 'Link with your passkey' }).click();
      await expect(page.getByRole('status')).toContainText('Linked.');
    });
    await page.getByRole('button', { name: 'Rotate the key now' }).click();
    await deploymentOnly(async () => {
      const prompt = page.getByRole('dialog', { name: 'Touch your passkey for the deployment' });
      await prompt.getByRole('button', { name: 'Use my passkey' }).click();
      await expect(page.getByRole('status')).toContainText('Rotated.');
    });
    await expect(page.getByRole('list', { name: 'Console keys' })).toContainText('revoked');
    await open('Tenants');
    const name = `Console ${randomUUID()}`;
    await page.getByLabel('Name', { exact: true }).fill(name);
    await page.getByRole('button', { name: 'Create tenant' }).click();
    await expect(page.getByRole('status')).toContainText(`Created ${name}.`);
    const row = page.getByRole('region', { name: 'Tenants, newest first' }).getByRole('row').filter({ hasText: name });
    await expect(row).toContainText('Active');
    await page.getByRole('button', { name: `Pause ${name}`, exact: true }).click();
    const pause = page.getByRole('dialog', { name: `Pause ${name}` });
    await pause.getByLabel('Reason').selectOption('billing');
    await pause.getByRole('button', { name: 'Pause tenant' }).click();
    await expect(row).toContainText('Paused');
    await page.getByRole('button', { name: `Resume ${name}`, exact: true }).click();
    await expect(row).toContainText('Active');
    await page.getByRole('button', { name: `Delete ${name}`, exact: true }).click();
    const del = page.getByRole('dialog', { name: `Delete ${name}` });
    const tenant = await del.locator('label .mono').innerText();
    await expect(del.getByRole('button', { name: 'Delete for good' })).toBeDisabled();
    await del.getByLabel(/Type the tenant's id/).fill(tenant);
    await del.getByRole('button', { name: 'Delete for good' }).click();
    await expect(page.getByRole('status')).toContainText(`Deleted ${name}.`);
    await open('Usage');
    await expect(page.getByRole('heading', { name: 'Usage', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Quotas' })).toBeVisible();
    await open('Connection');
    await page.getByRole('button', { name: 'Unlink', exact: true }).click();
    await page.getByRole('button', { name: 'Yes, unlink' }).click();
    await expect(page.getByRole('status')).toContainText('Unlinked.');
    await deploymentOnly(async () => {
      await page.getByRole('button', { name: 'Link with your passkey' }).click();
      await expect(page.getByRole('status')).toContainText('Linked.');
    });
  } finally {
    const all = (await cdp.send('WebAuthn.getCredentials', { authenticatorId })).credentials;
    writeFileSync(`${state}/browser-owner-credentials.json`, JSON.stringify(all.filter(c => deploymentIds.has(c.credentialId))), { mode: 0o600 });
  }
});
