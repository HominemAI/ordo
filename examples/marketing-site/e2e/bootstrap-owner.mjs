import { writeFileSync } from 'node:fs';
const [door, page, state] = process.argv.slice(2);
for (const name of ['owner', 'cache-owner']) {
const response = await fetch(`${door}/auth/complete`, {
  method: 'POST', headers: { Origin: page, 'Content-Type': 'application/json' },
  body: JSON.stringify({ method: 'guest', kind: 'new', display: `Site ${name}` }),
});
if (!response.ok) throw new Error(`Owner signup failed: ${response.status}`);
const { token, guest_secret: secret } = await response.json();
const principal = Buffer.from(token.split('.')[0], 'base64url').toString().split('|')[1];
if (!principal || !secret) throw new Error('Owner signup did not return an identity');
writeFileSync(`${state}/${name}-principal`, principal, { mode: 0o600 });
writeFileSync(`${state}/${name}-secret`, secret, { mode: 0o600 });
}

// A real passkey owner for browser workflows. Only the virtual authenticator's
// credentials are persisted in the private, throwaway stack directory.
const { chromium } = await import('@playwright/test');
const browser = await chromium.launch();
try {
  const context = await browser.newContext();
  const tab = await context.newPage();
  const cdp = await context.newCDPSession(tab);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', { options: {
    protocol: 'ctap2', transport: 'internal', hasResidentKey: true,
    hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true,
  } });
  await tab.goto(`${page}/studio`);
  await tab.getByText('Create a passkey', { exact: true }).click();
  await tab.getByLabel('Your name (required)').fill('Studio browser owner');
  await tab.getByRole('button', { name: 'Create account', exact: true }).click();
  await tab.waitForFunction(() => sessionStorage.getItem('lumen.studio') !== null, undefined, { timeout: 120_000 });
  const saved = JSON.parse(await tab.evaluate(() => sessionStorage.getItem('lumen.studio')));
  const principal = Buffer.from(saved.token.split('.')[0], 'base64url').toString().split('|')[1];
  if (!principal) throw new Error('Browser owner registration returned no principal');
  writeFileSync(`${state}/browser-owner-principal`, principal, { mode: 0o600 });
  const { credentials } = await cdp.send('WebAuthn.getCredentials', { authenticatorId });
  writeFileSync(`${state}/browser-owner-credentials.json`, JSON.stringify(credentials), { mode: 0o600 });
} finally { await browser.close(); }
