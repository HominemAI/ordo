/**
 * **The exhaustive-match ratchet, crossing the language boundary.**
 *
 * `crates/ordo-gen/src/spell.rs` decides, for every reserved word in the
 * language, either the name it has in TypeScript or the reason it has none —
 * with no wildcard arm, so a word added to `token.rs` fails `cargo build`
 * until somebody answers. That decision is emitted into
 * `src/generated/vocabulary.ts` as `KEYWORDS`.
 *
 * This file closes the loop. For every word tagged `offered`, the name it
 * claims must actually resolve on this package's surface. Without it, half
 * the ratchet would be theatre: a new keyword could be given a spelling in
 * Rust, published in the manifest, documented in the README — and never
 * implemented, with everything still green.
 *
 * So:
 *
 * - `cargo build` fails if a word has no decision;
 * - `npm run check` fails if a decision has no implementation;
 * - and the corpus (`corpus.test.ts` + `isomorphism.rs`) fails if an
 *   implementation says something the parser refuses.
 *
 * Three gates, and a new word has to pass all three.
 */

import { describe, expect, it } from 'vitest';

import * as pkg from '../src/index.js';
import {
  AGGREGATES,
  ALLOCATE_MODES,
  DURATION_UNITS,
  FUNCTIONS,
  KEYWORDS,
  OPERATORS,
  SCALARS,
  agg,
  f,
  fn,
  lit,
  t,
  type Word,
} from '../src/generated/index.js';
import { Body, DelStmt, Entity, Op, Q, SetStmt, Src, TypeSpec } from '../src/core/build.js';
import { Cond, ValueBase } from '../src/core/value.js';
import { g } from '../src/core/guard.js';

/** Everything a claimed spelling may resolve against. */
const PROTOTYPES: Record<string, object> = {
  Q: Q.prototype,
  Src: Src.prototype,
  Entity: Entity.prototype,
  Op: Op.prototype,
  Body: Body.prototype,
  SetStmt: SetStmt.prototype,
  DelStmt: DelStmt.prototype,
  TypeSpec: TypeSpec.prototype,
  Value: Object.getPrototypeOf(lit(1)) as object,
  ValueBase: ValueBase.prototype,
  Cond: Cond.prototype,
};

const NAMESPACES: Record<string, object> = { agg, fn, t, g };

/**
 * The candidate names a spelling claims.
 *
 * A spelling is written the way an author reads it — `.group`, `w.set`,
 * `agg.distinct`, `let_`, `.matchAny/.matchNone/.matchRef/.matchOn` — because
 * its first job is to be legible in the manifest. This turns that into the
 * identifiers to look for.
 */
function claimedNames(spelling: string): string[] {
  return spelling
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => {
      const call = part.replace(/\(.*$/, '');
      const segments = call.split('.').filter((x) => x.length > 0);
      return segments[segments.length - 1] ?? '';
    })
    .filter((n) => n.length > 0);
}

function resolves(name: string): boolean {
  if (name in (pkg as Record<string, unknown>)) return true;
  for (const proto of Object.values(PROTOTYPES)) {
    if (name in proto) return true;
  }
  for (const ns of Object.values(NAMESPACES)) {
    if (name in ns) return true;
  }
  return false;
}

const offered = KEYWORDS.filter((w: Word) => w.tag === 'offered');

describe('every offered word resolves on the surface', () => {
  it('the manifest is not empty', () => {
    // Non-vacuity: an empty `KEYWORDS` would make every case below pass
    // while checking nothing, which is the exact failure a two-directional
    // inventory exists to prevent.
    expect(KEYWORDS.length).toBeGreaterThan(50);
    expect(offered.length).toBeGreaterThan(25);
  });

  for (const word of offered) {
    it(`\`${word.word}\` → ${word.note}`, () => {
      const names = claimedNames(word.note);
      expect(names.length).toBeGreaterThan(0);
      for (const name of names) {
        expect(
          resolves(name),
          `the generator says the reserved word \`${word.word}\` is spelled ` +
            `\`${word.note}\` in TypeScript, and \`${name}\` resolves nowhere on ` +
            'this package. Either implement it in `src/core/` (if it is a shape) ' +
            'or in the generator (if it is vocabulary) — or change the decision in ' +
            '`crates/ordo-gen/src/spell.rs` to record that the builder does not ' +
            'offer it, with the reason.',
        ).toBe(true);
      }
    });
  }
});

