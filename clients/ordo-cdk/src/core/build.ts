/**
 * The builders — **the skeleton**, and the whole of the hand-designed
 * surface.
 *
 * What lives here is the *chaining shape and the argument order*: that a
 * pipeline is `b.source(…)` followed by steps, that `group` takes keys and
 * then an aggregate object, that a write body returns statements, that an
 * entity's clauses are chained after its fields. Not one word of Ordo's
 * vocabulary is written here — no operator, no function, no aggregate, no
 * scalar type. Those are generated (`src/generated/`), and this file is what
 * they are poured into.
 *
 * The line matters because of what each side costs when it drifts. A word
 * that drifts is a builder that can express something the language cannot (or
 * cannot express something it can) — caught by isomorphism, and prevented by
 * generating it. A *shape* that drifts is an API redesign, which is a human
 * decision and should not be a `git diff` on a generated file.
 */

import type {
  AggNode,
  CondNode,
  Declaration,
  DomainNode,
  EntityNode,
  ExprNode,
  FieldKindNode,
  FieldNode,
  GuardClauseNode,
  LetNode,
  OpBodyNode,
  OpNode,
  ParamNode,
  PipelineNode,
  ProjItemNode,
  ScalarType,
  SortKeyNode,
  SourceArgNode,
  SourceNode,
  StepNode,
  TakeCountNode,
  WriteStmtNode,
} from './ir.js';
import { printDeclaration } from './print.js';
import { Routine } from './routine.js';
import { Cond, intDigits, makeParams, ValueBase, type Value } from './value.js';

// ── declared types ───────────────────────────────────────────────────────

/**
 * A declared type, plus every modifier the declaration grammar admits.
 *
 * One class for fields and parameters, with a **refusal at the seam** rather
 * than two near-identical classes: a `max` on an entity field and a `sealed`
 * on a parameter are both nonsense, and `entity()`/`op()` say so by name. Two
 * classes would have made `t.money()` need to know which it was about to be,
 * which is a decision the author has not made yet at the moment they write
 * it.
 */
export class TypeSpec {
  readonly kind: FieldKindNode;
  optionalFlag = false;
  uniqueFlag = false;
  sealedFlag = false;
  indexedFlag = false;
  publicFlag = false;
  refinableFlag = false;
  inlineFlag = false;
  privateFlag = false;
  normalizeName: string | null = null;
  defaultValue: ExprNode | null = null;
  maxValue: ExprNode | null = null;

  constructor(kind: FieldKindNode) {
    this.kind = kind;
  }

  /** `?` — the field admits null, or the parameter is not required. */
  optional(): this {
    this.optionalFlag = true;
    return this;
  }

  unique(): this {
    this.uniqueFlag = true;
    return this;
  }

  sealed(): this {
    this.sealedFlag = true;
    return this;
  }

  indexed(): this {
    this.indexedFlag = true;
    return this;
  }

  /** `public` — this field may be served by a public op (field tail). */
  public(): this {
    this.publicFlag = true;
    return this;
  }

  refinable(): this {
    this.refinableFlag = true;
    return this;
  }

  /**
   * `blob inline` — the bytes live in the sealed payload store rather than
   * in the space's object store (DL §2, pin 2).
   *
   * A **storage-tier** modifier, orthogonal to the custody ones, which is
   * why it is a method on the spec rather than an argument to `t.blob()`:
   * DL says the custody tiers and `inline` compose orthogonally, and a
   * constructor argument would make one of them a parameter of the other.
   */
  inline(): this {
    if (this.kind.k !== 'ref' && this.kind.ty.name !== 'blob') {
      throw new Error(
        '`inline` chooses where a blob\'s BYTES are stored, so it is only meaningful ' +
          'on a `blob` field',
      );
    }
    this.inlineFlag = true;
    return this;
  }

  /**
   * `blob private` — client-sealed custody (DL §2): the client encrypts
   * before upload with a passkey-PRF-derived key, and the platform holds
   * no key.
   *
   * Declarable, and refused at every runtime path that cannot serve it —
   * F7 Phase 6 owns the key ceremony and the presigned upload. The builder
   * emits it because a program that cannot SAY `private` cannot be told
   * `private` is not built yet.
   */
  private(): this {
    if (this.kind.k !== 'ref' && this.kind.ty.name !== 'blob') {
      throw new Error(
        '`private` is client-sealed custody over a blob\'s bytes, so it is only ' +
          'meaningful on a `blob` field',
      );
    }
    if (this.sealedFlag) {
      throw new Error(
        '`sealed` seals under the space key and `private` under a key the platform ' +
          'never holds — pick one; together they name two custody positions, not a ' +
          'stronger one',
      );
    }
    this.privateFlag = true;
    return this;
  }

