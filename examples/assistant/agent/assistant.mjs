#!/usr/bin/env node
// The assistant: a small script that acts for a manager.
//
// It acts with a token that carries the manager's delegation. When it has no
// token yet it asks for one through the consent ceremony: it sends the
// request, prints the request id for the manager, and waits while the
// manager reads what it would be allowed to do and approves. Only this
// process holds the binder that collects the token, so the request id alone
// is safe to show.
//
// With the token it lists the pending expenses, approves the ones the limit
// allows, and leaves the rest for the manager. Then it tries three things the
// grant does not cover, to show the door refuses them. Whether a person or an
// AI model stands behind this loop makes no difference to the door.
//
//   node examples/assistant/agent/assistant.mjs [--limit 100.00] [--no-probe] [--ask] [--hours N]
//
// `--ask` asks again even when a token is saved. It reads, from
// `examples/assistant/dev/.data/` (written by serve.sh and seed.mjs):
// `seed.json` (the door, the tenant and the manager's member id) and
// `interface.json` (the program's op table). It saves its token to
// `assistant.token`. `ASSISTANT_TOKEN`, `ASSISTANT_DOOR`, `ASSISTANT_SPACE`
// and `ASSISTANT_INTERFACE` override them.
//
// Needs Node 18 or later and `npm install` in this directory (the SDK is read
// from `clients/ordo-sdk`, which needs `npm ci && npm run build` once).

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { OrdoClient, OrdoRefusal, requestDelegation, waitForDelegation, DelegationError } from '@hominemai/ordo-sdk';

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] !== undefined ? args[i + 1] : fallback;
};
const LIMIT = flag('--limit', '100.00');
const PROBE = !args.includes('--no-probe');
const ASK = args.includes('--ask');
const HOURS = Number(flag('--hours', '8'));

const AGENT = 'agt_assistant';
// Exactly the two ops the assistant needs: the pending list and the small
// approval. The manager sees what each reads and changes before saying yes.
const OPS = ['pendingExpenses', 'approveSmallExpense'];

const data = process.env.ASSISTANT_DEV_DIR || join(dirname(fileURLToPath(import.meta.url)), '..', 'dev', '.data');
const read = (name) => readFileSync(join(data, name), 'utf8');
const seed = process.env.ASSISTANT_SPACE ? {} : JSON.parse(read('seed.json'));
const door = process.env.ASSISTANT_DOOR || seed.door;
const space = process.env.ASSISTANT_SPACE || seed.space;
const descriptor = JSON.parse(
  process.env.ASSISTANT_INTERFACE ? readFileSync(process.env.ASSISTANT_INTERFACE, 'utf8') : read('interface.json'),
);

// The consent ceremony: ask, show the manager the request id, wait.
async function askTheManager() {
  const asked = await requestDelegation({ baseUrl: door }, {
    space,
    agent: AGENT,
    person: seed.managerId,
    ops: OPS,
    hours: HOURS,
  });
  writeFileSync(join(data, 'request.txt'), asked.request);
  console.log(`Asked the manager for ${OPS.join(' and ')}, for ${HOURS} hours.`);
  console.log(`Request ${asked.request}. In another terminal, as the manager:\n`);
  console.log(`  node examples/assistant/manager/approve.mjs\n`);
  console.log('Waiting for the answer...');
  const granted = await waitForDelegation(
    { baseUrl: door },
    { request: asked.request, binder: asked.binder },
    { intervalMs: 2000 },
  );
  writeFileSync(join(data, 'assistant.token'), granted.token);
  console.log(`Approved: ${granted.ops.join(', ')}, until ${new Date(granted.expiresAt * 1000).toISOString()}.\n`);
  return granted.token;
}

async function tokenOf() {
  if (process.env.ASSISTANT_TOKEN) return process.env.ASSISTANT_TOKEN.trim();
  if (!ASK && existsSync(join(data, 'assistant.token'))) return read('assistant.token').trim();
  return askTheManager();
}

