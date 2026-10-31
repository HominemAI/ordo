// GENERATED — do not edit; regenerate with `ordo gen ts`.
//
// The builder's language vocabulary: values, types, functions, aggregates.
//
// Source of truth: the parser's own closed tables in `crates/ordo-syntax` (`Keyword`, `ScalarTy`, `CmpOp`, `TextOp`, `BinOp`, `DurationUnit`, `functions()`, `AGGREGATE_SHAPES`, `ALLOCATE_MODES`)
//
// This file is the GENERATED half of this package. The hand-written
// half is under `src/core/` and is versioned by hand; the boundary
// between them is documented in the package README. An edit here is
// lost the next time the generator runs.

import type { AggNode, CondNode, ExprNode } from '../core/ir.js';
import { TypeSpec, ref } from '../core/build.js';
import {
  Cond,
  installValue,
  intDigits,
  makeFields,
  makePath,
  reserveAccessors,
  ValueBase,
  type Value,
} from '../core/value.js';

/** `ms s m h d w mo y` — the closed unit set. */
export type DurationUnit = 'ms' | 's' | 'm' | 'h' | 'd' | 'w' | 'mo' | 'y';

/** `allocate`'s optional third argument. */
export type AllocateMode = 'largestRemainder' | 'first' | 'last' | 'largestShare';

/**
 * A value: an expression node with the language's operators on it.
 *
 * Every method below is generated from a closed enum in the parser, so the builder has exactly the operators the language has. There is no escape hatch into an arbitrary predicate, because there could not be one that compiled.
 */
/**
 * The vocabulary, merged into the skeleton's `Value`.
 *
 * `core/value.ts` declares `Value` with no members so that the whole 
 * skeleton — parameters, field paths, sort keys, write values — can be 
 * typed against it without naming one operator. This block is where the 
 * operators arrive, and it is generated from the parser's enums, so the 
 * two halves cannot disagree about what a value can do.
 */
declare module '../core/value.js' {
  // eslint-disable-next-line @typescript-eslint/no-shadow
  interface Value {
    /** `a == b` */
    eq(other: Value): Cond;
    /** `a != b` */
    ne(other: Value): Cond;
    /** `a < b` */
    lt(other: Value): Cond;
    /** `a <= b` */
    le(other: Value): Cond;
    /** `a > b` */
    gt(other: Value): Cond;
    /** `a >= b` */
    ge(other: Value): Cond;
    /** `a contains b` */
    contains(other: Value): Cond;
    /** `a starts b` */
    starts(other: Value): Cond;
    /** `a matches b` */
    matches(other: Value): Cond;
    /** `a + b` */
    add(other: Value): Value;
    /** `a - b` */
    sub(other: Value): Value;
    /** `a * b` */
    mul(other: Value): Value;
    /** `a / b` */
    div(other: Value): Value;
    /** `-x` */
    neg(): Value;
    /** `expr in ($a, $b)` */
    in(...items: Value[]): Cond;
    /** `expr is null` */
    isNull(): Cond;
    /** `expr is not null` */
    isNotNull(): Cond;
    /** `expr since 90d` */
    since(dur: Value): Cond;
    /** `expr under $id` */
    under(id: Value): Cond;
    /** `expr above $id` */
    above(id: Value): Cond;
  }
}

export class Val extends ValueBase implements Value {
  /** `a == b` */
  eq(other: Value): Cond {
    return new Cond({ k: 'cmp', op: '==', l: this.node, r: other.node });
  }

  /** `a != b` */
  ne(other: Value): Cond {
    return new Cond({ k: 'cmp', op: '!=', l: this.node, r: other.node });
  }

  /** `a < b` */
  lt(other: Value): Cond {
    return new Cond({ k: 'cmp', op: '<', l: this.node, r: other.node });
  }

  /** `a <= b` */
  le(other: Value): Cond {
    return new Cond({ k: 'cmp', op: '<=', l: this.node, r: other.node });
  }

  /** `a > b` */
  gt(other: Value): Cond {
    return new Cond({ k: 'cmp', op: '>', l: this.node, r: other.node });
  }

