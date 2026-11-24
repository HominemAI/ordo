import { readFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { test, expect, type Page } from '@playwright/test';
import { stack, state, owner, rows } from '../support.js';
async function signIn(page: Page) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', { options: {
    protocol: 'ctap2', transport: 'internal', hasResidentKey: true, hasUserVerification: true,
    isUserVerified: true, automaticPresenceSimulation: true,
  } });
  for (const credential of JSON.parse(readFileSync(`${state}/browser-owner-credentials.json`, 'utf8'))) {
    await cdp.send('WebAuthn.addCredential', { authenticatorId, credential });
  }
  await page.goto('/studio');
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Enquiry inbox' })).toBeVisible();
}

test('a staff member follows up on an enquiry and sees another browser update live', async ({ page, request }) => {
  await signIn(page);
  const name = `Visitor ${randomUUID()}`;
  const headers = await owner(request);
  const id = (await rows(await request.post(`${stack().door}/op/contact`, { data: { name, email: 'visitor@example.org', company: 'Design team', message: 'Please send a quote' } })))[0].created;
  try {
    const row = page.getByRole('row').filter({ hasText: name });
    await expect(row).toBeVisible();
    await row.getByRole('button', { name: 'Follow up' }).click();
    await page.getByLabel('Status (required)').selectOption('responding');
    await page.getByLabel('Private notes').fill('Sent a proposal');
    await page.getByRole('button', { name: 'Save changes', exact: true }).click();
    await expect(row).toContainText('responding');
    await page.evaluate(() => { (window as any).__e2eMarker = 'same-document'; });
    const current = (await rows(await request.get(`${stack().door}/op/inbox`, { headers }))).find(r => r.id === id)!;
    await rows(await request.post(`${stack().door}/op/triageEnquiry`, { headers, data: { id, rev: current.rev, status: 'won', notes: 'Approved remotely' } }));
    await expect(row).toContainText('won');
    expect(await page.evaluate(() => (window as any).__e2eMarker)).toBe('same-document');
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Sign in with a passkey' })).toBeVisible();
  } finally { await rows(await request.delete(`${stack().door}/op/deleteEnquiry`, { headers, params: { id } })); }
});

test('staff create a campaign, upload a brief and schedule a reminder', async ({ page }) => {
  await signIn(page);
  const title = `Launch ${randomUUID()}`;
  await page.getByRole('button', { name: 'Campaigns', exact: true }).click();
  await page.getByRole('button', { name: 'New campaign', exact: true }).click();
  await page.getByLabel('Title (required)').fill(title);
  await page.getByLabel('Brief', { exact: true }).fill('A launch for small teams.');
  await page.getByLabel('Tags (one per line)').fill('launch\nstudio');
  await page.getByLabel('Budget (required)').fill('250.50');
  await page.getByRole('button', { name: 'Save changes', exact: true }).click();
  const row = page.getByRole('row').filter({ hasText: title });
  await expect(row).toBeVisible();
  await row.getByRole('button', { name: 'Files and access' }).click();
  await page.getByLabel('Choose a file (required)').setInputFiles({ name: 'brief.txt', mimeType: 'text/plain', buffer: Buffer.from('A campaign brief') });
  await page.getByRole('button', { name: 'Upload file' }).click();
  await expect(page.getByRole('row').filter({ hasText: 'brief.txt' })).toBeVisible();
  await page.getByLabel('Remind at (your local time)').fill('2030-01-01T10:00');
  await page.getByRole('button', { name: 'Set reminder' }).click();
  await expect(page.getByRole('button', { name: 'Clear reminder' })).toBeVisible();
  await page.getByRole('button', { name: 'Clear reminder' }).click();
  await expect(page.getByRole('button', { name: 'Clear reminder' })).toBeHidden();
  await row.getByRole('button', { name: 'Delete', exact: true }).click();
  await row.getByRole('button', { name: 'Confirm delete' }).click();
  await expect(row).toBeHidden();
});

test('staff create and revoke an invitation with a real passkey session', async ({ page }, info) => {
  await signIn(page);
  await page.getByRole('button', { name: 'Team', exact: true }).click();
  const label = `Colleague ${randomUUID()}`;
  await page.getByLabel('Invitation label (required)').fill(label);
  await page.getByLabel('Role', { exact: true }).selectOption('editor');
  await page.getByRole('button', { name: 'Create invitation' }).click();
  await expect(page.getByText('Invitation created.', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Hide invitation' }).click();
  const row = page.getByRole('row').filter({ hasText: label });
  await expect(row).toBeVisible();
  await row.getByRole('button', { name: 'Revoke invitation' }).click();
  await expect(row).toContainText('revoked');
  await page.screenshot({ path: info.outputPath('studio-desktop.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole('heading', { name: 'Team access' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: info.outputPath('studio-mobile.png'), fullPage: true });
});
