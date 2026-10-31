/**
 * C6 of `plan-client-cache.md`: the device answering new questions itself.
 *
 * The operator semantics are asserted against `refineRows`, which is pure, and
 * the store integration against `refineOp`, which reads what a synced op holds.
 */

import { LocalReplica, MemoryBackend, OrdoLocalRefusal, checkQuery, refineOp, refineRows } from '../src/index.js';
import type { LocalQuery, OpSpec, RefineCondition, Row } from '../src/index.js';

const products: OpSpec = {
  op: 'activeProducts',
  version: '0123456789abcdef',
  method: 'GET',
  kind: 'read',
  terminal: ['fetch', 'live', 'pages'],
  paged: true,
  singleRow: false,
  casParam: null,
  params: [],
  refusals: [],
  standingError: null,
  sync: { entity: 'products', key: 'id' },
  refine: { tier: 'none', fields: ['name', 'price', 'vendor', 'listed'] },
  order: { keys: [{ path: 'price', desc: true }, { path: 'id', desc: false }], stated: true },
};

// Values typed as JSON numbers, a bool as a boolean, text as a string and
// absent as null. Since 27ak `canonical()` sends money as a decimal string;
// that case has its own block below, `exact decimals`.
const rows: Row[] = [
  { id: 'p1', name: 'anvil', price: 4000, vendor: 'acme', listed: true },
  { id: 'p2', name: 'bolt', price: 250, vendor: 'acme', listed: false },
  { id: 'p3', name: 'crate', price: 12000, vendor: 'zenith', listed: true },
  { id: 'p4', name: 'dowel', price: null, vendor: null, listed: false },
];

const names = (out: readonly Row[]) => out.map((r) => r.name);

async function seeded(held: readonly Row[] = rows, holder = products.op): Promise<LocalReplica> {
  const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'usr_1');
  for (const row of held) {
    await replica.put({ entity: 'products', id: row.id as string, position: 7, row });
    await replica.hold(holder, 'products', row.id as string);
  }
  return replica;
}

describe('the Tier-0 operators, evaluated on the device', () => {
  const where = (...conditions: RefineCondition[]) => names(refineRows(rows, { where: conditions }));

  it('compares numbers numerically, not as text', () => {
    // The trap this exists to catch: "250" > "12000" as text, and money and
    // int arrive as JSON numbers precisely so they do not.
    expect(where({ field: 'price', op: 'gt', value: 1000 })).toEqual(['anvil', 'crate']);
    expect(where({ field: 'price', op: 'le', value: 4000 })).toEqual(['anvil', 'bolt']);
  });

  it('compares strings by code point, which is the order ISO dates need', () => {
    expect(where({ field: 'name', op: 'ge', value: 'c' })).toEqual(['crate', 'dowel']);
  });

  it('eq, ne and in, with two-valued null logic', () => {
    expect(where({ field: 'vendor', op: 'eq', value: 'acme' })).toEqual(['anvil', 'bolt']);
    // `dowel` has no vendor, and `null != "acme"` is true.
    expect(where({ field: 'vendor', op: 'ne', value: 'acme' })).toEqual(['crate', 'dowel']);
    expect(where({ field: 'vendor', op: 'in', value: ['zenith', 'acme'] })).toEqual(['anvil', 'bolt', 'crate']);
  });

  it('is_null finds the absent, and booleans compare', () => {
    expect(where({ field: 'price', op: 'is_null' })).toEqual(['dowel']);
    expect(where({ field: 'listed', op: 'eq', value: true })).toEqual(['anvil', 'crate']);
  });

  it('conditions combine as AND, and only AND', () => {
    expect(where({ field: 'vendor', op: 'eq', value: 'acme' }, { field: 'price', op: 'gt', value: 1000 })).toEqual([
      'anvil',
    ]);
  });

  it('a value of another type never matches an ordering test', () => {
    // A column has one type, so the server was never asked to order these.
    expect(where({ field: 'name', op: 'gt', value: 10 })).toEqual([]);
  });
});

describe('pick and sort', () => {
  it('pick narrows the columns, and an absent field reads as null', () => {
    const out = refineRows([rows[0]!], { pick: ['name', 'vendor'] });
    expect(out).toEqual([{ name: 'anvil', vendor: 'acme' }]);
  });

  it('sorts ascending and descending, with nulls last either way', () => {
    expect(names(refineRows(rows, { sort: [{ field: 'price' }] }))).toEqual(['bolt', 'anvil', 'crate', 'dowel']);
    expect(names(refineRows(rows, { sort: [{ field: 'price', direction: 'desc' }] }))).toEqual([
      'crate',
      'anvil',
      'bolt',
      'dowel',
    ]);
  });

  it('sorts by more than one key, keeping the earlier order within ties', () => {
    const out = refineRows(rows, { sort: [{ field: 'vendor' }, { field: 'price', direction: 'desc' }] });
    expect(names(out)).toEqual(['anvil', 'bolt', 'crate', 'dowel']);
  });
});

