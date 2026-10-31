/**
 * The IR — a TypeScript mirror of `ordo_syntax`'s AST, and nothing more.
 *
 * **This is the skeleton half of the package.** It is hand-written and
 * hand-versioned; the generated half pours vocabulary into it. The boundary
 * is documented in the README, and the rule that draws it is: a *shape* is
 * the skeleton's (that a comparison has a left, an operator and a right);
 * a *word* is the generator's (that the operators are `== != < <= > >=`, and
 * that they are spelled `.eq() .ne() .lt() .le() .gt() .ge()`).
 *
 * Every node here corresponds one-to-one with a variant of `ast.rs` or
 * `decl.rs`. That correspondence is not decoration: `ordo-gen`'s AST→builder
 * emitter matches exhaustively on the Rust side, so a variant with no node
 * here fails the generator's build. The isomorphism corpus then proves the
 * other direction — everything the builder can express parses.
 *
 * Integers are carried as **digit strings**, not JavaScript numbers. Ordo's
 * `int` is an i128 at lex time and its arithmetic is exact decimal; a literal
 * that round-tripped through an IEEE double would be a different number in
 * the synthesized file than in the source, silently, above 2^53.
 */

export type ExprNode =
  | { readonly k: 'int'; readonly v: string }
  | { readonly k: 'dec'; readonly v: string }
  | { readonly k: 'str'; readonly v: string }
  | { readonly k: 'date'; readonly v: string }
  | { readonly k: 'datetime'; readonly v: string }
  | { readonly k: 'dur'; readonly n: string; readonly unit: string }
  | { readonly k: 'bool'; readonly v: boolean }
  | { readonly k: 'null' }
  | { readonly k: 'param'; readonly name: string }
  | { readonly k: 'path'; readonly segments: readonly string[] }
  | { readonly k: 'neg'; readonly e: ExprNode }
  | { readonly k: 'bin'; readonly op: string; readonly l: ExprNode; readonly r: ExprNode }
  | { readonly k: 'call'; readonly name: string; readonly args: readonly ExprNode[] }
  | {
      readonly k: 'cond';
      readonly arms: readonly { readonly c: CondNode; readonly v: ExprNode }[];
      readonly else: ExprNode;
    };

export type CondNode =
  | { readonly k: 'cmp'; readonly op: string; readonly l: ExprNode; readonly r: ExprNode }
  | { readonly k: 'in'; readonly e: ExprNode; readonly list: readonly ExprNode[] }
  | { readonly k: 'text'; readonly op: string; readonly l: ExprNode; readonly r: ExprNode }
  | { readonly k: 'isnull'; readonly e: ExprNode; readonly negated: boolean }
  | { readonly k: 'since'; readonly e: ExprNode; readonly dur: ExprNode }
  | { readonly k: 'under'; readonly e: ExprNode; readonly id: ExprNode }
  | { readonly k: 'above'; readonly e: ExprNode; readonly id: ExprNode }
  | { readonly k: 'bare'; readonly e: ExprNode }
  | { readonly k: 'and'; readonly items: readonly CondNode[] }
  | { readonly k: 'or'; readonly items: readonly CondNode[] }
  | { readonly k: 'not'; readonly inner: CondNode };

// ── shapes ───────────────────────────────────────────────────────────────

/**
 * A scalar type, with the unit `duration` carries in its declaration and
 * the places `money` carries (`money(2)`; omitted means 0).
 */
export interface ScalarType {
  readonly name: string;
  readonly unit?: string;
  readonly places?: number;
}

export type FieldKindNode =
  | { readonly k: 'scalar'; readonly ty: ScalarType }
  | {
      readonly k: 'ref';
      readonly target: string;
      readonly owned: boolean;
      readonly tree: boolean;
    };

export interface FieldNode {
  readonly name: string;
  readonly kind: FieldKindNode;
  readonly optional: boolean;
  readonly default: ExprNode | null;
  readonly unique: boolean;
  readonly sealed: boolean;
  readonly indexed: boolean;
  /** `public` — this field may be served by a public op. */
  readonly public: boolean;
  readonly refinable: boolean;
  /** `blob inline` — the bytes ride the sealed payload, not object storage. */
  readonly inline: boolean;
  /** `blob private` — client-sealed; the platform holds no key. */
  readonly private: boolean;
  readonly normalize: string | null;
}

