// GENERATED — do not edit; regenerate with `ordo gen ts`.
//
// The language's closed vocabularies, and what each word is called here.
//
// Source of truth: the parser's own closed tables in `crates/ordo-syntax` (`Keyword`, `ScalarTy`, `CmpOp`, `TextOp`, `BinOp`, `DurationUnit`, `functions()`, `AGGREGATE_SHAPES`, `ALLOCATE_MODES`)
//
// This file is the GENERATED half of this package. The hand-written
// half is under `src/core/` and is versioned by hand; the boundary
// between them is documented in the package README. An edit here is
// lost the next time the generator runs.

/**
 * One reserved word, and its TypeScript spelling — or the reason it has none.
 *
 * `tag` is `offered` (an author types `name`), `fused` (the word is reachable only inside the spelling `name` gives), `does-not-parse` (the LANGUAGE refuses the construct, so the builder must too — an absent method is the language's refusal arriving one layer earlier), or `out-of-scope` (this increment does not cover it, and the README and the checklist both say so).
 */
export interface Word {
  readonly word: string;
  readonly tag: 'offered' | 'fused' | 'does-not-parse' | 'out-of-scope';
  readonly note: string;
}

/**
 * Every reserved word in the language, in `token.rs` order.
 *
 * The list is complete by construction: the generator matches `Keyword` exhaustively with no wildcard arm, so a word added to the lexer fails `cargo build` until someone decides what it is called here. `test/vocabulary.test.ts` then checks the other half — that every `offered` name really resolves on this package's surface.
 */