  normalize(name: string): this {
    this.normalizeName = name;
    return this;
  }

  default(v: Value): this {
    this.defaultValue = v.node;
    return this;
  }

  /**
   * `$n: int <= 200`. Upper bound only — the language has no `>=` and no
   * range form, so neither does this.
   */
  max(v: Value): this {
    this.maxValue = v.node;
    return this;
  }

  /** `owned` — the ownership edge. A ref modifier; a scalar refuses it. */
  owned(): this {
    if (this.kind.k !== 'ref') throw new Error('`owned` marks a ref, not a scalar field');
    (this.kind as { owned: boolean }).owned = true;
    return this;
  }

  /** `tree` — the self-edge that carries the implicit `depth`. */
  tree(): this {
    if (this.kind.k !== 'ref') throw new Error('`tree` marks a ref, not a scalar field');
    (this.kind as { tree: boolean }).tree = true;
    return this;
  }
}

function fieldNode(name: string, s: TypeSpec): FieldNode {
  if (s.maxValue !== null) {
    throw new Error(
      `\`${name}\`: \`max\` is a parameter bound, not a field modifier — a field's ` +
        'limit is `check`, which is a row invariant the schema can prove',
    );
  }
  return {
    name,
    kind: s.kind,
    optional: s.optionalFlag,
    default: s.defaultValue,
    unique: s.uniqueFlag,
    sealed: s.sealedFlag,
    indexed: s.indexedFlag,
    public: s.publicFlag,
    refinable: s.refinableFlag,
    inline: s.inlineFlag,
    private: s.privateFlag,
    normalize: s.normalizeName,
  };
}

export function paramNode(name: string, s: TypeSpec): ParamNode {
  if (s.kind.k !== 'scalar') {
    throw new Error(`\`$${name}\`: a parameter takes a scalar type, never a ref`);
  }
  for (const [flag, word] of [
    [s.uniqueFlag, 'unique'],
    [s.sealedFlag, 'sealed'],
    [s.indexedFlag, 'indexed'],
    [s.publicFlag, 'public'],
    [s.refinableFlag, 'refinable'],
    [s.inlineFlag, 'inline'],
    [s.privateFlag, 'private'],
    [s.normalizeName !== null, 'normalize'],
  ] as const) {
    if (flag) throw new Error(`\`$${name}\`: \`${word}\` is a field modifier, not a parameter one`);
  }
  const ty: ScalarType = s.kind.ty;
  // The compiler refuses `$f: blob` with class `blob_parameter`, so a
  // builder that emitted one would emit a program that does not deploy.
  // Refusing here means the author is told at the line they wrote rather
  // than by a registration gate reading generated source.
  if (ty.name === 'blob') {
    throw new Error(
      `\`$${name}\`: bytes are not a JSON wire value, so \`blob\` is not a parameter ` +
        'type — upload through the blob door, which mints the handle',
    );
  }
  return { name, ty, max: s.maxValue, default: s.defaultValue };
}

// ── sources ──────────────────────────────────────────────────────────────

/** `named('cap', p.cap)` — one `$cap: <expr>` binding in a source's parens. */
export function named(name: string, value: Value): SourceArgNode {
  return { k: 'named', name, value: value.node };
}

/** `[*]` */
export function star(): ProjItemNode {
  return { k: 'star' };
}

/** `[*, -memo]` — everything-minus. */
export function minus(name: string): ProjItemNode {
  return { k: 'exclude', name };
}

/** `[label: name]` and `[sale: price - 500]` — rename, or compute. */
export function as_(out: string, value: Value): ProjItemNode {
  return { k: 'named', out, value: value.node };
}

/** What `.pick(…)` accepts: a dotted field name, or one of the items above. */
export type PickItem = string | ProjItemNode;

function pickItem(p: PickItem): ProjItemNode {
  return typeof p === 'string' ? { k: 'field', path: p.split('.') } : p;
}

/** Anything usable as a source: an entity, an op, or a bare name. */
export interface Sourceable {
  sourceName(): string;
}

function sourceName(s: Sourceable | string): string {
  return typeof s === 'string' ? s : s.sourceName();
}