/**
 * One `and`-group of guard atoms — a single independent authorization path.
 *
 * The parser distributes parenthesized guards into disjunctive normal form
 * rather than keeping a nested node, because three separate rules are stated
 * over "a disjunct" (at least one must reference `$caller`; a create-exempt
 * write guard needs a reverse step in *every* disjunct; the lowering unions
 * one semi-join per disjunct). This builder distributes too, in
 * `core/guard.ts`, so the two cannot answer differently.
 */
export type GuardAtomNode =
  | {
      readonly k: 'cmp';
      readonly path: readonly string[];
      readonly caller: boolean;
      readonly lit: ExprNode | null;
    }
  | { readonly k: 'bare'; readonly path: readonly string[] }
  | { readonly k: 'step'; readonly path: readonly string[]; readonly inner: GuardExprNode };

export interface GuardExprNode {
  readonly disjuncts: readonly (readonly GuardAtomNode[])[];
}

export type GuardClauseNode =
  | { readonly k: 'walk'; readonly expr: GuardExprNode }
  | { readonly k: 'open' };

export interface EntityNode {
  readonly kind: 'entity';
  readonly name: string;
  readonly key: string | null;
  readonly fields: readonly FieldNode[];
  readonly accessRead: GuardClauseNode | null;
  readonly accessWrite: GuardClauseNode | null;
  readonly acl: readonly string[] | null;
  /** Entity line: `public` is `'public'`, `public*` is `'cascade'`. */
  readonly public: 'public' | 'cascade' | null;
  readonly uniqueKeys: readonly (readonly string[])[];
  readonly indexes: readonly (readonly string[])[];
  readonly checks: readonly CondNode[];
}

// ── pipelines ────────────────────────────────────────────────────────────

export type SourceArgNode =
  | { readonly k: 'named'; readonly name: string; readonly value: ExprNode }
  | { readonly k: 'cond'; readonly cond: CondNode };

export type ProjItemNode =
  | { readonly k: 'star' }
  | { readonly k: 'exclude'; readonly name: string }
  | { readonly k: 'field'; readonly path: readonly string[] }
  | { readonly k: 'named'; readonly out: string; readonly value: ExprNode };

export interface SourceNode {
  readonly name: string;
  readonly args: readonly SourceArgNode[];
  /** `null` is "no brackets written", which is not the same as `[]`. */
  readonly projection: readonly ProjItemNode[] | null;
}

export interface SortKeyNode {
  readonly expr: ExprNode;
  readonly desc: boolean;
}

export interface AggNode {
  readonly out: string;
  readonly func: string;
  readonly arg: ExprNode | null;
  readonly arg2: ExprNode | null;
  readonly cond: CondNode | null;
}

export type MatchKindNode =
  | { readonly k: 'any'; readonly source: SourceNode }
  | { readonly k: 'none'; readonly source: SourceNode }
  | { readonly k: 'ref'; readonly path: readonly string[] }
  | { readonly k: 'on'; readonly col: string };

export type TakeCountNode =
  | { readonly k: 'lit'; readonly n: string }
  | { readonly k: 'param'; readonly name: string };

export type StepNode =
  | { readonly k: 'filter'; readonly cond: CondNode }
  | { readonly k: 'sort'; readonly keys: readonly SortKeyNode[] }
  | { readonly k: 'take'; readonly keys: readonly ExprNode[]; readonly n: TakeCountNode }
  | { readonly k: 'group'; readonly keys: readonly ExprNode[]; readonly aggs: readonly AggNode[] }
  | { readonly k: 'reduce'; readonly aggs: readonly AggNode[] }
  | { readonly k: 'distinct' }
  | { readonly k: 'match'; readonly kind: MatchKindNode }
  | { readonly k: 'append'; readonly source: SourceNode };

export interface LetNode {
  readonly name: string;
  readonly pipeline: PipelineNode;
}

