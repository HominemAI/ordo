#!/usr/bin/env node
// The manager's side of the consent ceremony, as a command line.
//
// It uses the manager's own session from `dev/.data/seed.json` and calls the
// door as the manager through the SDK.
//
//   node examples/assistant/manager/approve.mjs [request]   read it, then approve
//   node examples/assistant/manager/approve.mjs --decline [request]
//   node examples/assistant/manager/approve.mjs --list
//   node examples/assistant/manager/approve.mjs --revoke [delegation]
//
// With no request id it reads the one the assistant saved in
// `dev/.data/request.txt`. `--yes` approves without asking.
//
// The approval asks for a fresh passkey touch by default. The manager here is
// a guest with no passkey, so `serve.sh` exempts `std.delegation.approve` in
// `ORDO_STEP_UP_EXEMPT_OPS`. A real page calls `stepUp` first.
//
// Needs Node 18 or later and `npm install` in this directory.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { createInterface } from 'node:readline/promises';
import { fileURLToPath } from 'node:url';
import {
  OrdoClient,
  OrdoRefusal,
  approveDelegation,
  declineDelegation,
  myDelegations,
  reviewDelegation,
  revokeDelegation,
} from '@hominemai/ordo-sdk';

const args = process.argv.slice(2);
const data = process.env.ASSISTANT_DEV_DIR || join(dirname(fileURLToPath(import.meta.url)), '..', 'dev', '.data');
const seed = JSON.parse(readFileSync(join(data, 'seed.json'), 'utf8'));
const client = new OrdoClient({ baseUrl: seed.door, space: seed.space, token: seed.managerToken });
const positional = args.filter((a) => !a.startsWith('--'));
const when = (secs) => new Date(secs * 1000).toISOString().replace('T', ' ').slice(0, 16) + ' UTC';

function show(d) {
  console.log(`\n${d.agent.name} (named by ${d.agent.namedBy}) asks to act for you in ${d.tenant}.`);
  console.log(`For ${d.term.hours} hours: until ${d.term.expiresText} if you approve now.\n`);
  console.log('It could:');
  for (const op of d.ops) {
    console.log(`  ${op.op}`);
    for (const w of op.words) console.log(`    ${w}`);
  }
  if (d.notGranted.length) {
    console.log('\nIt asked for these too, and will not get them:');
    for (const n of d.notGranted) console.log(`  ${n.op}: ${n.reason}`);
  }
  if (d.unknownNames) console.log(`\n${d.unknownNames} name(s) it asked for are not ops of this app.`);
  console.log('\nIt can never:');
  for (const c of d.cannot) console.log(`  ${c}`);
  console.log(`\n${d.ceiling}`);
  if (d.ownerSees) console.log(d.ownerSees);
  console.log(d.revoke);
}

async function confirm(question) {
  if (args.includes('--yes')) return true;
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const answer = await rl.question(`${question} [y/N] `);
  rl.close();
  return /^y(es)?$/i.test(answer.trim());
}

async function main() {
  if (args.includes('--list')) {
    for (const r of await myDelegations(client)) {
      const id = r.delegation ?? r.request;
      console.log(`${r.state.padEnd(8)} ${r.agentName}  ${id}  until ${when(r.expiresAt)}  ${r.ops.join(', ')}`);
    }
    return;
  }
  if (args.includes('--revoke')) {
    let ids = positional;
    if (!ids.length) {
      ids = (await myDelegations(client)).filter((r) => r.state === 'live').map((r) => r.delegation);
    }
    for (const id of ids) {
      const r = await revokeDelegation(client, id);
      console.log(`${id}: ${r.outcome}`);
    }
    if (!ids.length) console.log('Nothing live to revoke.');
    return;
  }
  const request = positional[0] || readFileSync(join(data, 'request.txt'), 'utf8').trim();
  show(await reviewDelegation(client, request));
  if (args.includes('--decline') || !(await confirm('\nApprove?'))) {
    await declineDelegation(client, request);
    console.log('Declined.');
    return;
  }
  const a = await approveDelegation(client, request);
  console.log(`\nApproved ${a.delegation}: ${a.ops.join(', ')}, until ${when(a.expiresAt)}.`);
  if (a.replaced.length) console.log(`It replaces ${a.replaced.join(', ')}, now revoked.`);
}

main().catch((e) => {
  if (e instanceof OrdoRefusal) {
    console.error(`The door refused: ${e.class}. ${e.detail.message}`);
    if (e.class === 'not_found') console.error('No open request by that id is waiting for you.');
  } else {
    console.error(e);
  }
  process.exit(1);
});
