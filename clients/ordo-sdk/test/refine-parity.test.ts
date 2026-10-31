/**
 * Device and server parity for local refinement (checklist 25k and 25l).
 *
 * `refineRows` must answer a filter or a sort exactly as the server does. The
 * case table is `crates/ordo-conformance/fixtures/refine-parity.json`, shared
 * with `crates/ordo-conformance/tests/refine_parity.rs`. That test runs every
 * case through the real read path on Postgres and requires the ids recorded
 * here, so the expected answers below are proved on the server. This test runs
 * the same cases through the device and requires the same ids.
 */

import { readFileSync } from 'node:fs';

import { ReadCall, refineMerge, refineRows, refineRowsAsync } from '../src/index.js';
import { compareText } from '../src/replica/refine.js';
import type { LocalQuery, OpSpec, RefineCondition, Row } from '../src/index.js';
import { MockDoor, q } from './harness.js';

interface ParityCase {
  readonly name: string;
  readonly where: readonly RefineCondition[];
  readonly sort: readonly { field: string; direction: 'asc' | 'desc' }[];
  readonly expect: readonly string[];
}

const table = JSON.parse(
  readFileSync(new URL('../../../crates/ordo-conformance/fixtures/refine-parity.json', import.meta.url), 'utf8'),
) as { rows: Row[]; cases: ParityCase[]; decimals: string[] };

describe('the device answers as the server does', () => {
  it('has a table worth running', () => {
    expect(table.cases.length).toBeGreaterThan(40);
    expect(new Set(table.cases.map((c) => c.name)).size).toBe(table.cases.length);
  });

  it.each(table.cases.map((c) => [c.name, c] as const))('%s', (_name, c) => {
    const query: LocalQuery = {
      where: c.where,
      sort: c.sort,
    };
    // `m` and `r` are money and decimal: exact decimal strings (27ak).
    const got = refineRows(table.rows, query, null, table.decimals).map((r) => r.id);
    expect(got).toEqual(c.expect);
  });
});

/**
 * The same cases through the sliced path and the merge path (H2,
 * `plan-client-loading-fixes.md` §2.1): each must give the server's ids.
 */
describe('the sliced and merged refinements answer as the server does', () => {
  const now = () => Promise.resolve();

  it.each(table.cases.map((c) => [c.name, c] as const))('%s, sliced', async (_name, c) => {
    // Slices of 3 rows, so every case filters, sorts and merges across slices.
    const got = await refineRowsAsync(table.rows, { where: c.where, sort: c.sort }, null, table.decimals, {
      chunk: 3,
      yieldTo: now,
    });
    expect(got.map((r) => r.id)).toEqual(c.expect);
  });

  it.each(table.cases.map((c) => [c.name, c] as const))('%s, merged', (_name, c) => {
    const query: LocalQuery = { where: c.where, sort: c.sort };
    // Half the rows answered, then the other half arrives as a delta.
    const half = Math.floor(table.rows.length / 2);
    const first = refineRows(table.rows.slice(0, half), query, null, table.decimals);
    const got = refineMerge(first, table.rows.slice(half), new Set(), query, null, table.decimals);
    expect(got.map((r) => r.id)).toEqual(c.expect);
    // Every row answered, then one removed and put back as a new object.
    const all = refineRows(table.rows, query, null, table.decimals);
    const victim = table.rows[1]!;
    const again = refineMerge(all, [{ ...victim }], new Set([victim]), query, null, table.decimals);
    expect(again.map((r) => r.id)).toEqual(c.expect);
  });
});

/**
 * The same query object on both paths. A case's conditions are built with the
 * typed proxy, checked in door mode, and sent through a mock door. The
 * envelope the door would receive is decoded and run over the cached rows,
 * and must give the ids the server gives. So the builder, the wire form and
 * the device copy agree on every case the server proved.
 */
describe('a builder query filters cached rows as the server does', () => {
  // The parity table's columns, as an op's spec states them.
  const spec: OpSpec = {
    op: 'probes',
    version: '0123456789abcdef',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'pages'],
    paged: true,
    singleRow: false,
    casParam: null,
    params: [],
    refusals: [],
    standingError: null,
    sync: { entity: 'probes', key: 'id' },
    refine: {
      tier: 'basic',
      fields: ['id', 'n', 't', 'b', 'm', 'r'],
      decimals: table.decimals,
      types: { id: 'id', n: 'int', t: 'text', b: 'bool', m: 'money', r: 'decimal' },
      sealed: [],
      limits: null,
    },
    order: null,
  };

  it.each(table.cases.map((c) => [c.name, c] as const))('%s', async (_name, c) => {
    const door = new MockDoor().reply({ json: { rows: [] } });
    const call = new ReadCall(door.client(), spec, {}).where((v) =>
      c.where.map((w) => {
        const f = v[w.field]!;
        if (w.op === 'is_null') return f.isNull();
        if (w.op === 'in') return f.in(w.value as unknown[]);
        return f[w.op](w.value);
      }),
    );
    // The builder holds the case's own conditions, in the wire shape, with
    // text made NFC as the door makes it.
    const nfc = (x: unknown): unknown =>
      typeof x === 'string' ? x.normalize('NFC') : Array.isArray(x) ? x.map(nfc) : x;
    expect(call.refinement.where ?? []).toEqual(
      c.where.map((w) => (w.op === 'is_null' ? { field: w.field, op: w.op } : { ...w, value: nfc(w.value) })),
    );

    await call.fetch();
    const wire = JSON.parse(q(door.calls[0]!.url).get('$refine') ?? '{}') as LocalQuery;
    const got = refineRows(table.rows, { ...wire, sort: c.sort }, null, table.decimals).map((r) => r.id);
    expect(got).toEqual(c.expect);
  });

  it('a pick keeps the named columns, in the order named, on both paths', async () => {
    const door = new MockDoor().reply({ json: { rows: [] } });
    const call = new ReadCall(door.client(), spec, {}).pick('t', 'id');
    await call.fetch();
    const wire = JSON.parse(q(door.calls[0]!.url).get('$refine') ?? '{}') as LocalQuery;
    expect(wire).toEqual({ pick: ['t', 'id'] });
    const first = refineRows(table.rows, wire, null, table.decimals)[0]!;
    expect(Object.keys(first)).toEqual(['t', 'id']);
  });
});