export interface PipelineNode {
  readonly lets: readonly LetNode[];
  readonly sources: readonly SourceNode[];
  readonly steps: readonly StepNode[];
}

// ── writes and ops ───────────────────────────────────────────────────────

export type WriteStmtNode =
  | {
      readonly k: 'set';
      readonly binding: string | null;
      readonly entity: string;
      readonly cond: CondNode | null;
      readonly fields: readonly (readonly [string, ExprNode])[];
    }
  | {
      readonly k: 'del';
      readonly entity: string;
      readonly cond: CondNode;
      readonly subtree: boolean;
    };

export interface ParamNode {
  readonly name: string;
  readonly ty: ScalarType;
  readonly max: ExprNode | null;
  readonly default: ExprNode | null;
}

export interface OpFlagsNode {
  readonly unbounded: boolean;
  readonly unguarded: boolean;
  readonly reproducible: boolean;
  /** `nopause`: the op keeps answering while its tenant is paused (27ar). */
  readonly nopause: boolean;
  /** `public`: the op runs on the public path. */
  readonly public: boolean;
  /** `download`: the op's rows go into a person's own download (`GET /download`). */
  readonly download?: boolean;
}

export type OpBodyNode =
  | { readonly k: 'read'; readonly pipeline: PipelineNode }
  | { readonly k: 'write'; readonly statements: readonly WriteStmtNode[] };

export interface OpNode {
  readonly kind: 'op';
  readonly name: string;
  readonly params: readonly ParamNode[];
  readonly flags: OpFlagsNode;
  readonly body: OpBodyNode;
}

export interface DomainNode {
  readonly kind: 'domain';
  readonly name: string;
  readonly version: string | null;
}

/**
 * `[not] entity(inner)`: one condition of a release rule. `inner` is a guard
 * walk whose subject is the seat being released, printed `$member`.
 */
export interface ReleaseAtomNode {
  readonly negated: boolean;
  readonly entity: string;
  readonly inner: GuardExprNode;
}

/** Release conditions in disjunctive normal form, as the parser keeps them. */
export interface ReleaseCondNode {
  readonly disjuncts: readonly (readonly ReleaseAtomNode[])[];
}

export type ReassignTargetNode =
  | { readonly k: 'owner' }
  | {
      readonly k: 'lookup';
      readonly entity: string;
      readonly inner: GuardExprNode;
      readonly field: string;
    };

export type ReleaseEffectNode =
  | { readonly k: 'remove'; readonly entity: string; readonly field: string }
  | {
      readonly k: 'reassign';
      readonly entity: string;
      readonly field: string;
      readonly to: readonly ReassignTargetNode[];
    };

/** `release member open | locked | when …` (checklist 27aq). */
export interface ReleaseNode {
  readonly kind: 'release';
  readonly subject: 'member';
  readonly mode: 'open' | 'locked' | 'when';
  readonly guard: ReleaseCondNode | null;
  readonly effects: readonly ReleaseEffectNode[];
}

/**
 * What starts a routine. The text is the canonical clause body: `every 5m`,
 * `every day 09:00Z`, or `when shop.orders.created debounce 60s coalesce`.
 * Event kinds live inside the pattern, so the closed set stays the parser's.
 */
export type RoutineTriggerNode =
  | { readonly k: 'clock'; readonly schedule: string }
  | { readonly k: 'change'; readonly pattern: string; readonly modifiers: string };

/**
 * `routine name in domain { trigger …  run(…) flags: writes }` (checklist 12a),
 * or `run(…): <pipeline> each -> writes` when `each` is set: the writes run
 * once for each row the pipeline produces.
 */
export interface RoutineNode {
  readonly kind: 'routine';
  readonly name: string;
  readonly domain: string | null;
  readonly trigger: RoutineTriggerNode;
  readonly params: readonly ParamNode[];
  readonly flags: OpFlagsNode;
  readonly each: PipelineNode | null;
  readonly statements: readonly WriteStmtNode[];
}

/** Anything `synth()` accepts, in written order. */
export type Declaration = DomainNode | EntityNode | OpNode | RoutineNode | ReleaseNode;