export const KEYWORDS: readonly Word[] = [
  { word: 'and', tag: 'offered', note: 'and' },
  { word: 'or', tag: 'offered', note: 'or' },
  { word: 'not', tag: 'offered', note: 'not' },
  { word: 'in', tag: 'offered', note: '.in' },
  { word: 'is', tag: 'fused', note: '.isNull() / .isNotNull()' },
  { word: 'null', tag: 'fused', note: 'nul() — the literal; .isNull() — the test' },
  { word: 'true', tag: 'fused', note: 'lit(true)' },
  { word: 'false', tag: 'fused', note: 'lit(false)' },
  { word: 'match', tag: 'fused', note: '.matchAny/.matchNone/.matchRef/.matchOn' },
  { word: 'any', tag: 'offered', note: '.matchAny' },
  { word: 'none', tag: 'offered', note: '.matchNone' },
  { word: 'on', tag: 'offered', note: '.matchOn' },
  { word: 'group', tag: 'offered', note: '.group' },
  { word: 'reduce', tag: 'offered', note: '.reduce' },
  { word: 'sort', tag: 'offered', note: '.sort' },
  { word: 'take', tag: 'offered', note: '.take' },
  { word: 'append', tag: 'offered', note: '.append' },
  { word: 'let', tag: 'offered', note: 'let_' },
  { word: 'distinct', tag: 'offered', note: '.distinct / agg.distinct' },
  { word: 'under', tag: 'offered', note: '.under' },
  { word: 'above', tag: 'offered', note: '.above' },
  { word: 'since', tag: 'offered', note: '.since' },
  { word: 'contains', tag: 'offered', note: '.contains' },
  { word: 'starts', tag: 'offered', note: '.starts' },
  { word: 'matches', tag: 'offered', note: '.matches' },
  { word: 'set', tag: 'offered', note: 'w.set' },
  { word: 'del', tag: 'offered', note: 'w.del' },
  { word: 'subtree', tag: 'offered', note: '.subtree' },
  { word: 'each', tag: 'offered', note: 'each' },
  { word: 'op', tag: 'offered', note: 'op' },
  { word: 'entity', tag: 'offered', note: 'entity' },
  { word: 'routine', tag: 'offered', note: 'routine' },
  { word: 'domain', tag: 'offered', note: 'domain' },
  { word: 'when', tag: 'offered', note: 'when' },
  { word: 'every', tag: 'offered', note: 'every / daily' },
  { word: 'trigger', tag: 'fused', note: 'the `trigger` option of routine()' },
  { word: 'check', tag: 'offered', note: '.check' },
  { word: 'index', tag: 'offered', note: '.index' },
  { word: 'unique', tag: 'offered', note: '.unique' },
  { word: 'sealed', tag: 'offered', note: '.sealed' },
  { word: 'indexed', tag: 'offered', note: '.indexed' },
  { word: 'owned', tag: 'offered', note: '.owned' },
  { word: 'tree', tag: 'offered', note: '.tree' },
  { word: 'normalize', tag: 'offered', note: '.normalize' },
  { word: 'whole', tag: 'out-of-scope', note: '`whole` is a special form, not a pipeline step, and it does not compose — a builder for it is a second skeleton, not vocabulary poured into this one. See the crate README' },
  { word: 'acl', tag: 'offered', note: '.acl' },
  { word: 'access', tag: 'offered', note: '.accessRead / .accessWrite' },
  { word: 'app', tag: 'does-not-parse', note: '`app` blocks are `ui-gen.md` §5, whose binding grammar is normative and whose block set is explicitly still open' },
  { word: 'page', tag: 'does-not-parse', note: '`page` blocks are `ui-gen.md` §5 with `app`: the binding grammar is normative and the block set is explicitly still open' },
  { word: 'principal', tag: 'does-not-parse', note: '`principal` is under §8\'s [lean] spelling pin; the mechanism is built, the declaration is not fixed' },
  { word: 'client', tag: 'does-not-parse', note: '`client` is under §8\'s [lean] spelling pin, like `principal`: the mechanism is built and the declaration form is not fixed' },
  { word: 'auth', tag: 'does-not-parse', note: '`auth` is under §8\'s [lean] spelling pin; no declaration form for it parses today' },
  { word: 'policy', tag: 'does-not-parse', note: '`policy` is under §8\'s [lean] spelling pin; no declaration form for it parses today' },
  { word: 'compliance', tag: 'does-not-parse', note: '`compliance` is under §8\'s [lean] spelling pin; no declaration form for it parses today' },
  { word: 'channel', tag: 'does-not-parse', note: '`channel` is under §8\'s [lean] spelling pin. Its semantics are settled and there is no engine mechanism behind it either: the only `channel` in this workspace is the invalidation bus, a different sense of the word' },
  { word: 'space', tag: 'does-not-parse', note: '`space` is reserved so it cannot be a clause head by accident; no declaration form for it parses' },
  { word: 'unguarded', tag: 'offered', note: '.unguarded' },
  { word: 'unbounded', tag: 'offered', note: '.unbounded' },
  { word: 'reproducible', tag: 'offered', note: '.reproducible' },
  { word: 'nopause', tag: 'offered', note: '.nopause' },
  { word: 'public', tag: 'offered', note: '.public' },
  { word: 'filled', tag: 'out-of-scope', note: '`filled` parses on a parameter (`$today: date filled`) and the builder does not offer it yet' },
  { word: 'cond', tag: 'offered', note: 'cond' },
] as const;

/**
 * The duration units — `ms s m h d w mo y` and nothing else.
 */
export const DURATION_UNITS = ['ms', 's', 'm', 'h', 'd', 'w', 'mo', 'y'] as const;

/**
 * The allocation modes.
 *
 * `rotation` is deliberately absent: spreading the odd minor unit over successive invoices has to remember what it did last time, which is application state, not a function of `(total, weights)`.
 */
export const ALLOCATE_MODES = ['largestRemainder', 'first', 'last', 'largestShare'] as const;

/**
 * The expression functions, with the arities the parser enforces.
 */
