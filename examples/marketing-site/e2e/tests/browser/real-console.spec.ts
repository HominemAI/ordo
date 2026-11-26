import { readFileSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { test, expect } from '@playwright/test';
import { state, owner, rows } from '../support.js';

// Real console -> real host -> actual tenant database. No e2e_door fixture.
test('the real console sets up, links, rotates its key and manages a disposable tenant', async ({ page, context, request }) => {
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
    const group = ['Health', 'Runtime'].includes(name) ? 'System' : 'Deployment';
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
    const recoveryCodes = await page.getByRole('list', { name: 'Recovery codes' }).getByRole('listitem').allInnerTexts();
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
    const creation = page.waitForResponse(r => r.url().endsWith('/api/tenants/create') && r.status() === 200);
    await page.getByRole('button', { name: 'Create tenant' }).click();
    await deploymentOnly(async () => {
      await page.getByRole('dialog', { name: 'Touch your passkey for the deployment' }).getByRole('button', { name: 'Use my passkey' }).click();
    });
    await expect(page.getByRole('status')).toContainText(`Created ${name}.`);
    const createdResponse = await creation;
    const original = createdResponse.request().postDataJSON();
    expect(original.idempotencyKey).toEqual(expect.any(String));
    const replay = await page.request.post(createdResponse.url(), {
      headers: { Origin: consoleState.url }, data: original,
    });
    expect(replay.status()).toBe(200);
    expect((await replay.json()).created.space).toBe((await createdResponse.json()).created.space);

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
    // Restart an actual supervised host and verify acknowledged data survives.
    const appHeaders = await owner(request);
    const campaign = (await rows(await request.post(`${consoleState.runtimeDoor}/op/createCampaign`, {
      headers: appHeaders, data: { title: 'Survives runtime restart', brief: 'Persisted before restart', tags: [], budget: '5.00', due: null,
        created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z') },
    })))[0].created;
    await open('Runtime');
    await page.getByRole('button', { name: 'Restart', exact: true }).click();
    await page.getByRole('button', { name: 'Yes, restart' }).click();
    await expect(page.getByRole('status')).toContainText('Restarted.', { timeout: 150_000 });
    expect((await rows(await request.get(`${consoleState.runtimeDoor}/op/oneCampaign`, {
      headers: appHeaders, params: { id: campaign },
    })))[0]).toMatchObject({ title: 'Survives runtime restart', brief: 'Persisted before restart' });
    await rows(await request.delete(`${consoleState.runtimeDoor}/op/deleteCampaign`, { headers: appHeaders, params: { id: campaign } }));
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
    // Lose the console credential, recover through the actual page, and prove
    // the old recovery set cannot open another session.
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();
    const all = (await cdp.send('WebAuthn.getCredentials', { authenticatorId })).credentials;
    for (const credential of all.filter(c => !deploymentIds.has(c.credentialId))) {
      await cdp.send('WebAuthn.removeCredential', { authenticatorId, credentialId: credential.credentialId });
    }
    await page.getByRole('button', { name: /Use a recovery code/ }).click();
    await page.getByLabel('Recovery code').fill(recoveryCodes[0]!);
    await page.getByRole('button', { name: 'Use this code' }).click();
    await expect(page.getByRole('heading', { name: 'Add a new passkey' })).toBeVisible();
    await page.reload();
    await expect(page.getByRole('heading', { name: 'Add a new passkey' })).toBeVisible();
    await page.getByRole('button', { name: 'Add a passkey' }).click();
    await expect(page.getByRole('heading', { name: 'Your new passkey is ready' })).toBeVisible();
    const fresh = await page.getByRole('list', { name: 'Recovery codes' }).getByRole('listitem').allInnerTexts();
    expect(fresh).toHaveLength(10);
    expect(fresh.filter(code => recoveryCodes.includes(code))).toEqual([]);
    await page.getByLabel('I saved these codes').check();
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();
    await page.getByRole('button', { name: /Use a recovery code/ }).click();
    for (const code of [recoveryCodes[0]!, recoveryCodes[1]!]) {
      await page.getByLabel('Recovery code').fill(code);
      await page.getByRole('button', { name: 'Use this code' }).click();
      await expect(page.getByRole('alert')).toContainText('That code did not work');
    }
    await page.getByRole('button', { name: 'Back to sign in' }).click();
    await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
    await expect(page.getByRole('button', { name: 'Sign out', exact: true })).toBeVisible();
  } finally {
    const all = (await cdp.send('WebAuthn.getCredentials', { authenticatorId })).credentials;
    writeFileSync(`${state}/console-owner-credentials.json`, JSON.stringify(all.filter(c => !deploymentIds.has(c.credentialId))), { mode: 0o600 });
    writeFileSync(`${state}/browser-owner-credentials.json`, JSON.stringify(all.filter(c => deploymentIds.has(c.credentialId))), { mode: 0o600 });
  }
});

// Keep this separate from recovery/runtime checks: a failure in those checks
// must not skip maintenance coverage after console setup has succeeded.
test('real console maintenance windows persist, reject invalid ranges, edit and remove skips', async ({ page, context }) => {
  const consoleState = JSON.parse(readFileSync(`${state}/console.json`, 'utf8'));
  const cdp = await context.newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', { options: {
    protocol: 'ctap2', transport: 'internal', hasResidentKey: true, hasUserVerification: true,
    isUserVerified: true, automaticPresenceSimulation: true,
  } });
  const deployment = JSON.parse(readFileSync(`${state}/browser-owner-credentials.json`, 'utf8'));
  const consoleCredentials = JSON.parse(readFileSync(`${state}/console-owner-credentials.json`, 'utf8'));
  expect(consoleCredentials.length, 'The real console setup must have enrolled a credential').toBeGreaterThan(0);
  for (const credential of consoleCredentials) await cdp.send('WebAuthn.addCredential', { authenticatorId, credential });
  await page.goto(consoleState.url);
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await expect(page.getByRole('button', { name: 'Sign out', exact: true })).toBeVisible();
  writeFileSync(`${state}/console-owner-credentials.json`, JSON.stringify((await cdp.send('WebAuthn.getCredentials', { authenticatorId })).credentials), { mode: 0o600 });
  // Only deployment credentials during door touches; both services use localhost.
  for (const credential of consoleCredentials) await cdp.send('WebAuthn.removeCredential', { authenticatorId, credentialId: credential.credentialId });
  for (const credential of deployment) await cdp.send('WebAuthn.addCredential', { authenticatorId, credential });
  async function open(name: string) {
    await page.getByRole('navigation', { name: 'Console', exact: true }).getByRole('link', { name: 'Deployment', exact: true }).click();
    await page.getByRole('navigation', { name: 'Deployment pages' }).getByRole('link', { name, exact: true }).click();
  }
  async function settled(message: string, role: 'status' | 'alert' = 'status') {
    const result = page.getByRole(role).filter({ hasText: message });
    const prompt = page.getByRole('dialog', { name: 'Touch your passkey for the deployment' });
    await expect(result.or(prompt)).toBeVisible();
    if (await prompt.isVisible()) await prompt.getByRole('button', { name: 'Use my passkey' }).click();
    await expect(result).toBeVisible();
  }
  function local(days: number, hour: number) {
    const d = new Date();
    d.setDate(d.getDate() + days);
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(hour)}:00`;
  }
  try {
    await open('Connection');
    if (await page.getByRole('button', { name: 'Link with your passkey' }).isVisible()) {
      await page.getByRole('button', { name: 'Link with your passkey' }).click();
      await expect(page.getByRole('status')).toContainText('Linked.');
    }
    await open('Maintenance');
    await expect(page.getByRole('heading', { name: 'Maintenance windows', level: 1 })).toBeVisible();
    const label = `Release rehearsal ${randomUUID()}`;
    await page.getByLabel('Starts (your local time)').fill(local(1, 2));
    await page.getByLabel('Ends (your local time)').fill(local(1, 3));
    await page.getByLabel('Message').fill(label);
    await page.getByRole('button', { name: 'Add window' }).click();
    await settled('Added window');
    const windows = page.getByRole('list', { name: 'Windows', exact: true });
    await page.reload();
    await expect(windows.getByText(label, { exact: true })).toBeVisible();
    await page.getByLabel('Starts (your local time)').fill(local(2, 5));
    await page.getByLabel('Ends (your local time)').fill(local(2, 4));
    await page.getByLabel('Message').fill('Backwards');
    await page.getByRole('button', { name: 'Add window' }).click();
    await settled('ends after it starts', 'alert');
    await expect(windows.getByRole('listitem').filter({ hasText: 'Backwards' })).toHaveCount(0);
    await page.getByLabel('Repeats on a schedule').check();
    await page.getByLabel('Schedule (cron, 5 fields, UTC)').fill('0 3 * * 0');
    await page.getByLabel('Length (minutes)').fill('60');
    await page.getByLabel('Message').fill(`${label} weekly`);
    await page.getByRole('button', { name: 'Add window' }).click();
    await settled('Added window');
    const weekly = windows.getByRole('listitem').filter({ hasText: `${label} weekly` });
    await weekly.getByRole('button', { name: 'Edit', exact: true }).click();
    await page.getByLabel('Length (minutes)').fill('90');
    await page.getByRole('button', { name: 'Save window' }).click();
    await settled('Saved window');
    await weekly.getByRole('button', { name: 'Skip a range' }).click();
    await weekly.getByLabel('From', { exact: true }).fill(local(0, 0));
    await weekly.getByLabel('To', { exact: true }).fill(local(8, 0));
    await weekly.getByLabel('Reason (optional)').fill('Release rehearsal');
    await page.getByRole('button', { name: 'Skip these times' }).click();
    await settled('Skipping window');
    await expect(page.getByRole('list', { name: 'Upcoming windows' }).getByText('skipped').first()).toBeVisible();
    await weekly.getByRole('button', { name: 'Remove skip' }).click();
    await settled('Removed skip');
    // Delete only the windows this journey created.
    for (const message of [`${label} weekly`, label]) {
      const row = windows.getByRole('listitem').filter({ has: page.getByText(message, { exact: true }) });
      await row.getByRole('button', { name: 'Delete', exact: true }).click();
      await page.getByRole('group', { name: /Confirm delete window/ }).getByRole('button', { name: 'Delete window' }).click();
      await settled('Deleted window');
      await expect(row).toHaveCount(0);
    }
    await page.reload();
    await expect(windows.getByText(label)).toHaveCount(0);
  } finally {
    const updated = (await cdp.send('WebAuthn.getCredentials', { authenticatorId })).credentials;
    writeFileSync(`${state}/browser-owner-credentials.json`, JSON.stringify(updated), { mode: 0o600 });
  }
});
