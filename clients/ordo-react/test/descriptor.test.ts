import { describe, expect, it } from 'vitest';
import { REFUSAL_CLASSES, isRefusalClass } from '../src/refusal.js';
import { MalformedDescriptorError, SchemaVersionError, parseInterface, parseTypedId } from '../src/load.js';
import { refinementSurface } from '../src/refine.js';
import { GOLDEN, iface } from './harness.js';

describe('schema_version gate', () => {
  it('accepts the golden', () => {
    const i = iface();
    expect(i.domain).toBe('market');
    expect(i.ops.length).toBeGreaterThan(100);
  });

  it('refuses an unknown major, loudly and by name', () => {
    const doc = { ...GOLDEN, schema_version: 'ordo.interface/3' };
    expect(() => parseInterface(doc)).toThrow(SchemaVersionError);
    try {
      parseInterface(doc);
    } catch (e) {
      const err = e as SchemaVersionError;
      expect(err.found).toBe('ordo.interface/3');
      expect(err.message).toContain('newer than this package understands');
      expect(err.message).toContain('ordo.interface/2');
    }
  });

  it('refuses an older major too — a narrowed value set is as breaking as a removed key', () => {
    // `/1` is exactly this case, and not hypothetically: `call.cas` meant
    // "is a mutation" there, so a `/1` document read by this package would
    // put a CAS refusal in front of every create and every delete.
    expect(() => parseInterface({ ...GOLDEN, schema_version: 'ordo.interface/1' })).toThrow(/older/);
  });

  it('refuses a document that is not an interface descriptor at all', () => {
    expect(() => parseInterface({ ...GOLDEN, schema_version: 'ordo.surfaces/1' })).toThrow(SchemaVersionError);
  });

  it('accepts an additive minor — adding a key is not breaking', () => {
    // interface.rs: "Adding a key is not breaking: every consumer of a JSON
    // document already ignores keys it does not know."
    const doc = { ...GOLDEN, schema_version: 'ordo.interface/2.7', someNewKey: true };
    expect(parseInterface(doc).domain).toBe('market');
  });

  it('refuses a document with no version rather than assuming one', () => {
    const { schema_version: _drop, ...rest } = GOLDEN;
    expect(() => parseInterface(rest)).toThrow(MalformedDescriptorError);
  });
});

describe('typed ids', () => {
  it('splits on the first underscore, so multi-word keys survive', () => {
    // `boardsAccess` abbreviates to `boar-acce`; splitting on the last
    // underscore, or on `-`, would mis-key every one of its rows.
    expect(parseTypedId('boar-acce_01J8ZK')).toEqual({
      key: 'boar-acce',
      body: '01J8ZK',
      raw: 'boar-acce_01J8ZK',
    });
    expect(parseTypedId('proj-memb_01J8ZK')?.key).toBe('proj-memb');
    expect(parseTypedId('prod_01J8ZK')?.key).toBe('prod');
  });

  it('rejects things that are not typed ids', () => {
    expect(parseTypedId('nounderscore')).toBeNull();
    expect(parseTypedId('_leading')).toBeNull();
    expect(parseTypedId('trailing_')).toBeNull();
  });

  it('every entity key in the golden round-trips', () => {
    const i = iface();
    for (const e of i.doc.entities) {
      const parsed = parseTypedId(`${e.key}_01J8ZKQ4X7`);
      expect(parsed?.key, e.entity).toBe(e.key);
      expect(i.entityForKey(e.key)?.entity).toBe(e.entity);
    }
  });
});