export const FUNCTIONS = [
  { name: 'year', min: 1, max: 1 },
  { name: 'month', min: 1, max: 1 },
  { name: 'week', min: 1, max: 1 },
  { name: 'day', min: 1, max: 1 },
  { name: 'hour', min: 1, max: 1 },
  { name: 'dow', min: 1, max: 1 },
  { name: 'abs', min: 1, max: 1 },
  { name: 'round', min: 1, max: 2 },
  { name: 'floor', min: 1, max: 1 },
  { name: 'ceil', min: 1, max: 1 },
  { name: 'mod', min: 2, max: 2 },
  { name: 'least', min: 2, max: 2 },
  { name: 'greatest', min: 2, max: 2 },
  { name: 'hash', min: 1, max: 1 },
  { name: 'allocate', min: 2, max: 3 },
  { name: 'coalesce', min: 2, max: 2 },
  { name: 'lower', min: 1, max: 1 },
  { name: 'length', min: 1, max: 1 },
  { name: 'trim', min: 1, max: 1 },
] as const;

/**
 * The aggregates, with the argument shape `agg_decl` parses for each.
 */
export const AGGREGATES = [
  { name: 'count', shape: 'none-or-condition' },
  { name: 'sum', shape: 'one-expr' },
  { name: 'avg', shape: 'one-expr' },
  { name: 'min', shape: 'one-expr' },
  { name: 'max', shape: 'one-expr' },
  { name: 'median', shape: 'one-expr' },
  { name: 'percentile', shape: 'expr-and-fraction' },
  { name: 'distinct', shape: 'one-expr' },
  { name: 'joined', shape: 'expr-and-separator' },
] as const;

/**
 * The comparison, text and arithmetic operators, with their spellings.
 */
export const OPERATORS = {
  compare: [
    { method: 'eq', ordo: '==' },
    { method: 'ne', ordo: '!=' },
    { method: 'lt', ordo: '<' },
    { method: 'le', ordo: '<=' },
    { method: 'gt', ordo: '>' },
    { method: 'ge', ordo: '>=' },
  ],
  text: [
    { method: 'contains', ordo: 'contains' },
    { method: 'starts', ordo: 'starts' },
    { method: 'matches', ordo: 'matches' },
  ],
  arithmetic: [
    { method: 'add', ordo: '+' },
    { method: 'sub', ordo: '-' },
    { method: 'mul', ordo: '*' },
    { method: 'div', ordo: '/' },
  ],
} as const;

/**
 * The scalar types that have a DECLARATION form, with their constructors.
 *
 * `money(p)` and `decimal(p)` carry their places (27ak): `t.money(2)` is an amount in cents, `t.decimal(4)` a rate or a unit price stored exactly. A bare `money` is `money(0)`. This list is generated from the same match every other type comes out of, so a type the parser gains or loses cannot be missed here by accident.
 */
export const SCALARS = [
  { ordo: 'text', ctor: 'text', ts: 'string', wire: 'JSON string; longer than a declared `text(n)` is `bound_violation`' },
  { ordo: 'int', ctor: 'int', ts: 'number', wire: 'JSON integer' },
  { ordo: 'money', ctor: 'money', ts: 'string', wire: 'exact decimal string (`"-12.50"`) — never a float, never a number' },
  { ordo: 'decimal', ctor: 'decimal', ts: 'string', wire: 'exact decimal string with at most the declared places (`"0.0825"`) — never a float' },
  { ordo: 'bool', ctor: 'bool', ts: 'boolean', wire: 'JSON boolean' },
  { ordo: 'date', ctor: 'date', ts: 'string', wire: '`"YYYY-MM-DD"`, validated' },
  { ordo: 'datetime', ctor: 'datetime', ts: 'string', wire: '`"YYYY-MM-DDTHH:MM:SSZ"` — UTC, the `Z` mandatory' },
  { ordo: 'duration', ctor: 'duration', ts: 'string', wire: '`"<int><unit>"`, units exactly `ms s m h d w mo y`' },
  { ordo: 'id', ctor: 'id', ts: 'string', wire: 'typed id string (`prefix_body`)' },
  { ordo: 'blob', ctor: 'blob', ts: 'string', wire: 'opaque hex handle; the bytes move through the blob door, never in JSON' },
  { ordo: 'uri', ctor: 'uri', ts: 'string', wire: 'JSON string — an address the platform references and does not hold' },
  { ordo: 'json', ctor: 'json', ts: 'JsonValue', wire: 'any JSON value, carried as JSON; larger or deeper than the deployment\'s limit is `bound_violation`' },
] as const;
