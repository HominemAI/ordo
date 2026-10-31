// Fills the shopfront deployment with synthesized people and data.
//
//   node seed.mjs owner                      sign up the shop's owner
//   node seed.mjs                            the data
//   node seed.mjs make-staff <member> [staff|admin]
//
// Everything goes through the door's ops, as the pages do. People are
// software passkeys kept in $SHOPFRONT_DATA/seed-keys.json. A later run signs
// them in again instead of registering them twice.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Passkey } from './passkey.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const DOOR = (process.env.SHOPFRONT_DOOR ?? 'http://127.0.0.1:4010').replace(/\/$/, '');
const SPACE = process.env.SHOPFRONT_SPACE ?? 'shopfront';
const LOGIN = process.env.SHOPFRONT_LOGIN_PAGE ?? 'http://localhost:5180';
const DATA = process.env.SHOPFRONT_DATA ?? join(here, '.data');
const KEYS = join(DATA, 'seed-keys.json');
mkdirSync(DATA, { recursive: true });

class Refused extends Error {}
const fail = (msg) => { throw new Refused(msg); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── Keys on disk ───────────────────────────────────────────────────────────
const loadKeys = () => (existsSync(KEYS) ? JSON.parse(readFileSync(KEYS, 'utf8')) : {});
const saveKeys = (k) => writeFileSync(KEYS, JSON.stringify(k, null, 2));

// ── The door ───────────────────────────────────────────────────────────────
async function call(method, path, { headers = {}, body, query } = {}) {
  let url = DOOR + path;
  if (query && Object.keys(query).length) {
    url += '?' + new URLSearchParams(Object.entries(query).map(([k, v]) => [k, String(v)]));
  }
  const res = await fetch(url, {
    method,
    headers: { Origin: LOGIN, Accept: 'application/json', ...headers },
    body,
  });
  const text = await res.text();
  let json = null;
  try { json = JSON.parse(text); } catch { /* not JSON */ }
  return { status: res.status, json, text };
}

async function auth(path, obj) {
  const r = await call('POST', path, {
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(obj),
  });
  if (r.status !== 200) fail(`${path} refused (${r.status}): ${r.text}`);
  return r.json;
}

// The principal is field 1 of the token's payload (`payload.mac`).
const principalOf = (token) =>
  Buffer.from(token.split('.')[0], 'base64url').toString('utf8').split('|')[1];

let counter = 0;
const runId = Date.now().toString(36);

class Person {
  constructor(name, key) {
    this.name = name;
    this.key = key;
    this.token = null;
  }

  async register(space) {
    const opts = await auth('/auth/register', { method: 'passkey', display: this.name });
    const body = { ...this.key.register(opts.challenge), display: this.name };
    if (space) body.space = space;
    const done = await auth('/auth/complete', body);
    this.token = done.token;
    this.principal = principalOf(done.token);
    return done;
  }

  async signIn() {
    const opts = await auth('/auth/begin', { method: 'passkey' });
    // The door refuses a sign count that does not rise. Seconds since the
    // epoch always rise between runs.
    this.key.signCount = Math.max(this.key.signCount, Math.floor(Date.now() / 1000));
    const done = await auth('/auth/complete', this.key.assert(opts.challenge));
    this.token = done.token;
    this.principal = principalOf(done.token);
    return done;
  }

  // One op call. A read carries its parameters in the query, a write in a
  // JSON body. `retry` waits for the door to learn of a new membership.
  async op(name, params = {}, { write = true, space = SPACE, retry = false } = {}) {
    const deadline = Date.now() + (retry ? 30000 : 0);
    for (;;) {
      const headers = { 'X-Ordo-Space': space, Authorization: `Bearer ${this.token}` };
      let r;
      if (write) {
        headers['Content-Type'] = 'application/json';
        headers['Idempotency-Key'] = `seed-${runId}-${counter++}`;
        r = await call('POST', `/op/${name}`, { headers, body: JSON.stringify(params) });
      } else {
        r = await call('GET', `/op/${name}`, { headers, query: params });
      }
      if (r.status === 200) return r.json;
      if ((r.status === 401 || r.status === 403) && Date.now() < deadline) {
        await sleep(200);
        continue;
      }
      fail(`${name} as ${this.name} refused (${r.status}): ${r.text}`);
    }
  }
  read(name, params, opts) { return this.op(name, params, { ...opts, write: false }); }
}

// A person who is in seed-keys.json signs in. Others register.
async function person(name, { space } = {}) {
  const keys = loadKeys();
  if (keys[name]) {
    const p = new Person(name, Passkey.restore(keys[name], { origin: LOGIN }));
    await p.signIn();
    keys[name] = p.key.serialise();
    saveKeys(keys);
    return p;
  }
  const p = new Person(name, Passkey.create({ origin: LOGIN }));
  await p.register(space);
  // Save at once, so a failure later does not lose the key.
  const k = loadKeys();
  k[name] = p.key.serialise();
  saveKeys(k);
  return p;
}

// ── owner ──────────────────────────────────────────────────────────────────
async function ownerCmd() {
  const name = 'Olive (owner)';
  if (loadKeys()[name]) fail(`${name} is already in ${KEYS}. Delete the data directory to start again.`);
  // No `space`: she is the owner because the server names her, not because
  // the tenant admitted her.
  const p = await person(name);
  writeFileSync(join(DATA, 'owner'), p.principal);
  console.log(`Registered ${name} as ${p.principal}; the door restarts with her as a deployment owner.`);
}

// ── data ───────────────────────────────────────────────────────────────────
function prng(seed) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const CATEGORIES = {
  Apparel: { code: 'APP', names: ['T-shirt', 'Hoodie', 'Rain jacket', 'Wool socks', 'Linen shirt', 'Cap', 'Scarf', 'Apron'], extra: (r, pick) => ({ size: pick(['XS', 'S', 'M', 'L', 'XL']), material: pick(['cotton', 'wool', 'linen', 'nylon']) }) },
  Home: { code: 'HOM', names: ['Throw blanket', 'Cushion', 'Candle', 'Picture frame', 'Table lamp', 'Door mat', 'Vase'], extra: (r, pick) => ({ material: pick(['ceramic', 'glass', 'wool', 'wax', 'oak']), width_cm: 10 + Math.floor(r() * 90) }) },
  Kitchen: { code: 'KIT', names: ['Mug', 'Teapot', 'Chef knife', 'Cutting board', 'Water bottle', 'Mixing bowl', 'Whisk', 'Pan'], extra: (r, pick) => ({ capacity_ml: pick([250, 350, 500, 750, 1000, 1500]), material: pick(['steel', 'ceramic', 'glass', 'bamboo']) }) },
  Garden: { code: 'GAR', names: ['Trowel', 'Watering can', 'Plant pot', 'Seed tin', 'Garden gloves', 'Pruning shears', 'Bird feeder'], extra: (r, pick) => ({ material: pick(['clay', 'steel', 'plastic', 'cedar']), capacity_ml: pick([500, 2000, 5000, 9000]) }) },
  Stationery: { code: 'STA', names: ['Notebook', 'Fountain pen', 'Pencil set', 'Desk pad', 'Sticky notes', 'Planner', 'Ink bottle'], extra: (r, pick) => ({ pages: pick([48, 96, 160, 240]), size: pick(['A6', 'A5', 'A4']) }) },
  Outdoor: { code: 'OUT', names: ['Backpack', 'Tent pegs', 'Headlamp', 'Camp stove', 'Sleeping mat', 'Flask', 'Hiking poles'], extra: (r, pick) => ({ capacity_ml: pick([500, 750, 1000, 25000, 40000]), material: pick(['nylon', 'steel', 'aluminium', 'foam']) }) },
};
const COLORS = ['red', 'blue', 'green', 'black', 'white', 'yellow', 'grey'];
const CARRIERS = ['PostNorth', 'QuickParcel', 'Harbor Freight', 'RoyalDrop'];
const ADJ = ['Classic', 'Field', 'Studio', 'Harbor', 'Meadow', 'Ridge', 'Cedar', 'Linen'];

function makeProducts(n) {
  const r = prng(20260918);
  const pick = (a) => a[Math.floor(r() * a.length)];
  const cats = Object.keys(CATEGORIES);
  const out = [];
  const seen = new Set();
  for (let i = 0; i < n; i++) {
    const category = cats[i % cats.length];
    const c = CATEGORIES[category];
    const sku = `SF-${c.code}-${String(Math.floor(i / cats.length) + 1).padStart(3, '0')}`;
    let name;
    do { name = `${pick(ADJ)} ${pick(c.names)}`; } while (seen.has(name));
    seen.add(name);
    const cents = 300 + Math.floor(r() * 17700);
    // The first 6 are red, so the colour filter always has matches.
    const color = i < 6 ? 'red' : pick(COLORS);
    out.push({
      sku, name, category,
      price: `${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, '0')}`,
      stock: i % 11 === 7 ? 0 : 5 + Math.floor(r() * 56),
      attributes: { color, ...c.extra(r, pick) },
    });
  }
  return out;
}

const iso = (ms) => new Date(ms).toISOString().replace(/\.\d+Z$/, 'Z');

async function seedCmd() {
  // a. Olive opens the door to customers.
  const olive = await person('Olive (owner)');
  await olive.op('std.space.setAdmission', { open: true, role: 'editor' });

  // b. The shop.
  const shops = (await olive.read('myShops', {}, { retry: true })).rows ?? [];
  let shop = shops[0]?.id;
  if (!shop) {
    const made = await olive.op('openShop', { name: 'Shopfront' });
    shop = made.rows?.[0]?.created ?? fail(`openShop returned no id: ${JSON.stringify(made)}`);
  }

  // c. Products. addProduct converges on sku, so a re-run changes nothing.
  const products = makeProducts(45);
  const ids = [];
  for (const p of products) {
    const done = await olive.op('addProduct', { shop, ...p }, { retry: true });
    // A first run answers `created`, a re-run `converged` (same sku).
    ids.push(done.rows?.[0]?.created ?? done.rows?.[0]?.converged ?? fail(`addProduct ${p.sku} returned no id: ${JSON.stringify(done)}`));
  }

  // d. Staff.
  const sam = await person('Sam (staff)', { space: SPACE });
  const samMe = (await sam.read('me', {}, { retry: true })).rows?.[0];
  if (!samMe) fail('Sam has no member row');
  await olive.op('grantShopsAccess', { shop, member: samMe.id, level: 'staff' }, { retry: true });

  // e. Customers and their orders. 30 in all, uneven across the four.
  const names = ['Ada', 'Bo', 'Cy', 'Dee'];
  const customers = [];
  for (const n of names) {
    const c = await person(n, { space: SPACE });
    await c.read('me', {}, { retry: true });
    customers.push(c);
  }
  const existing = (await sam.read('allOrders', {}, { retry: true })).rows ?? [];
  if (existing.length === 0) {
    const r = prng(77);
    const share = [11, 3, 9, 7];
    const left = products.map((p) => p.stock);
    const start = Date.parse('2026-09-01T08:00:00Z');
    const span = Date.parse('2026-09-30T20:00:00Z') - start;
    const plan = [];
    share.forEach((count, ci) => {
      for (let k = 0; k < count; k++) plan.push({ ci, at: start + Math.floor(r() * span) });
    });
    plan.sort((a, b) => a.at - b.at);
    const notes = ['', '', '', 'Leave at the door', 'Gift, please wrap', 'Call on arrival', 'Second one for my sister'];
    for (const o of plan) {
      const quantity = 1 + Math.floor(r() * 3);
      let pi;
      do { pi = Math.floor(r() * products.length); } while (left[pi] < quantity);
      left[pi] -= quantity;
      await customers[o.ci].op('placeOrder', {
        product: ids[pi], quantity, placed: iso(o.at),
        note: notes[Math.floor(r() * notes.length)],
      });
    }
  }

  // f. Sam ships about half of them.
  const orders = (await sam.read('allOrders')).rows ?? [];
  const r2 = prng(5);
  let shipped = 0;
  for (const o of orders) {
    if (o.shipped) continue;
    if (r2() < 0.5) {
      const at = Date.parse(o.placed) + (1 + Math.floor(r2() * 3)) * 86400000;
      await sam.op('markShipped', {
        order: o.id, product: o.product, quantity: o.quantity,
        shipped: iso(at), carrier: CARRIERS[Math.floor(r2() * CARRIERS.length)],
      });
      shipped++;
    }
  }

  // g. Done.
  writeFileSync(join(DATA, 'seeded'), new Date().toISOString());
  const total = (await sam.read('allOrders')).rows?.length ?? 0;
  console.log(
    `Seeded shop ${shop}: ${ids.length} products, ${total} orders (${shipped} shipped now). ` +
    `People: ${['Olive (owner)', 'Sam (staff)', ...names].join(', ')}.`,
  );
}

// ── make-staff ─────────────────────────────────────────────────────────────
async function makeStaffCmd(member, level = 'staff') {
  if (!member) fail('usage: node seed.mjs make-staff <memberId> [staff|admin]');
  if (!['staff', 'admin'].includes(level)) fail(`level is staff or admin, not ${level}`);
  const olive = await person('Olive (owner)');
  const shop = (await olive.read('myShops', {}, { retry: true })).rows?.[0]?.id
    ?? fail('there is no shop yet: run node seed.mjs first');
  await olive.op('grantShopsAccess', { shop, member, level });
  console.log(`Granted ${member} the level ${level} on the shop.`);
}

try {
  const [cmd, ...rest] = process.argv.slice(2);
  if (cmd === 'owner') await ownerCmd();
  else if (cmd === 'make-staff') await makeStaffCmd(...rest);
  else if (!cmd) await seedCmd();
  else fail(`unknown command ${cmd}`);
} catch (e) {
  console.error(e instanceof Refused ? `seed: ${e.message}` : e);
  process.exit(1);
}