  /** `a >= b` */
  ge(other: Value): Cond {
    return new Cond({ k: 'cmp', op: '>=', l: this.node, r: other.node });
  }

  /** `a contains b` — matching stays in operators, never functions. */
  contains(other: Value): Cond {
    return new Cond({ k: 'text', op: 'contains', l: this.node, r: other.node });
  }

  /** `a starts b` — matching stays in operators, never functions. */
  starts(other: Value): Cond {
    return new Cond({ k: 'text', op: 'starts', l: this.node, r: other.node });
  }

  /** `a matches b` — matching stays in operators, never functions. */
  matches(other: Value): Cond {
    return new Cond({ k: 'text', op: 'matches', l: this.node, r: other.node });
  }

  /** `a + b` */
  add(other: Value): Value {
    return new Val({ k: 'bin', op: '+', l: this.node, r: other.node });
  }

  /** `a - b` */
  sub(other: Value): Value {
    return new Val({ k: 'bin', op: '-', l: this.node, r: other.node });
  }

  /** `a * b` */
  mul(other: Value): Value {
    return new Val({ k: 'bin', op: '*', l: this.node, r: other.node });
  }

  /** `a / b` */
  div(other: Value): Value {
    return new Val({ k: 'bin', op: '/', l: this.node, r: other.node });
  }

  /** `-x`. */
  neg(): Value {
    return new Val({ k: 'neg', e: this.node });
  }

/** `expr in ($a, $b)` — membership over a parenthesized comma list. */
  in(...items: Value[]): Cond {
    return new Cond({ k: 'in', e: this.node, list: items.map((i) => i.node) });
  }

/** `expr is null` — one of the only two null tests the language has. */
  isNull(): Cond {
    return new Cond({ k: 'isnull', e: this.node, negated: false });
  }

/** `expr is not null` — the other. */
  isNotNull(): Cond {
    return new Cond({ k: 'isnull', e: this.node, negated: true });
  }

/** `expr since 90d` — a trailing window of execution time. */
  since(dur: Value): Cond {
    return new Cond({ k: 'since', e: this.node, dur: dur.node });
  }

/** `expr under $id` — tree entities only. */
  under(id: Value): Cond {
    return new Cond({ k: 'under', e: this.node, id: id.node });
  }

/** `expr above $id` — tree entities only. */
  above(id: Value): Cond {
    return new Cond({ k: 'above', e: this.node, id: id.node });
  }
}

// The run-time half of the seam: every path and parameter the skeleton
// mints from here on is built with this constructor, so a value typed
// as having `.eq()` really has it.
installValue(Val);

/**
 * The method names `f.…` must not shadow.
 *
 * A field genuinely called `eq` or `in` is unreachable through the proxy and is spelled `path('eq')` instead. The list is generated from the same tables the methods are, so it can never be short.
 */
export const VALUE_METHODS = [
  'eq',
  'ne',
  'lt',
  'le',
  'gt',
  'ge',
  'contains',
  'starts',
  'matches',
  'add',
  'sub',
  'mul',
  'div',
  'neg',
  'in',
  'isNull',
  'isNotNull',
  'since',
  'under',
  'above',
  'node',
] as const;
reserveAccessors(VALUE_METHODS);

/**
 * `f` — the field namespace. `f.total`, `f.customer.name`.
 *
 * Untyped against any particular schema. `src/generated/schema.ts` 
 * re-types this same object against one `interface.json`, so an author 
 * with a descriptor gets completion and ref-following.
 */
export const f = makeFields();

/** The escape when a field name collides with a method name. */
export function path(...segments: string[]): Value {
  return makePath(segments.flatMap((s) => s.split('.')));
}

/**
 * `lit(3)`, `lit("open")`, `lit(true)` — the literals whose Ordo type 
 * a JavaScript value already determines.
 *
 * Integers pass through {@link intDigits}, which refuses a `number` 
 * that IEEE would have rounded: Ordo's `int` is exact, so a literal 
 * above 2^53 must arrive as a bigint or a digit string.
 */
