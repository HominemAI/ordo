#!/usr/bin/env node
// Fills an assistant deployment with a made-up company: one manager, 3 staff
// and 32 expenses, some small and some large. Then the manager decides 5 of
// them by hand, so the history already has entries when the assistant starts.
//
// Every person is a guest (a script cannot touch a passkey). The manager's
// own new tenant is the company, and the staff join it by invitation. All the
// writes go through the door's ops, as the people they belong to. The script
// never touches SQL.
//
// The data is deterministic: the same --seed gives the same people, amounts
// and titles. It leaves `.data/seed.json` for the assistant and
// `approve.mjs`, holding the tenant id, the manager's member id and token.
//
//   sh examples/assistant/dev/serve.sh
//   node examples/assistant/dev/seed.mjs [--door URL] [--seed N]
//
// Needs Node 18 or later. No dependencies.

import { randomUUID } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
function flag(name, fallback) {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] !== undefined ? args[i + 1] : fallback;
}
const DOOR = flag('--door', process.env.ASSISTANT_DOOR || 'http://127.0.0.1:4008').replace(/\/+$/, '');
const SEED = Number(flag('--seed', '1')) || 1;
const OUT = join(process.env.ASSISTANT_DEV_DIR || join(dirname(fileURLToPath(import.meta.url)), '.data'), 'seed.json');

// A small seeded generator (mulberry32).
function prng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rand = prng(SEED);
const pick = (xs) => xs[Math.floor(rand() * xs.length)];
const between = (lo, hi) => lo + Math.floor(rand() * (hi - lo + 1));

const now = new Date();
const daysAgoIso = (n, hour, minute) =>
  new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() - n, hour, minute, 0))
    .toISOString()
    .replace('.000Z', 'Z');

class SeedError extends Error {}