let client;
const call = (name, params = {}) => client.request(specOf(name), params);

// Money arrives as a decimal string. Compare in whole cents, never as floats.
const cents = (s) => {
  const [whole, frac = ''] = String(s).split('.');
  return Number(whole) * 100 + Number((frac + '00').slice(0, 2));
};
const field = (row, name, index) => (Array.isArray(row) ? row[index] : row[name]);

const done = [];
const left = [];
const refused = [];

async function pending() {
  const rows = [];
  let page;
  do {
    const spec = specOf('pendingExpenses');
    const res = await client.request(spec, {}, page ? { page } : {});
    rows.push(...res.rows);
    page = res.next;
  } while (page);
  return rows;
}

async function main() {
  client = new OrdoClient({ baseUrl: door, space, token: await tokenOf() });
  console.log(`The assistant, acting for the manager of ${space}. Limit: ${LIMIT}.\n`);

  const rows = await pending();
  console.log(`${rows.length} expenses are waiting.`);

  for (const row of rows) {
    const id = field(row, 'id', 0);
    const amount = field(row, 'amount', 2);
    const title = field(row, 'title', 3);
    const who = field(row, 'submitter', 5);
    const label = `${amount.padStart(8)}  ${title} (${who})`;
    if (cents(amount) > cents(LIMIT)) {
      left.push(label);
      continue;
    }
    try {
      await call('approveSmallExpense', { expense: id, note: 'Approved by the assistant' });
      done.push(label);
    } catch (e) {
      if (!(e instanceof OrdoRefusal)) throw e;
      refused.push(`${label}: ${e.class}`);
    }
  }

  console.log(`\nApproved ${done.length}:`);
  for (const l of done) console.log(`  ${l}`);
  console.log(`\nLeft for the manager, over ${LIMIT} (${left.length}):`);
  for (const l of left) console.log(`  ${l}`);
  if (refused.length) {
    console.log(`\nRefused by the door (${refused.length}):`);
    for (const l of refused) console.log(`  ${l}`);
  }

  if (PROBE && left.length) {
    // Things the grant does not cover. Each should be refused.
    console.log('\nTrying what the grant does not cover:');
    const sample = rows.find((r) => cents(field(r, 'amount', 2)) > cents(LIMIT));
    const id = sample ? field(sample, 'id', 0) : undefined;
    const attempts = [
      ['approveExpense', { expense: id }, 'approve without the limit'],
      ['rejectExpense', { expense: id }, 'reject an expense'],
      ['std.expenses.delete', { id }, 'delete an expense'],
    ];
    for (const [op, params, what] of attempts) {
      try {
        await call(op, params);
        console.log(`  ${what}: ALLOWED (this should not happen)`);
        process.exitCode = 1;
      } catch (e) {
        if (!(e instanceof OrdoRefusal)) throw e;
        console.log(`  ${what}: refused, ${e.class}`);
      }
    }
    // The small-approval op checks the amount in its own body. On a large
    // expense it matches no row and changes nothing. (Today the door answers
    // that no-match write as `log_append_failed`, a 500; the README lists it.)
    if (id) {
      try {
        await call('approveSmallExpense', { expense: id });
        console.log('  approveSmallExpense on a large expense: ALLOWED (this should not happen)');
        process.exitCode = 1;
      } catch (e) {
        if (!(e instanceof OrdoRefusal)) throw e;
        console.log(`  approveSmallExpense on a large expense: refused, ${e.class} (nothing changed)`);
      }
    }
  }
}

main().catch((e) => {
  if (e instanceof DelegationError) {
    console.error(`The manager's answer: ${e.code ?? e.failure}. ${e.message}`);
  } else if (e instanceof OrdoRefusal) {
    console.error(`The door refused: ${e.class}. ${e.detail.message}`);
    if (e.class === 'delegation_required') console.error('The grant was revoked or has expired. Run again with --ask.');
  } else {
    console.error(e);
  }
  process.exit(1);
});