describe('the descriptor value sets this package types', () => {
  const i = iface();

  it('covers every refusal class named in the golden', () => {
    const seen = new Set(i.ops.flatMap((o) => o.refusals));
    for (const cls of seen) {
      expect(isRefusalClass(cls), `${cls} is missing from REFUSAL_CLASSES`).toBe(true);
    }
    expect(seen.size).toBeGreaterThan(15);
  });

  it('has no duplicate refusal classes in the closed inventory', () => {
    expect(new Set(REFUSAL_CLASSES).size).toBe(REFUSAL_CLASSES.length);
  });

  it('sees three terminal-verb shapes, and the third is the single-row read', () => {
    const shapes = new Set(i.ops.map((o) => o.call.terminal.join('+')));
    expect([...shapes].sort()).toEqual(['commit', 'fetch+live', 'fetch+live+pages']);
  });

  it('never advertises a page a single-row op cannot serve', () => {
    // `/1`'s disagreement: `terminal` said `pages` and `paged` said `true`
    // on ops the executor answers whole (no ordering, no cursor, `next:
    // null`, `invalid_cursor` on a token). The call contract and the result
    // contract now say one thing.
    const single = i.ops.filter((o) => o.result.single_row);
    expect(single.length).toBeGreaterThan(0);
    for (const o of i.ops) {
      expect(o.call.paged, o.op).toBe(o.kind === 'read' && !o.result.single_row);
      expect(o.call.terminal.includes('pages'), o.op).toBe(o.call.paged);
    }
  });

  it('says CAS only where a statement asserts a revision', () => {
    // `/1` said `cas: true` for every mutation, so this package detected the
    // real thing from the parameter list and treated the flag as a hint.
    // Both halves are now one fact, and the fixture must contain a mutation
    // on each side or the assertion proves nothing.
    const casOps = i.ops.filter((o) => o.call.cas);
    const mutations = i.ops.filter((o) => o.kind !== 'read');
    expect(casOps.length).toBeGreaterThan(0);
    expect(casOps.length).toBeLessThan(mutations.length);
    for (const o of i.ops) {
      expect(
        o.call.cas,
        o.op,
      ).toBe(o.statements.some((s) => s.cas_param !== null));
      // A delete has no braces to put a `rev` in.
      if (o.kind === 'delete') expect(o.call.cas, o.op).toBe(false);
      // And the refusal list follows the same fact — a `cas_miss` affordance
      // on an op that cannot produce one is a merge dialog nobody can reach.
      expect(o.refusals.includes('cas_miss'), o.op).toBe(o.call.cas);
    }
  });

  it('states what a write returns, which is not the row', () => {
    for (const o of i.ops) {
      expect(o.result.shape, o.op).toBe(o.kind === 'read' ? 'rows' : 'write_summary');
      for (const s of o.statements) {
        if (s.label === 'ambiguous-identity') {
          // The one label with no summary, and rightly: rule 8 refuses a
          // statement covering two unique keys, so there is no response to
          // describe. `claimCoupon` is the fixture's witness.
          //
          // Note what this does NOT get to assert: that `error` is set. It
          // is `null` — the artifact records only read-bind failures, so a
          // write refused by rule 8 reads as callable. Left as a finding;
          // a runtime cannot pre-empt it, and the door will.
          expect(s.summary, `${o.op} / ${s.entity}`).toBeNull();
          continue;
        }
        expect(s.summary, `${o.op} / ${s.entity}`).not.toBeNull();
        expect(['id', 'count']).toContain(s.summary?.value);
        expect(s.summary?.keys.length).toBeGreaterThan(0);
      }
    }
    // The consequence a form needs: no write hands back a `rev`, so the CAS
    // loop must re-read the row. `outputs` stays empty and now says why.
    const update = i.requireOp('std.tasks.update');
    expect(update.result.shape).toBe('write_summary');
    expect(update.result.outputs).toEqual([]);
    expect(update.statements[0]?.summary).toEqual({ keys: ['updated'], value: 'count' });
    expect(i.requireOp('std.tasks.create').statements[0]?.summary).toEqual({
      keys: ['created'],
      value: 'id',
    });
  });

  it('maps kind to method with no exceptions', () => {
    for (const o of i.ops) {
      const expected = o.kind === 'read' ? 'GET' : o.kind === 'write' ? 'POST' : 'DELETE';
      expect(o.call.method, o.op).toBe(expected);
    }
  });

  it('draws filter chrome exactly where the interface says the door refines', () => {
    // `call.refinable` is a projection of the tier, and the surface follows
    // the tier: `basic` on every program read once the server derives it,
    // `none` on writes, platform ops and public reads. This holds on either
    // side of that change.
    for (const o of i.ops) {
      expect(o.call.refinable, o.op).toBe(o.refine.tier !== 'none');
      const surface = refinementSurface(o);
      expect(surface.enabled, o.op).toBe(o.refine.tier !== 'none');
      expect(surface.sortOnly, o.op).toBe(o.kind === 'read' && o.refine.tier === 'none');
    }
  });

  it('a basic read draws filter chrome; a none read draws sort-only', () => {
    const read = i.requireOp('std.tasks.list');
    const basic = refinementSurface({ ...read, refine: { ...read.refine, tier: 'basic' } });
    expect(basic.enabled).toBe(true);
    expect(basic.sortOnly).toBe(false);
    expect(basic.fields).toContain('title');
    const none = refinementSurface({ ...read, refine: { ...read.refine, tier: 'none' } });
    expect(none.enabled).toBe(false);
    expect(none.sortOnly).toBe(true);
  });
});

describe('catalog scoping is honoured, not second-guessed', () => {
  it('an absent op is undefined, and requireOp says why absence is ambiguous', () => {
    const i = iface();
    expect(i.op('nope.not.here')).toBeUndefined();
    expect(() => i.requireOp('nope.not.here')).toThrow(/may not call it/);
  });
});