/**
 * One source in a source list, with its call arguments and its projection.
 *
 * The *ordered* `args` method is the honest one: the grammar mixes named
 * bindings and conditions in one comma list, and their order is part of the
 * tree. `where` and `bind` are sugar that appends, for the common case where
 * order is the order they were written.
 */
export class Src implements Sourceable {
  #name: string;
  #args: SourceArgNode[] = [];
  #projection: ProjItemNode[] | null = null;

  constructor(name: string) {
    this.#name = name;
  }

  sourceName(): string {
    return this.#name;
  }

  /** The full argument list, in order. */
  args(...items: (SourceArgNode | Cond)[]): Src {
    const next = this.#clone();
    for (const i of items) {
      next.#args.push(i instanceof Cond ? { k: 'cond', cond: i.node } : i);
    }
    return next;
  }

  /** Append conditions: the entity filter form, and call-form refinement. */
  where(...conds: Cond[]): Src {
    return this.args(...conds);
  }

  /** Append a named binding: `$cap: <expr>`. */
  bind(name: string, value: Value): Src {
    return this.args(named(name, value));
  }

  /**
   * `[…]`. Calling it with nothing is `[]`, which the grammar admits and
   * which is NOT the same as writing no brackets at all.
   */
  pick(...items: PickItem[]): Src {
    const next = this.#clone();
    next.#projection = items.map(pickItem);
    return next;
  }

  node(): SourceNode {
    return { name: this.#name, args: [...this.#args], projection: this.#projection };
  }

  #clone(): Src {
    const c = new Src(this.#name);
    c.#args = [...this.#args];
    c.#projection = this.#projection === null ? null : [...this.#projection];
    return c;
  }
}

/** `src('orders')` — a source named by string, for a name with no binding. */
export function src(s: Sourceable | string): Src {
  return new Src(sourceName(s));
}

function toSource(s: Sourceable | string | Src): SourceNode {
  if (s instanceof Src) return s.node();
  return new Src(sourceName(s)).node();
}

// ── sort keys ────────────────────────────────────────────────────────────

/** `sort -total` */
export function desc(v: Value): SortKeyNode {
  return { expr: v.node, desc: true };
}

/** `sort placed` — explicit, for symmetry with {@link desc}. */
export function asc(v: Value): SortKeyNode {
  return { expr: v.node, desc: false };
}

export type SortArg = SortKeyNode | Value | string;

function sortKey(k: SortArg): SortKeyNode {
  if (typeof k === 'string') {
    const descending = k.startsWith('-');
    const name = descending ? k.slice(1) : k;
    return { expr: { k: 'path', segments: name.split('.') }, desc: descending };
  }
  return k instanceof ValueBase ? { expr: k.node, desc: false } : k;
}

// ── the pipeline builder ─────────────────────────────────────────────────

/** A read pipeline under construction. Immutable: every step returns a new one. */
export class Q {
  #lets: LetNode[];
  #sources: SourceNode[];
  #steps: StepNode[];

  constructor(lets: LetNode[], sources: SourceNode[], steps: StepNode[] = []) {
    this.#lets = lets;
    this.#sources = sources;
    this.#steps = steps;
  }

  #with(step: StepNode): Q {
    return new Q(this.#lets, this.#sources, [...this.#steps, step]);
  }

  /** `| <condition>` — the post-source filter stage. */
  where(cond: Cond): Q {
    return this.#with({ k: 'filter', cond: cond.node });
  }

  sort(...keys: SortArg[]): Q {
    return this.#with({ k: 'sort', keys: keys.map(sortKey) });
  }

  /**
   * `take 3`, `take $n`, and `take[plants.id] 1`.
   *
   * Two shapes because the grammar has two: with keys the count follows the
   * bracket list, so the keys come first here too.
   */
  take(n: number | bigint | string | Value): Q;
  take(keys: Value[], n: number | bigint | string | Value): Q;
  take(a: unknown, b?: unknown): Q {
    const keys = Array.isArray(a) ? (a as Value[]).map((k) => k.node) : [];
    const count = (Array.isArray(a) ? b : a) as number | bigint | string | Value;
    let node: TakeCountNode;
    if (count instanceof ValueBase) {
      if (count.node.k !== 'param') {
        throw new Error('`take` takes a literal count or a `$parameter`, nothing else');
      }
      node = { k: 'param', name: count.node.name };
    } else {
      node = { k: 'lit', n: intDigits(count) };
    }
    return this.#with({ k: 'take', keys, n: node });
  }

