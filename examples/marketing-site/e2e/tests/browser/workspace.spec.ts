import { readFileSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { test, expect, type Page, type CDPSession } from '@playwright/test';
import { stack, state, owner, rows } from '../support.js';
const devices = new WeakMap<Page, { cdp: CDPSession; authenticatorId: string }>();
// Keep counters from real assertions, including step-up. Reusing the original
// credential snapshot would look like a cloned authenticator to the server.
test.afterEach(async ({ page }) => {
  const device = devices.get(page);
  if (!device) return;
  const { credentials } = await device.cdp.send('WebAuthn.getCredentials', { authenticatorId: device.authenticatorId });
  writeFileSync(`${state}/browser-owner-credentials.json`, JSON.stringify(credentials), { mode: 0o600 });
});
function unzip(data: Buffer): Map<string, Buffer> {
  const entries = new Map<string, Buffer>();
  let at = 0;
  while (at + 30 <= data.length && data.readUInt32LE(at) === 0x04034b50) {
    expect(data.readUInt16LE(at + 8), 'The server ZIP stores uncompressed entries').toBe(0);
    const size = data.readUInt32LE(at + 18);
    const length = data.readUInt16LE(at + 26);
    const extra = data.readUInt16LE(at + 28);
    const name = data.toString('utf8', at + 30, at + 30 + length);
    const start = at + 30 + length + extra;
    entries.set(name, data.subarray(start, start + size));
    at = start + size;
  }
  return entries;
}
async function signIn(page: Page) {
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
  for (const credential of JSON.parse(readFileSync(`${state}/browser-owner-credentials.json`, 'utf8'))) {
    await cdp.send('WebAuthn.addCredential', { authenticatorId, credential });
  }
  devices.set(page, { cdp, authenticatorId });
  await page.goto('/studio');
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Enquiry inbox' })).toBeVisible();
}

test('a staff member follows up on an enquiry and sees another browser update live', async ({ page, request }) => {
  await signIn(page);
  const name = `Visitor ${randomUUID()}`;
  const headers = await owner(request);
  const id = (
    await rows(
      await request.post(`${stack().door}/op/contact`, {
        data: { name, email: 'visitor@example.org', company: 'Design team', message: 'Please send a quote' },
      }),
    )
  )[0].created;
  try {
    const row = page.getByRole('row').filter({ hasText: name });
    await expect(row).toBeVisible();
    await row.getByRole('button', { name: 'Follow up' }).click();
    await page.getByLabel('Status (required)').selectOption('responding');
    await page.getByLabel('Private notes').fill('Sent a proposal');
    await page.getByRole('button', { name: 'Save changes', exact: true }).click();
    await expect(row).toContainText('responding');
    await page.evaluate(() => {
      (window as any).__e2eMarker = 'same-document';
    });
    const current = (await rows(await request.get(`${stack().door}/op/inbox`, { headers }))).find((r) => r.id === id)!;
    await rows(
      await request.post(`${stack().door}/op/triageEnquiry`, {
        headers,
        data: { id, rev: current.rev, status: 'won', notes: 'Approved remotely' },
      }),
    );
    await expect(row).toContainText('won');
    expect(await page.evaluate(() => (window as any).__e2eMarker)).toBe('same-document');
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Sign in with a passkey' })).toBeVisible();
  } finally {
    await rows(await request.delete(`${stack().door}/op/deleteEnquiry`, { headers, params: { id } }));
  }
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
  await page.getByRole('button', { name: 'Check brief' }).click();
  await expect(page.getByText('5 words in this brief.')).toBeVisible();
  await page.getByLabel('Approval note (required)').fill('Reviewed for launch');
  await page.getByRole('button', { name: 'Approve campaign' }).click();
  await expect(page.getByRole('row').filter({ hasText: 'Reviewed for launch' })).toBeVisible();
  await page
    .getByLabel('Choose a file (required)')
    .setInputFiles({ name: 'brief.txt', mimeType: 'text/plain', buffer: Buffer.from('A campaign brief') });
  await page.getByRole('button', { name: 'Upload file' }).click();
  await expect(page.getByRole('row').filter({ hasText: 'brief.txt' })).toBeVisible();
  await page.getByRole('button', { name: 'Inspect campaign files' }).click();
  await expect(page.getByRole('region', { name: 'File inspection' })).toContainText('1 file verified, 16 bytes read.');
  await page.getByLabel('Remind at (your local time)').fill('2030-01-01T10:00');
  await page.getByRole('button', { name: 'Set reminder' }).click();
  await expect(page.getByRole('button', { name: 'Clear reminder' })).toBeVisible();
  await page.getByRole('button', { name: 'Clear reminder' }).click();
  await expect(page.getByRole('button', { name: 'Clear reminder' })).toBeHidden();
  const [bundle] = await Promise.all([
    page.waitForEvent('download'),
    page.getByRole('button', { name: 'Download studio data' }).click(),
  ]);
  const archive = unzip(readFileSync(await bundle.path()));
  expect(JSON.parse(archive.get('manifest.json')!.toString())).toMatchObject({
    format: 'ordo.download',
    complete: true,
  });
  expect(JSON.parse(archive.get('campaigns.json')!.toString()).some((r: any) => r.title === title)).toBe(true);
  const assets = JSON.parse(archive.get('assets.json')!.toString());
  const brief = assets.find((r: any) => r.name === 'brief.txt');
  expect(archive.get(brief.file.path)?.toString()).toBe('A campaign brief');
  await row.getByRole('button', { name: 'Delete', exact: true }).click();
  await row.getByRole('button', { name: 'Confirm delete' }).click();
  await expect(row).toBeHidden();
});

test('staff create and revoke an invitation with a real passkey session', async ({ page, request }, info) => {
  await signIn(page);
  await page.getByRole('button', { name: 'Team', exact: true }).click();
  const label = `Colleague ${randomUUID()}`;
  await page.getByLabel('Invitation label (required)').fill(label);
  await page.getByLabel('Role', { exact: true }).selectOption('editor');
  await page.getByRole('button', { name: 'Create invitation' }).click();
  await expect(page.getByText('Invitation created.', { exact: false })).toBeVisible();
  const ticket = await page.locator('output.ticket').innerText();
  await page.getByRole('button', { name: 'Hide invitation' }).click();
  const row = page.getByRole('row').filter({ hasText: label });
  await expect(row).toBeVisible();
  await row.getByRole('button', { name: 'Revoke invitation' }).click();
  await expect(row).toBeHidden();
  const revoked = await request.post(`${stack().door}/auth/invitation/peek`, { data: { ticket } });
  expect(revoked.status()).toBe(403);
  expect((await revoked.json()).error.code).toBe('invitation_refused');
  await page.screenshot({ path: info.outputPath('studio-desktop.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole('heading', { name: 'Team access' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: info.outputPath('studio-mobile.png'), fullPage: true });
});

test('a colleague accepts an invitation, receives read access, and loses it on revocation', async ({
  page,
  request,
}) => {
  await signIn(page);
  const saved = await page.evaluate(() => JSON.parse(sessionStorage.getItem('lumen.studio')!));
  const headers = { Origin: stack().page, Authorization: `Bearer ${saved.token}`, 'X-Ordo-Space': 'system' };
  const invitation = (
    await rows(
      await request.post(`${stack().door}/op/std.space.invite`, {
        headers,
        data: { role: 'editor', expires: '1h', label: `Campaign reviewer ${randomUUID()}` },
      }),
    )
  )[0];
  const accepted = await request.post(`${stack().door}/auth/complete`, {
    headers: { Origin: stack().page },
    data: {
      method: 'guest',
      kind: 'new',
      display: `Reviewer ${randomUUID()}`,
      space: 'system',
      invitation: invitation.ticket,
    },
  });
  expect(accepted.status(), await accepted.text()).toBe(200);
  const colleague = {
    Origin: stack().page,
    Authorization: `Bearer ${(await accepted.json()).token}`,
    'X-Ordo-Space': 'system',
  };
  const me = (await rows(await request.get(`${stack().door}/op/me`, { headers: colleague })))[0];
  const created = (
    await rows(
      await request.post(`${stack().door}/op/createCampaign`, {
        headers,
        data: {
          title: `Private ${randomUUID()}`,
          brief: 'Private launch plan',
          tags: [],
          budget: '75.00',
          due: null,
          created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
        },
      }),
    )
  )[0];
  const id = created.created;
  try {
    expect(
      (await rows(await request.get(`${stack().door}/op/campaignList`, { headers: colleague }))).some(
        (r) => r.id === id,
      ),
    ).toBe(false);
    await rows(
      await request.post(`${stack().door}/op/grantCampaignsAccess`, {
        headers,
        data: { campaign: id, member: me.id, level: 'read' },
      }),
    );
    const visible = (await rows(await request.get(`${stack().door}/op/campaignList`, { headers: colleague }))).find(
      (r) => r.id === id,
    )!;
    expect(visible.brief).toBe('Private launch plan');
    const denied = await request.post(`${stack().door}/op/editCampaign`, {
      headers: colleague,
      data: {
        id,
        rev: visible.rev,
        title: 'Taken over',
        brief: '',
        tags: [],
        budget: '0.00',
        due: null,
        status: 'draft',
      },
    });
    expect(denied.status()).toBe(403);
    expect((await denied.json()).error.code).toBe('access_guard');
    await rows(
      await request.delete(`${stack().door}/op/unshareCampaign`, { headers, params: { campaign: id, member: me.id } }),
    );
    expect(
      (await rows(await request.get(`${stack().door}/op/campaignList`, { headers: colleague }))).some(
        (r) => r.id === id,
      ),
    ).toBe(false);
    await rows(
      await request.post(`${stack().door}/op/std.space.setRole`, { headers, data: { member: me.id, role: 'viewer' } }),
    );
    const blocked = await request.post(`${stack().door}/op/createCampaign`, { headers: colleague, data: {} });
    expect(blocked.status()).toBe(403);
    expect((await blocked.json()).error.code).toBe('access_denied');
    await rows(await request.post(`${stack().door}/op/std.space.suspend`, { headers, data: { member: me.id } }));
    const suspended = await request.get(`${stack().door}/op/people`, { headers: colleague });
    expect(suspended.status()).toBe(403);
    expect((await suspended.json()).error.code).toBe('access_denied');
    await rows(await request.post(`${stack().door}/op/std.space.reinstate`, { headers, data: { member: me.id } }));
    await rows(await request.get(`${stack().door}/op/people`, { headers: colleague }));
  } finally {
    await rows(await request.delete(`${stack().door}/op/deleteCampaign`, { headers, params: { id } }));
  }
});

test('staff publish products, plans and reviews through the editor and remove them again', async ({
  page,
  context,
}) => {
  await signIn(page);
  const publicPage = await context.newPage();
  for (const kind of ['Products', 'Plans', 'Reviews']) {
    const name = `${kind} ${randomUUID()}`;
    await page.getByRole('button', { name: kind, exact: true }).click();
    await page.getByRole('button', { name: `Add ${kind.toLowerCase().slice(0, -1)}`, exact: true }).click();
    if (kind === 'Reviews') {
      await page.getByLabel('Author (required)').fill(name);
      await page.getByLabel('Role (required)').fill('Designer');
      await page.getByLabel('Company (required)').fill('A small studio');
      await page.getByLabel('Quote', { exact: true }).fill(`A lovely launch ${name}`);
    } else {
      if (kind === 'Products') await page.getByLabel('URL name (required)').fill(`product-${randomUUID()}`);
      await page.getByLabel('Name (required)', { exact: true }).fill(name);
      await page.getByLabel('Tagline (required)').fill('A thoughtful launch');
      await page.getByLabel('Price (required)').fill('24.50');
      await page.getByLabel('Features (one per line)').fill('Made for teams\nPersonal support');
      if (kind === 'Products') await page.getByLabel('Private staff note').fill('Never publish this note');
      else await page.getByLabel('Billing period (required)').fill('month');
    }
    await page.getByRole('button', { name: 'Save changes', exact: true }).click();
    const row = page.getByRole('row').filter({ hasText: name });
    await expect(row).toBeVisible();
    await publicPage.goto('/');
    await expect(
      publicPage.getByText(kind === 'Reviews' ? `A lovely launch ${name}` : name, { exact: true }),
    ).toBeVisible();
    await expect(publicPage.getByText('Never publish this note')).toBeHidden();
    await row.getByRole('button', { name: 'Edit', exact: true }).click();
    if (kind === 'Reviews') await page.getByLabel('Quote', { exact: true }).fill(`Updated recommendation ${name}`);
    else await page.getByLabel('Tagline (required)').fill(`Updated launch ${name}`);
    await page.getByRole('button', { name: 'Save changes', exact: true }).click();
    // The editor closes after the write resolves; clicking Save alone does not await it.
    await expect(page.getByRole('button', { name: 'Save changes', exact: true })).toBeHidden();
    const [fresh] = await Promise.all([
      publicPage.waitForResponse(r => r.url().endsWith(`/op/${kind === 'Products' ? 'catalogue' : kind === 'Plans' ? 'pricing' : 'reviews'}`)),
      publicPage.reload(),
    ]);
    expect(fresh.headers()['cache-control']).not.toContain('stale-while-revalidate');
    expect(fresh.headers()['cdn-cache-control']).toContain('stale-while-revalidate');
    await expect(
      publicPage.getByText(kind === 'Reviews' ? `Updated recommendation ${name}` : `Updated launch ${name}`, {
        exact: true,
      }),
    ).toBeVisible();
    await row.getByRole('button', { name: 'Delete', exact: true }).click();
    await row.getByRole('button', { name: 'Confirm delete' }).click();
    await expect(row).toBeHidden();
    await publicPage.reload();
    await expect(publicPage.getByText(name, { exact: kind !== 'Reviews' })).toBeHidden();
  }
  await publicPage.close();
});