async function http(method, path, { headers = {}, body, query } = {}) {
  let url = DOOR + path;
  if (query) url += '?' + new URLSearchParams(query).toString();
  let res;
  try {
    res = await fetch(url, {
      method,
      headers: { Accept: 'application/json', ...headers },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    throw new SeedError(`Cannot reach the door at ${DOOR} (${e.cause?.code || e.message}). Start it with: sh examples/assistant/dev/serve.sh`);
  }
  const text = await res.text();
  let json = null;
  try { json = text ? JSON.parse(text) : null; } catch { /* not JSON */ }
  return { status: res.status, ok: res.ok, json, text };
}

function fail(what, r) {
  throw new SeedError(`${what} failed: HTTP ${r.status} ${r.text.slice(0, 400)}`);
}

async function signIn(display, team) {
  const body = { method: 'guest', kind: 'new', display };
  if (team) { body.space = team.space; body.invitation = team.ticket; }
  const r = await http('POST', '/auth/complete', { headers: { 'Content-Type': 'application/json' }, body });
  if (!r.ok || !r.json?.token) fail(`Guest sign-in of ${display}`, r);
  if (team && r.json.admission?.outcome !== 'admitted') {
    throw new SeedError(`${display} was not admitted: ${JSON.stringify(r.json.admission)}`);
  }
  return r.json;
}

class Session {
  constructor(name, token, space) { this.name = name; this.token = token; this.space = space; }
  async call(method, op, { query, body } = {}) {
    const headers = { 'X-Ordo-Space': this.space, Authorization: `Bearer ${this.token}` };
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    if (method === 'POST') headers['Idempotency-Key'] = randomUUID();
    const r = await http(method, `/op/${op}`, { headers, query, body });
    if (!r.ok) fail(`${op} as ${this.name}`, r);
    return r.json?.rows ?? [];
  }
  get(op, query) { return this.call('GET', op, { query }); }
  post(op, body) { return this.call('POST', op, { body }); }
  // The door learns of a new tenant or seat some moments after sign-in.
  async waitUntilKnown() {
    const until = Date.now() + 15000;
    let last;
    while (Date.now() < until) {
      const headers = { 'X-Ordo-Space': this.space, Authorization: `Bearer ${this.token}` };
      last = await http('GET', '/op/me', { headers });
      if (last.status === 200 && last.json?.rows?.length) return last.json.rows[0];
      await new Promise((r) => setTimeout(r, 250));
    }
    fail(`Waiting for ${this.name} to be known`, last);
  }
}

const field = (row, name, index) => (Array.isArray(row) ? row[index] : row[name]);

const MANAGER = 'Maya Chen';
const STAFF = ['Daniel Okafor', 'Priya Raman', 'Lucas Moreau'];
const SMALL = [
  'Train ticket to the client', 'Team lunch', 'Printer paper', 'Coffee for the workshop',
  'Taxi from the airport', 'USB-C cables', 'Parking at the venue', 'Conference snacks',
  'Postage for contracts', 'Notebook and pens', 'Software licence, one month', 'Lunch with a supplier',
];
const LARGE = [
  'Conference ticket', 'Hotel for the offsite', 'New monitor', 'Flights to the summit',
  'Catering for the launch', 'Training course', 'Office chairs', 'Laptop dock and screens',
];

async function main() {
  console.log(`Seeding the assistant deployment at ${DOOR} (seed ${SEED}).`);
  const health = await http('GET', '/health/live');
  if (!health.ok) throw new SeedError(`The door answered ${health.status} on /health/live.`);

  const mGuest = await signIn(MANAGER);
  const space = mGuest.space;
  if (!space) throw new SeedError(`The manager's sign-in answered no space: ${JSON.stringify(mGuest)}`);
  const manager = new Session(MANAGER, mGuest.token, space);
  const mMe = await manager.waitUntilKnown();
  const managerId = field(mMe, 'id', 0);
  console.log(`Manager ${MANAGER} (${field(mMe, 'role', 2)}) founded the company ${space}.`);

  const staff = [];
  for (const name of STAFF) {
    const inv = await manager.post('std.space.invite', { role: 'editor', expires: '7d', label: name });
    const ticket = field(inv[0] ?? {}, 'ticket', 0);
    if (!ticket) throw new SeedError(`std.space.invite answered no ticket: ${JSON.stringify(inv)}`);
    const g = await signIn(name, { space, ticket });
    const s = new Session(name, g.token, space);
    await s.waitUntilKnown();
    staff.push(s);
  }
  console.log(`Admitted ${staff.length} staff as editors.`);

  let small = 0;
  let large = 0;
  for (let k = 0; k < 32; k++) {
    const isSmall = rand() < 0.7;
    const title = pick(isSmall ? SMALL : LARGE);
    const cents = isSmall ? between(500, 10000) : between(10500, 90000);
    const amount = `${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, '0')}`;
    const who = pick(staff);
    await who.post('submitExpense', {
      manager: managerId,
      amount,
      title,
      added: daysAgoIso(between(0, 20), between(8, 17), between(0, 59)),
    });
    if (isSmall) small++; else large++;
  }
  console.log(`Staff submitted 32 expenses (${small} of 100.00 or less, ${large} above).`);

  // The manager decides 5 by hand, so the history has entries to start with.
  const pending = await manager.get('pendingExpenses');
  const rows = pending.map((r) => ({ id: field(r, 'id', 0), amount: field(r, 'amount', 2) }));
  const bigRows = rows.filter((r) => Number(r.amount) > 100);
  const smallRows = rows.filter((r) => Number(r.amount) <= 100);
  for (const r of bigRows.slice(0, 2)) await manager.post('approveExpense', { expense: r.id, note: 'Approved after review' });
  for (const r of bigRows.slice(2, 4)) await manager.post('rejectExpense', { expense: r.id, note: 'Over the team budget' });
  if (smallRows[0]) await manager.post('approveExpense', { expense: smallRows[0].id, note: 'Approved by hand' });
  console.log('The manager decided 5 by hand.');

  mkdirSync(dirname(OUT), { recursive: true });
  writeFileSync(OUT, JSON.stringify({ door: DOOR, space, managerId, managerToken: mGuest.token }, null, 2));
  console.log(`\nDone. Wrote ${OUT}. Next: node examples/assistant/agent/assistant.mjs`);
}

main().catch((e) => {
  console.error(e instanceof SeedError ? e.message : e);
  process.exit(1);
});