  /** `group customer { spent: sum total, n: count }` */
  group(keys: Value | Value[], aggs: Record<string, AggNode>): Q {
    const ks = (Array.isArray(keys) ? keys : [keys]).map((k) => k.node);
    if (ks.length === 0) throw new Error('`group` needs keys — a keyless aggregation is `reduce`');
    return this.#with({ k: 'group', keys: ks, aggs: namedAggs(aggs) });
  }

  /** `reduce { … }` — a group with no keys: exactly one row. */
  reduce(aggs: Record<string, AggNode>): Q {
    return this.#with({ k: 'reduce', aggs: namedAggs(aggs) });
  }

  distinct(): Q {
    return this.#with({ k: 'distinct' });
  }

  /** `match any <source>` — a semi-join, never a preservation marker. */
  matchAny(s: Sourceable | string | Src): Q {
    return this.#with({ k: 'match', kind: { k: 'any', source: toSource(s) } });
  }

  /** `match none <source>` — an anti-join. */
  matchNone(s: Sourceable | string | Src): Q {
    return this.#with({ k: 'match', kind: { k: 'none', source: toSource(s) } });
  }

  /** `match payee` / `match order.customer` — the duplicate-ref choice. */
  matchRef(...path: string[]): Q {
    const segs = path.flatMap((p) => p.split('.'));
    if (segs.length === 0) throw new Error('`match` needs a ref path');
    return this.#with({ k: 'match', kind: { k: 'ref', path: segs } });
  }

  /** `match on <col>` — the let-key choice. */
  matchOn(col: string): Q {
    return this.#with({ k: 'match', kind: { k: 'on', col } });
  }

  /** `append <source>` — one source form per arm (G8). */
  append(s: Sourceable | string | Src): Q {
    return this.#with({ k: 'append', source: toSource(s) });
  }