// The server has no spelling for these, so the table cannot carry them.
describe('device-only corners', () => {
  it('an empty `in` list is false for every row, as the language says', () => {
    const where = [{ field: 'n', op: 'in', value: [] }] as const;
    expect(refineRows(table.rows, { where })).toEqual([]);
  });

  it('a sort breaks ties by id, whatever order the rows were held in', () => {
    const held = [...table.rows].reverse();
    const got = refineRows(held, { sort: [{ field: 'n' }] }).map((r) => r.id);
    const want = refineRows(table.rows, { sort: [{ field: 'n' }] }).map((r) => r.id);
    expect(got).toEqual(want);
  });

  it('compareText is code point order', () => {
    const emoji = '\u{1F600}';
    expect(compareText(emoji, '')).toBeGreaterThan(0);
    expect(compareText('', emoji)).toBeLessThan(0);
    expect(compareText('퟿', emoji)).toBeLessThan(0);
    expect(compareText('ab', 'abc')).toBeLessThan(0);
    expect(compareText('Z', 'a')).toBeLessThan(0);
    expect(compareText('x', 'x')).toBe(0);
  });

  it('negative decimals, and one value at two scales, order as numbers', () => {
    const rows = [
      { id: 'a', m: '-1.5' },
      { id: 'b', m: '1.50' },
      { id: 'c', m: '-0.25' },
      { id: 'd', m: '1.5' },
      { id: 'e', m: '-10' },
      { id: 'f', m: 0 },
    ];
    const ids = (direction: 'asc' | 'desc') =>
      refineRows(rows, { sort: [{ field: 'm', direction }] }, null, ['m']).map((r) => r.id);
    expect(ids('asc')).toEqual(['e', 'a', 'c', 'f', 'b', 'd']);
    expect(ids('desc')).toEqual(['b', 'd', 'f', 'c', 'a', 'e']);
  });

  it('decimals too wide for a double still order exactly', () => {
    const rows = [
      { id: 'a', m: '12345678901234567890.01' },
      { id: 'b', m: '12345678901234567890.001' },
      { id: 'c', m: '-12345678901234567890' },
    ];
    expect(refineRows(rows, { sort: [{ field: 'm' }] }, null, ['m']).map((r) => r.id)).toEqual(['c', 'b', 'a']);
  });

  it('nulls go last in every key of a composite sort, either way', () => {
    const rows = [
      { id: 'a', t: null, n: 1 },
      { id: 'b', t: 'x', n: null },
      { id: 'c', t: 'x', n: 2 },
      { id: 'd', t: 'w', n: null },
      { id: 'e', t: null, n: null },
    ];
    const asc = refineRows(rows, { sort: [{ field: 't' }, { field: 'n' }] }).map((r) => r.id);
    expect(asc).toEqual(['d', 'c', 'b', 'a', 'e']);
    const desc = refineRows(rows, {
      sort: [
        { field: 't', direction: 'desc' },
        { field: 'n', direction: 'desc' },
      ],
    }).map((r) => r.id);
    expect(desc).toEqual(['c', 'b', 'd', 'a', 'e']);
  });

  it('rows already in order come back in it: a new array of the same rows', () => {
    const rows = table.rows.slice().sort((a, b) => (String(a.id) < String(b.id) ? -1 : 1));
    const got = refineRows(rows, {}, { keys: [{ path: 'id', desc: false }], stated: true });
    expect(got).toEqual(rows);
    expect(got).not.toBe(rows);
    got.forEach((r, i) => expect(r).toBe(rows[i]));
  });

  it('text outside the basic plane sorts by code point in a full sort', () => {
    const rows = [
      { id: 'a', t: '\u{1F600}' },
      { id: 'b', t: '\uFF21' },
      { id: 'c', t: 'z' },
    ];
    expect(refineRows(rows, { sort: [{ field: 't' }] }).map((r) => r.id)).toEqual(['c', 'b', 'a']);
  });

  it('a merge with nothing changed gives back the same array', () => {
    const all = refineRows(table.rows, { sort: [{ field: 'n' }] });
    expect(refineMerge(all, [], new Set(), { sort: [{ field: 'n' }] })).toBe(all);
  });

  it('an aborted sliced refinement rejects with the reason', async () => {
    const controller = new AbortController();
    const rows = Array.from({ length: 50 }, (_, i) => ({ id: `r${String(i).padStart(3, '0')}`, n: 50 - i }));
    const pending = refineRowsAsync(rows, { sort: [{ field: 'n' }] }, null, [], {
      chunk: 5,
      signal: controller.signal,
      yieldTo: () => Promise.resolve(),
    });
    controller.abort(new Error('a newer query'));
    await expect(pending).rejects.toThrow('a newer query');
  });
});