describe('what the device refuses, and what it deliberately does not', () => {
  it('refuses a field the op does not return, naming what it offers', () => {
    expect(() => checkQuery(products, { where: [{ field: 'margin', op: 'eq', value: 1 }] })).toThrowError(
      OrdoLocalRefusal,
    );
    try {
      checkQuery(products, { where: [{ field: 'margin', op: 'eq', value: 1 }] });
    } catch (e) {
      const r = e as OrdoLocalRefusal;
      expect(r.detail.class).toBe('invalid_refinement');
      expect(r.detail.param).toBe('margin');
      expect(r.detail.message).toContain('name, price, vendor, listed');
    }
  });

  it('refuses the value shapes the door refuses', () => {
    const bad = (query: LocalQuery) => () => checkQuery(products, query);
    expect(bad({ where: [{ field: 'price', op: 'is_null', value: 1 }] })).toThrowError(/takes no value/);
    expect(bad({ where: [{ field: 'price', op: 'in', value: 4000 }] })).toThrowError(/takes an array/);
    expect(bad({ where: [{ field: 'price', op: 'gt', value: null }] })).toThrowError(/non-null scalar/);
    expect(bad({ pick: [] })).toThrowError(/at least one field/);
    // A repeated pick: the door's projection would carry the column twice.
    expect(bad({ pick: ['name', 'price', 'name'] })).toThrowError(/`pick` names `name` twice/);
    // A decimal is the door's shape: a leading plus is not one.
    const exact: OpSpec = { ...products, refine: { ...products.refine, decimals: ['price'] } };
    expect(() => checkQuery(exact, { where: [{ field: 'price', op: 'gt', value: '+5' }] })).toThrowError(
      /decimal string/,
    );
    expect(() => checkQuery(exact, { where: [{ field: 'price', op: 'gt', value: '-5.25' }] })).not.toThrow();
  });

  it('refuses a write op, which is never refinable at any tier', () => {
    const write: OpSpec = { ...products, op: 'closeTask', kind: 'write' };
    expect(() => checkQuery(write, {})).toThrowError(/writes are never refinable/);
  });

  it('does NOT refuse on `refine.tier`, which is the deliberate divergence', () => {
    // Every op ships `tier: "none"`, so honouring it here would refuse the
    // whole catalog. Refining rows already on the device asks the server for
    // nothing, so the permission that governs server work does not govern it.
    // On the board as `refine-tier-local`.
    expect(products.refine.tier).toBe('none');
    expect(() => checkQuery(products, { where: [{ field: 'price', op: 'gt', value: 1 }] })).not.toThrow();
  });
});

describe('the stated order (27o)', () => {
  const byPrice = { keys: [{ path: 'price', desc: true }, { path: 'id', desc: false }], stated: true };
  const [anvil, bolt, crate, dowel] = rows as [Row, Row, Row, Row];

  it('returns rows in the order the descriptor states when the query has no sort', () => {
    expect(names(refineRows(rows, {}, byPrice))).toEqual(['crate', 'anvil', 'bolt', 'dowel']);
    expect(names(refineRows(rows, { where: [{ field: 'listed', op: 'eq', value: true }] }, byPrice))).toEqual([
      'crate',
      'anvil',
    ]);
  });

  it('keeps the rows as held when the order is not stated, or none is given', () => {
    const unstated = { keys: [{ path: 'id', desc: false }], stated: false };
    const shuffled = [crate, anvil, dowel, bolt];
    expect(names(refineRows(shuffled, {}, unstated))).toEqual(['crate', 'anvil', 'dowel', 'bolt']);
    expect(names(refineRows(shuffled, {}, null))).toEqual(['crate', 'anvil', 'dowel', 'bolt']);
    expect(names(refineRows(shuffled, {}))).toEqual(['crate', 'anvil', 'dowel', 'bolt']);
  });

  it('a sort in the query replaces the stated keys, and ties fall to the id the order names', () => {
    const keyed = { keys: [{ path: 'price', desc: true }, { path: '$id', desc: false }], stated: true };
    const tagged: Row[] = [
      { $id: 'p2', vendor: 'acme', price: 1 },
      { $id: 'p1', vendor: 'acme', price: 2 },
      { $id: 'p3', vendor: 'zenith', price: 3 },
    ];
    const out = refineRows(tagged, { sort: [{ field: 'vendor' }] }, keyed);
    expect(out.map((r) => r.$id)).toEqual(['p1', 'p2', 'p3']);
  });

  it('refineOp answers in the op’s stated order', async () => {
    const replica = await seeded([bolt, dowel, anvil, crate]);
    const out = await refineOp(replica, products);
    expect(names(out)).toEqual(['crate', 'anvil', 'bolt', 'dowel']);
  });
});