describe('every absent word says why', () => {
  for (const word of KEYWORDS.filter((w: Word) => w.tag !== 'offered' && w.tag !== 'fused')) {
    it(`\`${word.word}\` is ${word.tag}`, () => {
      // The two absences are owed differently, and conflating them turns a
      // deliberate exclusion into a work item (or hides a real one).
      expect(word.note.length).toBeGreaterThan(20);
      expect(word.tag === 'does-not-parse' || word.tag === 'out-of-scope').toBe(true);
    });
  }

  it('the out-of-scope words are `whole` and `filled`', () => {
    // A tripwire, not a rule: `out-of-scope` is a debt, and the moment a
    // second one appears somebody has to decide whether the README's
    // exclusions section still tells the truth.
    const scoped = KEYWORDS.filter((w: Word) => w.tag === 'out-of-scope').map((w) => w.word);
    expect(scoped).toEqual(['whole', 'filled']);
  });
});

describe('the borrowed closed sets are whole', () => {
  it('the duration units are the eight', () => {
    expect([...DURATION_UNITS]).toEqual(['ms', 's', 'm', 'h', 'd', 'w', 'mo', 'y']);
  });

  it('`rotation` is not an allocation mode', () => {
    expect(ALLOCATE_MODES).not.toContain('rotation');
    expect(ALLOCATE_MODES.length).toBeGreaterThan(0);
  });

  it('every expression function builds a call at every legal arity', () => {
    // Called rather than measured. A TypeScript optional parameter compiles
    // to an ordinary one, so `Function.length` reports the MAXIMUM and would
    // have made this test assert the wrong number for every variadic
    // function — which it did, until `round` said so.
    for (const spec of FUNCTIONS) {
      const impl = (fn as Record<string, unknown>)[spec.name];
      expect(typeof impl, `fn.${spec.name} is missing`).toBe('function');
      const call = impl as (...a: unknown[]) => { node: { k: string; args: unknown[] } };
      for (let n = spec.min; n <= spec.max; n++) {
        // `allocate`'s last argument is a MODE WORD, not a value: the parser
        // checks the written form against a closed set, so the builder types
        // it as that union and passes the word through.
        const args = Array.from({ length: n }, (_, i) =>
          spec.name === 'allocate' && i === 2 ? ALLOCATE_MODES[0] : lit(1),
        );
        const built = call(...args);
        expect(built.node.k, `fn.${spec.name}(${n} args)`).toBe('call');
        expect(built.node.args.length, `fn.${spec.name}(${n} args)`).toBe(n);
      }
    }
  });

  it('every aggregate has a builder', () => {
    for (const spec of AGGREGATES) {
      expect(typeof (agg as Record<string, unknown>)[spec.name], `agg.${spec.name}`).toBe(
        'function',
      );
    }
  });

  it('every declarable scalar has a constructor, `decimal(p)` and `money(p)` included', () => {
    for (const s of SCALARS) {
      expect(typeof (t as Record<string, unknown>)[s.ctor], `t.${s.ctor}`).toBe('function');
    }
    // Storable since 27ak: a rate declares its places, and money may.
    expect(SCALARS.map((s) => s.ordo)).toContain('decimal');
    expect(t.decimal(4).kind).toEqual({ k: 'scalar', ty: { name: 'decimal', places: 4 } });
    expect(t.money(2).kind).toEqual({ k: 'scalar', ty: { name: 'money', places: 2 } });
    expect(t.money().kind).toEqual({ k: 'scalar', ty: { name: 'money' } });
  });

  it('every operator method exists on a value', () => {
    const v = lit(1);
    for (const op of OPERATORS.compare) {
      expect(typeof (v as unknown as Record<string, unknown>)[op.method]).toBe('function');
    }
    for (const op of OPERATORS.text) {
      expect(typeof (v as unknown as Record<string, unknown>)[op.method]).toBe('function');
    }
    for (const op of OPERATORS.arithmetic) {
      expect(typeof (v as unknown as Record<string, unknown>)[op.method]).toBe('function');
    }
  });
});

describe('the proxy does not swallow the vocabulary', () => {
  it('`f.total` is a path and `f.total.eq` is the operator', () => {
    expect(f.total.node).toEqual({ k: 'path', segments: ['total'] });
    expect(f.total.eq(lit(1)).node).toEqual({
      k: 'cmp',
      op: '==',
      l: { k: 'path', segments: ['total'] },
      r: { k: 'int', v: '1' },
    });
  });

  it('`f.customer.name` dots through', () => {
    expect(f.customer.name.node).toEqual({ k: 'path', segments: ['customer', 'name'] });
  });

  it('the collision is only ever on a SECOND segment', () => {
    // Worth stating precisely, because the obvious guess is wrong. `f.eq` is
    // a field called `eq`: the top-level namespace has no operators on it, so
    // there is nothing to collide with. The shadowing only starts once a
    // path exists to extend — `f.total.eq` is the comparison, so a field
    // genuinely called `eq` UNDER `total` is the case that needs the escape.
    expect(f.eq.node).toEqual({ k: 'path', segments: ['eq'] });
    expect(typeof f.total.eq).toBe('function');
    expect(pkg.path('total', 'eq').node).toEqual({ k: 'path', segments: ['total', 'eq'] });
  });
});
