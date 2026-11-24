import { writeFileSync } from 'node:fs';
const [door, page, state] = process.argv.slice(2);
const response = await fetch(`${door}/auth/complete`, {
  method: 'POST', headers: { Origin: page, 'Content-Type': 'application/json' },
  body: JSON.stringify({ method: 'guest', kind: 'new', display: 'Site owner' }),
});
if (!response.ok) throw new Error(`Owner signup failed: ${response.status}`);
const { token, guest_secret: secret } = await response.json();
const principal = Buffer.from(token.split('.')[0], 'base64url').toString().split('|')[1];
if (!principal || !secret) throw new Error('Owner signup did not return an identity');
writeFileSync(`${state}/owner-principal`, principal, { mode: 0o600 });
writeFileSync(`${state}/owner-secret`, secret, { mode: 0o600 });
