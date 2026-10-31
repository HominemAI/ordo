#!/usr/bin/env node
// Fills a teamspace deployment with a made-up company, so the app has
// something to show: "Northwind Studio", a founder, 7 colleagues, 3
// projects and about 60 tasks.
//
// Why guests: a script cannot touch a passkey. Every person here signs in as
// a guest (`ORDO_AUTH_GUEST=on`), which needs no device. The founder's own
// new tenant is the team. Colleagues join it with an invitation, at the
// role the invitation names. The founder sends the invitations through
// `std.space.invite`, which `serve.sh` frees from the passkey touch when it
// is started with TEAMSPACE_SEEDABLE=1.
//
// The script uses the door's ops only, as the people it makes. It never
// touches SQL. The app developer who runs it is invited last, as an owner,
// and finishes by passkey from the link the script prints.
//
// The data is deterministic. The same --seed gives the same people, titles,
// assignments and statuses. Dates are relative to today.
//
//   TEAMSPACE_SEEDABLE=1 sh examples/teamspace/dev/serve.sh
//   node examples/teamspace/dev/seed.mjs [--door URL] [--page URL] [--seed N] [--verify]
//
// Needs Node 18 or later. No dependencies.

import { randomUUID } from 'node:crypto';

// ── flags ──────────────────────────────────────────────────────────────────
const args = process.argv.slice(2);
function flag(name, fallback) {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] !== undefined ? args[i + 1] : fallback;
}
const DOOR = flag('--door', process.env.TEAMSPACE_DOOR || 'http://127.0.0.1:4001').replace(/\/+$/, '');
const PAGE = flag('--page', 'http://localhost:5174').replace(/\/+$/, '');
const SEED = Number(flag('--seed', '1')) || 1;
const VERIFY = args.includes('--verify');
if (args.includes('--help') || args.includes('-h')) {
  console.log('usage: node seed.mjs [--door URL] [--page URL] [--seed N] [--verify]');
  process.exit(0);
}

// ── a small seeded PRNG (mulberry32) ───────────────────────────────────────
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
function shuffle(xs) {
  const a = xs.slice();
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(rand() * (i + 1));
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
}

// ── dates ──────────────────────────────────────────────────────────────────
const now = new Date();
const isoDate = (d) => d.toISOString().slice(0, 10);
const today = isoDate(now);
const daysAgoIso = (n, hour, minute) => {
  const d = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() - n, hour, minute, 0));
  return d.toISOString().replace('.000Z', 'Z');
};
const daysFromToday = (n) => isoDate(new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() + n)));

// ── the door ───────────────────────────────────────────────────────────────
class SeedError extends Error {}