describe('answering from the rows an op holds', () => {
  it('reads only what that op holds, and answers without a request', async () => {
    const replica = await seeded();
    const out = await refineOp(replica, products, {
      where: [{ field: 'vendor', op: 'eq', value: 'acme' }],
      sort: [{ field: 'price', direction: 'desc' }],
      pick: ['name', 'price'],
    });
    expect(out).toEqual([
      { name: 'anvil', price: 4000 },
      { name: 'bolt', price: 250 },
    ]);
  });

  it('another op’s rows are not in the answer', async () => {
    const replica = await seeded([rows[0]!]);
    // A second op holds a row of the same entity; it is not this op's result.
    await replica.put({ entity: 'products', id: 'p9', position: 7, row: { id: 'p9', name: 'zzz', price: 1 } });
    await replica.hold('someOtherOp', 'products', 'p9');

    expect(names(await refineOp(replica, products))).toEqual(['anvil']);
  });

  it('holds rows apart by holder, so one op at two parameter sets answers separately', async () => {
    const replica = await seeded([rows[0]!], 'activeProducts?vendor=acme');
    expect(names(await refineOp(replica, products, {}, 'activeProducts?vendor=acme'))).toEqual(['anvil']);
    expect(await refineOp(replica, products)).toEqual([]);
  });

  it('refuses an op the device cannot sync, since it holds no rows of its own', async () => {
    const replica = await seeded();
    await expect(refineOp(replica, { ...products, sync: null })).rejects.toThrowError(/not synced/);
  });
});

describe('the generated table carries the refinable manifest', () => {
  it('a read op names its refinable fields, whatever the tier says', async () => {
    const { SPECS } = await import('../src/generated/index.js');
    // `$id` is in the list since 2026-09-17 (C4e): the anchor's id is a
    // genuine projected output now, so it reaches `refine.fields` and the
    // door's own field manifest through the same list, and the two agree
    // without either side special-casing it. Filtering a synced list by row
    // id is a reasonable thing to permit.
    expect(SPECS.activeProducts.refine).toMatchObject({
      fields: ['name', 'price', 'vendor', 'category', '$id'],
      // `price` is money: a decimal string on the wire, compared exactly.
      decimals: ['price'],
      types: { name: 'text', price: 'money', vendor: 'id', category: 'id', $id: 'id' },
      sealed: [],
    });
    // The tier is the door's permission. The device reads `fields` whatever
    // it says, so this test holds before and after the server derives it.
    expect(['none', 'basic']).toContain(SPECS.allTasks.refine.tier);
    expect(SPECS.allTasks.refine.fields).toEqual(['title', 'status', '$id']);
  });
});

// Money and `decimal` arrive as exact decimal strings (27ak). The spec names
// them, and they compare as numbers, exactly, as Postgres's `numeric` does.
describe('exact decimals', () => {
  const priced: Row[] = [
    { id: 'p1', name: 'anvil', price: '40.00' },
    { id: 'p2', name: 'bolt', price: '2.50' },
    { id: 'p3', name: 'crate', price: '120.00' },
    { id: 'p4', name: 'dowel', price: null },
    { id: 'p5', name: 'eyelet', price: '-0.05' },
  ];
  const exact = ['price'];
  const where = (...conditions: RefineCondition[]) =>
    names(refineRows(priced, { where: conditions }, null, exact));

  it('orders as numbers, never by code point', () => {
    // By code point "120.00" < "2.50" < "40.00"; as numbers it is the other way.
    expect(where({ field: 'price', op: 'gt', value: '10' })).toEqual(['anvil', 'crate']);
    expect(where({ field: 'price', op: 'lt', value: '0' })).toEqual(['eyelet']);
    expect(names(refineRows(priced, { sort: [{ field: 'price' }] }, null, exact))).toEqual([
      'eyelet',
      'bolt',
      'anvil',
      'crate',
      'dowel',
    ]);
  });

  it('equality is by value: "2.5" is "2.50"', () => {
    expect(where({ field: 'price', op: 'eq', value: '2.5' })).toEqual(['bolt']);
    expect(where({ field: 'price', op: 'in', value: ['40', null] })).toEqual(['anvil', 'dowel']);
    expect(where({ field: 'price', op: 'ne', value: '2.500' })).toEqual(['anvil', 'crate', 'dowel', 'eyelet']);
    expect(where({ field: 'price', op: 'eq', value: 40 })).toEqual(['anvil']);
  });

  it('a float filter value is refused', () => {
    const spec: OpSpec = { ...products, refine: { tier: 'none', fields: ['price'], decimals: ['price'] } };
    expect(() => checkQuery(spec, { where: [{ field: 'price', op: 'gt', value: 2.5 }] })).toThrowError(
      /never a float/,
    );
    expect(() => checkQuery(spec, { where: [{ field: 'price', op: 'in', value: ['1', 0.1] }] })).toThrowError(
      /never a float/,
    );
    expect(() => checkQuery(spec, { where: [{ field: 'price', op: 'gt', value: '2.50' }] })).not.toThrow();
  });
});