export function lit(v: number | bigint | string | boolean): Value {
  if (typeof v === 'boolean') return new Val({ k: 'bool', v });
  if (typeof v === 'string') return new Val({ k: 'str', v });
  return new Val({ k: 'int', v: intDigits(v) });
}

/** `10.50` — exact source digits; arithmetic is exact decimal, no float exists. */
export function decLit(digits: string): Value {
  if (!/^-?\d+\.\d+$/.test(digits)) {
    throw new Error(`not a decimal literal: ${digits}`);
  }
  return new Val({ k: 'dec', v: digits });
}

/** `2026-07-01` — a bare ISO date. */
export function dateLit(iso: string): Value {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(iso)) throw new Error(`not an ISO date: ${iso}`);
  return new Val({ k: 'date', v: iso });
}

/** `2026-07-01T14:30:00Z` — UTC only; the `Z` is mandatory. */
export function datetimeLit(iso: string): Value {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(iso)) {
    throw new Error(`not an ISO datetime with a mandatory Z: ${iso}`);
  }
  return new Val({ k: 'datetime', v: iso });
}

/** `90d`, `5m`, `1mo`, `500ms`. */
export function durationLit(n: number | bigint | string, unit: DurationUnit): Value {
  return new Val({ k: 'dur', n: intDigits(n), unit });
}

/** `null`. */
export function nul(): Value {
  return new Val({ k: 'null' });
}

/**
 * `a and b` — a same-connective chain.
 *
 * Mixing `and` and `or` without parentheses does not parse, and the 
 * printer parenthesizes a nested chain for exactly that reason.
 */
export function and(...items: Cond[]): Cond {
  if (items.length === 0) throw new Error('`and` needs at least one condition');
  return new Cond({ k: 'and', items: items.map((i) => i.node) });
}

/**
 * `a or b` — a same-connective chain.
 *
 * Mixing `and` and `or` without parentheses does not parse, and the 
 * printer parenthesizes a nested chain for exactly that reason.
 */
export function or(...items: Cond[]): Cond {
  if (items.length === 0) throw new Error('`or` needs at least one condition');
  return new Cond({ k: 'or', items: items.map((i) => i.node) });
}

/** `not <atom>` — binds to a comparison or a parenthesized group. */
export function not(inner: Cond): Cond {
  return new Cond({ k: 'not', inner: inner.node });
}

/** A bare boolean field as a complete condition (G7). */
export function bare(v: Value): Cond {
  return new Cond({ k: 'bare', e: v.node });
}

/**
 * `cond(c1 -> v1, c2 -> v2, …, velse)` — the only conditional.
 *
 * The final bare else is mandatory (totality) and is therefore a 
 * required argument rather than an option.
 */
export function cond(
  arms: readonly (readonly [Cond, Val])[],
  otherwise: Value,
): Value {
  if (arms.length === 0) throw new Error('`cond` needs at least one arm');
  return new Val({
    k: 'cond',
    arms: arms.map(([c, v]) => ({ c: c.node, v: v.node })),
    else: otherwise.node,
  });
}

/**
 * `t` — the declared types.
 *
 * Generated from `ScalarTy` by an exhaustive match: every declarable 
 * type has a constructor, `t.decimal(places)` included since 27ak.
 */