  node(): PipelineNode {
    return { lets: [...this.#lets], sources: [...this.#sources], steps: [...this.#steps] };
  }
}

function namedAggs(aggs: Record<string, AggNode>): AggNode[] {
  return Object.entries(aggs).map(([out, a]) => ({ ...a, out }));
}

// ── the write builders ───────────────────────────────────────────────────

export class SetStmt {
  #binding: string | null = null;
  #entity: string;
  #cond: CondNode | null = null;
  #fields: [string, ExprNode][] = [];

  constructor(entity: string, fields?: Record<string, Value>) {
    this.#entity = entity;
    if (fields) this.fields(fields);
  }

  /** `set entity(cond) { … }` — the bulk converge. */
  where(cond: Cond): this {
    this.#cond = cond.node;
    return this;
  }

  fields(fields: Record<string, Value>): this {
    this.#fields = Object.entries(fields).map(([k, v]) => [k, v.node]);
    return this;
  }

  /**
   * `let x = set E { … }` — name this statement's row for later statements
   * in the same group. Only a CREATE may be named, which is derived rather
   * than syntactic, so the refusal is the registration gate's
   * (`unbindable_write`) and not this builder's.
   */
  let_(name: string): this {
    this.#binding = name;
    return this;
  }

  node(): WriteStmtNode {
    return {
      k: 'set',
      binding: this.#binding,
      entity: this.#entity,
      cond: this.#cond,
      fields: this.#fields.map(([a, b]) => [a, b] as const),
    };
  }
}

export class DelStmt {
  #entity: string;
  #cond: CondNode;
  #subtree = false;

  constructor(entity: string, cond: Cond) {
    this.#entity = entity;
    this.#cond = cond.node;
  }

  /** Cascade to every descendant of the anchor. */
  subtree(): this {
    this.#subtree = true;
    return this;
  }

  node(): WriteStmtNode {
    return { k: 'del', entity: this.#entity, cond: this.#cond, subtree: this.#subtree };
  }
}

// ── the op body builder ──────────────────────────────────────────────────

/**
 * What an op's callback is handed. A read body returns a {@link Q}; a write
 * body returns statements. Which one it was decides the op's kind, exactly
 * as the first word of the body does in the language.
 */
export class Body {
  #lets: LetNode[] = [];

  /** `let name = <pipeline>` — a named intermediate, lowered as a CTE. */
  let_(name: string, build: (b: Body) => Q): Body {
    const inner = build(new Body());
    const next = new Body();
    next.#lets = [...this.#lets, { name, pipeline: inner.node() }];
    return next;
  }

  /** The source line: one source, or several joined on one line. */
  source(...sources: (Sourceable | string | Src)[]): Q {
    if (sources.length === 0) throw new Error('a pipeline needs at least one source');
    return new Q([...this.#lets], sources.map(toSource));
  }

  set(entity: Sourceable | string, fields?: Record<string, Value>): SetStmt {
    return new SetStmt(sourceName(entity), fields);
  }

  /**
   * `del entity(cond)`. There is no bare `del entity`: rule 5 settles that a
   * delete without a stated target does not parse, and delete-everything is
   * spelled `del entity(true)` — visible to a reader and to grep. The
   * required `cond` parameter is that rule, in TypeScript.
   */
  del(entity: Sourceable | string, cond: Cond): DelStmt {
    return new DelStmt(sourceName(entity), cond);
  }
}

/** A write statement, or something that can produce one. */
export type WriteLike = SetStmt | DelStmt;

// ── declarations ─────────────────────────────────────────────────────────

/** `domain market @ 1.1.0` */
export function domain(name: string, version?: string): DomainNode {
  return { kind: 'domain', name, version: version ?? null };
}

export interface EntityOptions {
  /** `entity projects (key: proj)` — the typed-id prefix override. */
  readonly key?: string;
}

/** An entity declaration, usable as a source wherever one is wanted. */
export class Entity implements Sourceable {
  readonly kind = 'entity' as const;
  readonly name: string;
  readonly key: string | null;
  readonly fields: FieldNode[];
  accessReadClause: GuardClauseNode | null = null;
  accessWriteClause: GuardClauseNode | null = null;
  aclLevels: string[] | null = null;
  publicMark: 'public' | 'cascade' | null = null;
  uniqueKeys: string[][] = [];
  indexes: string[][] = [];
  checks: CondNode[] = [];

  constructor(name: string, fields: Record<string, TypeSpec>, options?: EntityOptions) {
    this.name = name;
    this.key = options?.key ?? null;
    this.fields = Object.entries(fields).map(([n, s]) => fieldNode(n, s));
  }

  sourceName(): string {
    return this.name;
  }

  /** Sugar so an entity reads as a source: `products.where(f.active)`. */
  where(...conds: Cond[]): Src {
    return new Src(this.name).where(...conds);
  }

  bind(name: string, value: Value): Src {
    return new Src(this.name).bind(name, value);
  }

  pick(...items: PickItem[]): Src {
    return new Src(this.name).pick(...items);
  }

  args(...items: (SourceArgNode | Cond)[]): Src {
    return new Src(this.name).args(...items);
  }

  /** `unique (a, b)` — the composite spelling only; one column is a modifier. */
  unique(...columns: string[]): this {
    if (columns.length < 2) {
      throw new Error(
        '`unique (a)` is refused: the single-column form is a field modifier ' +
          '(`email: text unique`), and two spellings of one claim could drift',
      );
    }
    this.uniqueKeys.push(columns);
    return this;
  }

  index(...columns: string[]): this {
    this.indexes.push(columns);
    return this;
  }

  check(cond: Cond): this {
    this.checks.push(cond.node);
    return this;
  }

  /** `acl (read, write, admin)` — levels in declaration order, which IS the ordering. */
  acl(...levels: string[]): this {
    this.aclLevels = levels;
    return this;
  }

  /** Entity line `public`: every field of this entity is public. */
  public(): this {
    this.publicMark = 'public';
    return this;
  }

  /** Entity line `public*`: `public`, and every entity owned by this one, transitively. */
  publicAll(): this {
    this.publicMark = 'cascade';
    return this;
  }

  /** `access read <walk>` — G9: one verb without the other is the schema's refusal. */
  accessRead(clause: GuardClauseNode): this {
    this.accessReadClause = clause;
    return this;
  }

  /** `access write <walk>` */
  accessWrite(clause: GuardClauseNode): this {
    this.accessWriteClause = clause;
    return this;
  }

  node(): EntityNode {
    return {
      kind: 'entity',
      name: this.name,
      key: this.key,
      fields: this.fields,
      accessRead: this.accessReadClause,
      accessWrite: this.accessWriteClause,
      acl: this.aclLevels,
      public: this.publicMark,
      uniqueKeys: this.uniqueKeys,
      indexes: this.indexes,
      checks: this.checks,
    };
  }
}

export function entity(
  name: string,
  fields: Record<string, TypeSpec>,
  options?: EntityOptions,
): Entity {
  return new Entity(name, fields, options);
}

/** `t.ref(vendors)` / `t.ref('categories')` — the self-reference needs the name. */
export function ref(target: Sourceable | string): TypeSpec {
  return new TypeSpec({ k: 'ref', target: sourceName(target), owned: false, tree: false });
}

/** An op declaration, usable as a composition source. */
export class Op implements Sourceable {
  readonly kind = 'op' as const;
  readonly name: string;
  readonly params: ParamNode[];
  readonly body: OpBodyNode;
  unboundedFlag = false;
  unguardedFlag = false;
  reproducibleFlag = false;
  nopauseFlag = false;
  publicFlag = false;
  downloadFlag = false;

  constructor(name: string, params: ParamNode[], body: OpBodyNode) {
    this.name = name;
    this.params = params;
    this.body = body;
  }

  sourceName(): string {
    return this.name;
  }

  where(...conds: Cond[]): Src {
    return new Src(this.name).where(...conds);
  }

  bind(name: string, value: Value): Src {
    return new Src(this.name).bind(name, value);
  }

  pick(...items: PickItem[]): Src {
    return new Src(this.name).pick(...items);
  }

  args(...items: (SourceArgNode | Cond)[]): Src {
    return new Src(this.name).args(...items);
  }

  unbounded(): this {
    this.unboundedFlag = true;
    return this;
  }

  /** `unguarded` — bypass, which the language makes explicit and greppable. */
  unguarded(): this {
    this.unguardedFlag = true;
    return this;
  }

  reproducible(): this {
    this.reproducibleFlag = true;
    return this;
  }

  /** `nopause`: the op keeps answering while its tenant is paused. */
  nopause(): this {
    this.nopauseFlag = true;
    return this;
  }

  /** `public`: the op runs on the public path (anonymous caller, cached, no guard walk). */
  public(): this {
    this.publicFlag = true;
    return this;
  }

  /**
   * `download`: the op's rows go into a person's own download (`GET /download`).
   * A read over one entity; its fields are the projection, and a blob field brings the files.
   */
  download(): this {
    this.downloadFlag = true;
    return this;
  }

  node(): OpNode {
    return {
      kind: 'op',
      name: this.name,
      params: this.params,
      flags: {
        unbounded: this.unboundedFlag,
        unguarded: this.unguardedFlag,
        reproducible: this.reproducibleFlag,
        nopause: this.nopauseFlag,
        public: this.publicFlag,
        ...(this.downloadFlag ? { download: true } : {}),
      },
      body: this.body,
    };
  }
}

/** The `p.` namespace an op body is handed, typed from its own parameters. */
export type ParamsOf<P> = { readonly [K in keyof P]: Value };

/**
 * `op(name, params, body)`.
 *
 * The body callback is handed the {@link Body} builder and the parameter
 * namespace. It returns a {@link Q} for a read op, or an array of statements
 * for a write group — never both, exactly as the language allows never both.
 */
export function op<P extends Record<string, TypeSpec>>(
  name: string,
  params: P,
  build: (b: Body, p: ParamsOf<P>) => Q | WriteLike | WriteLike[],
): Op {
  const paramNodes = Object.entries(params).map(([n, s]) => paramNode(n, s));
  const p = makeParams(Object.keys(params)) as ParamsOf<P>;
  const built = build(new Body(), p);
  const body: OpBodyNode =
    built instanceof Q
      ? { k: 'read', pipeline: built.node() }
      : {
          k: 'write',
          statements: (Array.isArray(built) ? built : [built]).map((w) => w.node()),
        };
  if (body.k === 'write' && body.statements.length === 0) {
    throw new Error(`op ${name}: a write group needs at least one statement`);
  }
  return new Op(name, paramNodes, body);
}

// ── synth ────────────────────────────────────────────────────────────────

/** Anything `synth` accepts: a declaration, or a builder that produces one. */
export type Synthesizable = Declaration | Entity | Op | Routine;

function declaration(d: Synthesizable): Declaration {
  return d instanceof Entity || d instanceof Op || d instanceof Routine ? d.node() : d;
}

/**
 * Emit canonical `.ordo` text.
 *
 * The synthesized file is the artifact that is reviewed, diffed, committed
 * and deployed — builders may loop and abstract, and the output is always the
 * flat readable form. Declarations are printed in the order they are handed
 * over, so the file's layout is the author's.
 */
export function synth(...decls: Synthesizable[]): string {
  const out = decls.map((d) => printDeclaration(declaration(d)));
  return out.join('\n\n') + '\n';
}