async function http(method, path, { headers = {}, body, query } = {}) {
  let url = DOOR + path;
  if (query) {
    const q = new URLSearchParams();
    for (const [k, v] of Object.entries(query)) if (v !== undefined && v !== null) q.set(k, String(v));
    url += '?' + q.toString();
  }
  let res;
  try {
    res = await fetch(url, {
      method,
      headers: { Origin: PAGE, Accept: 'application/json', ...headers },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    throw new SeedError(
      `Cannot reach the door at ${DOOR} (${e.cause?.code || e.message}).\n` +
        'Start it with: TEAMSPACE_SEEDABLE=1 sh examples/teamspace/dev/serve.sh',
    );
  }
  const text = await res.text();
  let json = null;
  try { json = text ? JSON.parse(text) : null; } catch { /* not JSON */ }
  return { status: res.status, ok: res.ok, json, text };
}

const SEEDABLE_HINT = 'Start serve.sh with TEAMSPACE_SEEDABLE=1 (guests on, std.space.invite without a passkey touch).';

function fail(what, r) {
  const code = r.json?.error?.code;
  const msg = r.json?.error?.message;
  let hint = '';
  if (code === 'guests_off') hint = '\n' + SEEDABLE_HINT;
  if (code === 'access_denied' || code === 'presence' || /presence|step.?up/i.test(msg || '')) hint = '\n' + SEEDABLE_HINT;
  throw new SeedError(`${what} failed: HTTP ${r.status} ${r.text.slice(0, 400)}${hint}`);
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

// A person in the team: reads and writes go through their own session.
class Session {
  constructor(name, token, space) { this.name = name; this.token = token; this.space = space; }
  headers(extra = {}) {
    return { 'X-Ordo-Space': this.space, Authorization: `Bearer ${this.token}`, ...extra };
  }
  async raw(method, op, { query, body } = {}) {
    const headers = this.headers();
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    if (method === 'POST') headers['Idempotency-Key'] = randomUUID();
    return http(method, `/op/${op}`, { headers, query, body });
  }
  async call(method, op, opts) {
    const r = await this.raw(method, op, opts);
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
      last = await this.raw('GET', 'me');
      if (last.status === 200 && last.json?.rows?.length) return last.json.rows[0];
      await new Promise((r) => setTimeout(r, 250));
    }
    fail(`Waiting for ${this.name} to be known to the team`, last);
  }
}

// Rows may come as objects or as arrays in the op's field order.
function field(row, name, index) {
  return Array.isArray(row) ? row[index] : row[name];
}

// ── the cast ───────────────────────────────────────────────────────────────
const COMPANY = 'Northwind Studio';
const FOUNDER = 'Maya Chen';
const COLLEAGUES = [
  { name: 'Daniel Okafor', role: 'manager' },
  { name: 'Priya Raman', role: 'editor' },
  { name: 'Lucas Moreau', role: 'editor' },
  { name: 'Sofia Alvarez', role: 'editor' },
  { name: 'Tom Becker', role: 'editor' },
  { name: 'Aiko Tanaka', role: 'editor' },
  { name: 'Ben Hughes', role: 'editor' },
];

const PROJECTS = [
  {
    name: 'Website relaunch',
    summary: 'New marketing site: content, design system, and a cut-over with no broken links.',
    founder: true,
    size: 5,
    quota: 26,
    tasks: [
      'Audit the current site content', 'Write the new homepage copy', 'Design the homepage hero',
      'Build the pricing page', 'Set up 301 redirects', 'Move the blog to the new CMS',
      'Fix mobile navigation', 'Add analytics events', 'Compress and lazy-load images',
      'Review accessibility with a screen reader', 'Write the case study for Fernwood', 'Prepare the launch checklist',
      'Update the footer links', 'Test the contact form', 'Create an Open Graph image set',
      'Proofread the About page', 'Load test the staging site', 'Switch DNS to the new host',
    ],
  },
  {
    name: 'Mobile app',
    summary: 'The first release of the companion app for iOS and Android.',
    founder: false,
    size: 4,
    quota: 20,
    tasks: [
      'Sketch the onboarding flow', 'Set up the build pipeline', 'Implement sign-in screen',
      'Offline cache for the task list', 'Push notification permissions', 'Fix the crash on rotation',
      'Design the app icon', 'Write the App Store listing', 'Dark mode colors',
      'Add pull to refresh', 'Beta test with ten customers', 'Handle expired sessions',
      'Localize strings for German', 'Profile startup time', 'Submit for TestFlight review',
      'Settings screen', 'Crash reporting setup', 'Deep links from email',
    ],
  },
  {
    name: 'Customer onboarding',
    summary: 'A smoother first month for new customers: welcome emails, guides and a check-in call.',
    founder: true,
    size: 4,
    quota: 18,
    tasks: [
      'Map the current onboarding steps', 'Draft the welcome email series', 'Record the product tour video',
      'Write the getting started guide', 'Build the checklist widget', 'Schedule day-7 check-in calls',
      'Collect feedback from the last five signups', 'Create the support macros', 'Set up the onboarding dashboard',
      'Translate the guide to Spanish', 'Review churn reasons', 'Add a sample workspace for new accounts',
      'Define the activation metric', 'Pilot with three accounts', 'Update the help center index',
      'Write the hand-off notes for support',
    ],
  },
];

// ── the run ────────────────────────────────────────────────────────────────
const say = (s) => console.log(s);

async function main() {
  say(`Seeding ${COMPANY} at ${DOOR} (seed ${SEED}, today ${today}).`);

  const health = await http('GET', '/health/live');
  if (!health.ok) throw new SeedError(`The door answered ${health.status} on /health/live.`);

  // The founder: a fresh guest whose own new tenant is the team.
  const fGuest = await signIn(FOUNDER);
  const space = fGuest.space;
  if (!space) throw new SeedError(`The founder's sign-in answered no space: ${JSON.stringify(fGuest)}`);
  const founder = new Session(FOUNDER, fGuest.token, space);
  const fMe = await founder.waitUntilKnown();
  say(`Founder ${FOUNDER} (${field(fMe, 'role', 2)}) founded team ${space}.`);

  // The colleagues, invited by the founder at the role they will hold.
  const people = [{ name: FOUNDER, role: 'owner', session: founder, id: field(fMe, 'id', 0) }];
  for (const c of COLLEAGUES) {
    const inv = await founder.post('std.space.invite', { role: c.role, expires: '7d', label: `${c.name} (${c.role})` });
    const ticket = field(inv[0] ?? {}, 'ticket', 0);
    if (!ticket) throw new SeedError(`std.space.invite answered no ticket: ${JSON.stringify(inv)}`);
    const g = await signIn(c.name, { space, ticket });
    const s = new Session(c.name, g.token, space);
    const me = await s.waitUntilKnown();
    people.push({ name: c.name, role: c.role, session: s, id: field(me, 'id', 0) });
  }
  say(`Invited and admitted ${COLLEAGUES.length} colleagues (1 manager, ${COLLEAGUES.length - 1} editors).`);

  // Projects and their teams.
  const byName = (n) => people.find((p) => p.name === n);
  const editorsAll = people.filter((p) => p.role === 'editor');
  const projects = [];
  for (const def of PROJECTS) {
    const added = daysAgoIso(between(26, 28), 9, 30);
    const rows = await founder.post('std.projects.create', { name: def.name, summary: def.summary, added });
    const id = rows[0]?.created;
    if (!id) throw new SeedError(`std.projects.create answered no id: ${JSON.stringify(rows)}`);
    // Team: the manager on every project, the founder on two, then editors.
    const members = [];
    if (def.founder) members.push(byName(FOUNDER));
    members.push(byName('Daniel Okafor'));
    const editors = shuffle(editorsAll);
    while (members.length < def.size) members.push(editors.shift());
    for (const m of members) await founder.post('std.team.create', { project: id, member: m.id });
    projects.push({ ...def, id, members });
    say(`Project "${def.name}": ${members.map((m) => m.name).join(', ')}`);
  }

  // Tasks: created by team members, added over the last 4 weeks.
  const created = [];
  for (const p of projects) {
    const titles = shuffle(p.tasks);
    for (let k = 0; k < p.quota; k++) {
      const base = titles[k % titles.length];
      const title = k < titles.length ? base : `${base}: follow-up`;
      const author = pick(p.members);
      const assignee = rand() < 0.2 ? null : pick(p.members);
      const due = rand() < 0.75 ? daysFromToday(between(-12, 21)) : null;
      const r = rand();
      const target = r < 0.3 ? 'todo' : r < 0.55 ? 'doing' : r < 0.7 ? 'review' : 'done';
      const body = { project: p.id, title, added: daysAgoIso(between(0, 27), between(8, 17), between(0, 59)) };
      if (assignee) body.assignee = assignee.id;
      if (due) body.due = due;
      const rows = await author.session.post('createTask', body);
      const id = rows[0]?.created;
      if (!id) throw new SeedError(`createTask answered no id: ${JSON.stringify(rows)}`);
      created.push({ id, project: p, author, target });
    }
  }
  say(`Created ${created.length} tasks.`);

  // Statuses: a task moves one step per call, and the rev is read again each time.
  const ORDER = ['todo', 'doing', 'review', 'done'];
  for (let step = 1; step <= 3; step++) {
    for (const p of projects) {
      const board = await p.members[0].session.get('board', { project: p.id });
      const byId = new Map(board.map((row) => [field(row, 'id', 0), row]));
      for (const t of created.filter((c) => c.project === p && ORDER.indexOf(c.target) >= step)) {
        const row = byId.get(t.id);
        if (!row) throw new SeedError(`Task ${t.id} is not on the board of "${p.name}".`);
        await t.author.session.post('moveTask', { id: t.id, rev: field(row, 'rev', 1), status: ORDER[step] });
      }
    }
  }
  const counts = { todo: 0, doing: 0, review: 0, done: 0 };
  for (const c of created) counts[c.target]++;
  say(`Moved tasks forward: todo ${counts.todo}, doing ${counts.doing}, review ${counts.review}, done ${counts.done}.`);

  // The app developer joins as an owner.
  const dev = await founder.post('std.space.invite', { role: 'owner', expires: '7d', label: 'App developer' });
  const devTicket = field(dev[0] ?? {}, 'ticket', 0);
  if (!devTicket) throw new SeedError(`std.space.invite answered no ticket: ${JSON.stringify(dev)}`);

  say('');
  say('Done.');
  say(`  Team id:   ${space}`);
  say(`  People:    ${people.length} (1 owner, 1 manager, ${people.length - 2} editors)`);
  say(`  Projects:  ${projects.length}`);
  say(`  Tasks:     ${created.length}`);
  say('');
  say('Open this link and sign up with a passkey to join as an owner:');
  say(`  ${PAGE}/#/join/${space}/${devTicket}`);
  say('');
  if (fGuest.guest_secret) {
    say(`Founder guest secret: ${fGuest.guest_secret}`);
    say('The page can resume this founder only if it is built with guest sign-in.');
  } else {
    say('The door answered no guest_secret for the founder.');
  }

  if (VERIFY) {
    for (const op of ['people', 'reportByProject', 'reportByPerson']) {
      const rows = await founder.get(op, op === 'people' ? undefined : { today });
      say('');
      say(`${op} (today=${today}):`);
      table(rows);
    }
  }
}

function table(rows) {
  if (!rows.length) { say('  (no rows)'); return; }
  const cols = Array.isArray(rows[0]) ? rows[0].map((_, i) => String(i)) : [...new Set(rows.flatMap((r) => Object.keys(r)))];  const cell = (r, i, c) => String(Array.isArray(r) ? r[i] : r[c] ?? '');
  const w = cols.map((c, i) => Math.max(c.length, ...rows.map((r) => cell(r, i, c).length)));
  const line = (vals) => '  ' + vals.map((v, i) => v.padEnd(w[i])).join('  ');
  say(line(cols));
  for (const r of rows) say(line(cols.map((c, i) => cell(r, i, c))));
}

main().catch((e) => {
  console.error(e instanceof SeedError ? e.message : e);
  process.exit(1);
});