export const t = {
  /** `text` — JSON string; longer than a declared `text(n)` is `bound_violation` */
  text: (): TypeSpec => new TypeSpec({ k: 'scalar', ty: { name: 'text' } }),
  /** `int` — JSON integer */
  int: (): TypeSpec => new TypeSpec({ k: 'scalar', ty: { name: 'int' } }),
  /** `money(<places>)` — exact decimal string (`"-12.50"`) — never a float, never a number The places are the currency's: `t.money(2)` stores cents and is written `29.99`. Omitted, they are 0. */
  money: (places?: number): TypeSpec =>
    new TypeSpec({ k: 'scalar', ty: places === undefined ? { name: 'money' } : { name: 'money', places } }),
  /** `decimal(<places>)` — exact decimal string with at most the declared places (`"0.0825"`) — never a float Rates and unit prices; it becomes money only through `round`. */
  decimal: (places: number): TypeSpec =>
    new TypeSpec({ k: 'scalar', ty: { name: 'decimal', places } }),
  /** `bool` — JSON boolean */
  bool: (): TypeSpec => new TypeSpec({ k: 'scalar', ty: { name: 'bool' } }),
  /** `date` — `"YYYY-MM-DD"`, validated */
  date: (): TypeSpec => new TypeSpec({ k: 'scalar', ty: { name: 'date' } }),
  /** `datetime` — `"YYYY-MM-DDTHH:MM:SSZ"` — UTC, the `Z` mandatory */
  datetime: (): TypeSpec => new TypeSpec({ k: 'scalar', ty: { name: 'datetime' } }),
  /** `duration(<unit>)` — the unit is part of the type; nothing converts between two. */
  duration: (unit: DurationUnit): TypeSpec =>
    new TypeSpec({ k: 'scalar', ty: { name: 'duration', unit } }),
  /** `id` — typed id string (`prefix_body`) */
  id: (): TypeSpec => new TypeSpec({ k: 'scalar', ty: { name: 'id' } }),
  /** `blob` — opaque hex handle; the bytes move through the blob door, never in JSON */
  blob: (): TypeSpec => new TypeSpec({ k: 'scalar', ty: { name: 'blob' } }),
  /** `uri` — JSON string — an address the platform references and does not hold */
  uri: (): TypeSpec => new TypeSpec({ k: 'scalar', ty: { name: 'uri' } }),
  /** `json` — any JSON value, carried as JSON; larger or deeper than the deployment's limit is `bound_violation` */
  json: (): TypeSpec => new TypeSpec({ k: 'scalar', ty: { name: 'json' } }),
  /** `field -> target` — a many-to-one edge; `.owned()` and `.tree()` mark it. */
  ref,
} as const;

/**
 * `fn` — the closed expression-function set, at the arities the parser 
 * checks. An unknown function is not a run-time error here; it is a 
 * name that does not exist.
 */
export const fn = {
  /** `year(…)` — 1..1 argument(s). */
  year: (a0: Value): Value => new Val({ k: 'call', name: 'year', args: [a0.node] }),
  /** `month(…)` — 1..1 argument(s). */
  month: (a0: Value): Value => new Val({ k: 'call', name: 'month', args: [a0.node] }),
  /** `week(…)` — 1..1 argument(s). */
  week: (a0: Value): Value => new Val({ k: 'call', name: 'week', args: [a0.node] }),
  /** `day(…)` — 1..1 argument(s). */
  day: (a0: Value): Value => new Val({ k: 'call', name: 'day', args: [a0.node] }),
  /** `hour(…)` — 1..1 argument(s). */
  hour: (a0: Value): Value => new Val({ k: 'call', name: 'hour', args: [a0.node] }),
  /** `dow(…)` — 1..1 argument(s). */
  dow: (a0: Value): Value => new Val({ k: 'call', name: 'dow', args: [a0.node] }),
  /** `abs(…)` — 1..1 argument(s). */
  abs: (a0: Value): Value => new Val({ k: 'call', name: 'abs', args: [a0.node] }),
  /** `round(…)` — 1..2 argument(s). */
  round: (a0: Value, a1?: Value): Value => new Val({ k: 'call', name: 'round', args: [a0, a1].filter((x): x is Value => x !== undefined).map((x) => x.node) }),
  /** `floor(…)` — 1..1 argument(s). */
  floor: (a0: Value): Value => new Val({ k: 'call', name: 'floor', args: [a0.node] }),
  /** `ceil(…)` — 1..1 argument(s). */
  ceil: (a0: Value): Value => new Val({ k: 'call', name: 'ceil', args: [a0.node] }),
  /** `mod(…)` — 2..2 argument(s). */
  mod: (a0: Value, a1: Value): Value => new Val({ k: 'call', name: 'mod', args: [a0.node, a1.node] }),
  /** `least(…)` — 2..2 argument(s). */
  least: (a0: Value, a1: Value): Value => new Val({ k: 'call', name: 'least', args: [a0.node, a1.node] }),
  /** `greatest(…)` — 2..2 argument(s). */
  greatest: (a0: Value, a1: Value): Value => new Val({ k: 'call', name: 'greatest', args: [a0.node, a1.node] }),
  /** `hash(…)` — 1..1 argument(s). */
  hash: (a0: Value): Value => new Val({ k: 'call', name: 'hash', args: [a0.node] }),
  /**
   * `allocate(total, weights[, mode])` — the blessed money split.
   *
   * The third argument is a MODE NAME, not an expression: the parser
   * checks the written form. Omit it for `largestRemainder`.
   */
  allocate: (total: Value, weights: Value, mode?: AllocateMode): Value =>
    new Val({
      k: 'call',
      name: 'allocate',
      args:
        mode === undefined
          ? [total.node, weights.node]
          : [total.node, weights.node, { k: 'path', segments: [mode] }],
    }),
  /** `coalesce(…)` — 2..2 argument(s). */
  coalesce: (a0: Value, a1: Value): Value => new Val({ k: 'call', name: 'coalesce', args: [a0.node, a1.node] }),
  /** `lower(…)` — 1..1 argument(s). */
  lower: (a0: Value): Value => new Val({ k: 'call', name: 'lower', args: [a0.node] }),
  /** `length(…)` — 1..1 argument(s). */
  length: (a0: Value): Value => new Val({ k: 'call', name: 'length', args: [a0.node] }),
  /** `trim(…)` — 1..1 argument(s). */
  trim: (a0: Value): Value => new Val({ k: 'call', name: 'trim', args: [a0.node] }),
} as const;

/**
 * `agg` — the aggregates, at the argument shapes the parser accepts.
 *
 * The output name is the KEY in `group`'s object, not an argument 
 * here: `group(f.customer, { spent: agg.sum(f.total) })` is the shape 
 * `group customer { spent: sum total }` has, and one output name in two 
 * places would be two places for it to differ.
 */
export const agg = {
  /** `count` and `count(<condition>)` — the conditional aggregate. */
  count: (when?: Cond): AggNode => ({
    out: '',
    func: 'count',
    arg: null,
    arg2: null,
    cond: when ? when.node : null,
  }),
  /** `sum <expr>` */
  sum: (of: Value): AggNode => ({
    out: '',
    func: 'sum',
    arg: of.node,
    arg2: null,
    cond: null,
  }),
  /** `avg <expr>` */
  avg: (of: Value): AggNode => ({
    out: '',
    func: 'avg',
    arg: of.node,
    arg2: null,
    cond: null,
  }),
  /** `min <expr>` */
  min: (of: Value): AggNode => ({
    out: '',
    func: 'min',
    arg: of.node,
    arg2: null,
    cond: null,
  }),
  /** `max <expr>` */
  max: (of: Value): AggNode => ({
    out: '',
    func: 'max',
    arg: of.node,
    arg2: null,
    cond: null,
  }),
  /** `median <expr>` */
  median: (of: Value): AggNode => ({
    out: '',
    func: 'median',
    arg: of.node,
    arg2: null,
    cond: null,
  }),
  /**
   * `percentile(<expr>, 0.95)` — the fraction is a literal strictly
   * between 0 and 1, checked at parse time. 0 and 1 are excluded on
   * purpose: those are `min` and `max`, which already have spellings.
   */
  percentile: (of: Value, fraction: string): AggNode => ({
    out: '',
    func: 'percentile',
    arg: of.node,
    arg2: decLit(fraction).node,
    cond: null,
  }),
  /** `distinct <expr>` */
  distinct: (of: Value): AggNode => ({
    out: '',
    func: 'distinct',
    arg: of.node,
    arg2: null,
    cond: null,
  }),
  /** `joined(<expr>, ", ")` — the separator is a text literal. */
  joined: (of: Value, separator: string): AggNode => ({
    out: '',
    func: 'joined',
    arg: of.node,
    arg2: { k: 'str', v: separator },
    cond: null,
  }),
} as const;

type _Nodes = [AggNode, CondNode, ExprNode];
export type { _Nodes };
