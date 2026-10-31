/**
 * The shape of the generated op table.
 *
 * **Static half.** `src/generated/specs.ts` produces values of these types
 * and this file decides what the types are. The split is deliberate: a spec
 * is *data about a deployment*, so it is generated; what a spec is made of is
 * a contract, so it is hand-written and reviewed once.
 */

/** The parameter types the door binds (`api-engine-design.md` §5). */
export type ParamType =
  | 'int'
  | 'money'
  | 'decimal'
  | 'text'
  | 'date'
  | 'datetime'
  | 'duration'
  | 'bool'
  | 'id'
  | 'json';

export interface ParamSpec {
  readonly name: string;
  readonly type: string;
  readonly required: boolean;
  /** Which entity an `id` parameter addresses; `null` where the body does not pin one. */
  readonly idEntity: string | null;
  /** The value set, where the descriptor could resolve one through `field_ref`. */
  readonly enumValues: readonly string[] | null;
  /**
   * Bounds, as **canonical digit strings** — never JSON numbers.
   *
   * `@hominemai/ordo-react` found a dead branch that read these as numbers: the emitter
   * has never written one, and no descriptor populated `max` at all, so
   * nothing failed to notice. Typing them as strings here is what stops the
   * same branch reappearing.
   */
  readonly min: string | null;
  readonly max: string | null;
  /**
   * The places of a `money(p)` parameter, or the scale of a `decimal(p)`
   * (27ak). A value with more digits after the point is refused before
   * the wire, as the door refuses it. `null` (or absent) for other types.
   */
  readonly places?: number | null;
}

export interface OpSpec {
  readonly op: string;
  /** The per-op version hash, sent as `X-Ordo-Version` (§10). */
  readonly version: string;
  readonly method: string;
  readonly kind: string;
  /**
   * Present and true for a public op. A public read can be called with no
   * token: the server runs it as the anonymous principal. It takes no
   * `$refine`.
   */
  readonly public?: boolean;
  readonly terminal: readonly string[];
  readonly paged: boolean;
  readonly singleRow: boolean;
  /**
   * The parameter that asserts a revision, from `statements[].cas_param`.
   *
   * `call.cas` says *whether* a revision is asserted; only this says which
   * parameter carries it, and the core needs the name to refuse a commit
   * that omits it — before the wire, so there is no last-write-wins path
   * through this package either.
   */
  readonly casParam: string | null;
  readonly params: readonly ParamSpec[];
  readonly refusals: readonly string[];
  /** A registration failure the descriptor already knows about. */
  readonly standingError: { readonly code: string; readonly message: string } | null;
  /**
   * Whether a device can keep this op in sync (`plan-replica-feed.md` §1),
   * from `result.sync`: the entity its rows are stored rows of, and the output
   * that carries each row's id. `null` for an op that cannot be synced.
   */
  readonly sync: { readonly entity: string; readonly key: string } | null;
  /**
   * The Tier-0 refinement manifest, from the descriptor's `ops[].refine`
   * (`api-engine-design.md` §7).
   *
   * `tier` is the door's permission. Every program read is `basic` (ruled
   * 2026-10-05: refinement is a standard feature with no off switch); a
   * write, a platform op and a public read are `none`, and the door refuses
   * `$refine` on them. The client does not consult the tier before sending:
   * the door is the authority, and its refusal is typed.
   *
   * `fields` is the separate fact: which output fields a condition may
   * *name*. The device reads `fields` and not `tier` (`plan-client-cache.md`
   * C6): rows already on the device cost the server nothing.
   */
  readonly refine: {
    readonly tier: 'none' | 'basic';
    readonly fields: readonly string[];
    /**
     * The outputs whose values are exact decimals, `money` and `decimal`
     * (27ak). They arrive as decimal strings, and the device compares them
     * as numbers, exactly, as Postgres compares `numeric`.
     */
    readonly decimals?: readonly string[];
    /**
     * Every output's descriptor type (`text`, `money`, `int`, ...). A
     * condition's value is checked against it before the wire, and a `pick`
     * may name any key here. Absent on a hand-built spec, which then skips
     * the type check and picks from `fields`.
     */
    readonly types?: Readonly<Record<string, string>>;
    /** The sealed outputs. At the door a sealed condition is `eq` or `in` only. */
    readonly sealed?: readonly string[];
    /**
     * The size limits, when the descriptor states them (ruled 2026-10-05,
     * ask 6: a setting, by default 16 conditions and 200 values in one `in`).
     * `null` or absent leaves the check to the door.
     */
    readonly limits?: { readonly conditions: number; readonly inValues: number } | null;
  };
  /**
   * The order the server returns this read's rows in, from the descriptor's
   * `result.order` (27o, ruled 2026-10-01). `keys` name output fields, the
   * row's id last, so the device copy can return what it holds in the same
   * order. `stated` is `false` where the language cannot state the order (a
   * grouped result, a sort key the device cannot evaluate); the keys are
   * then the id alone and the device keeps id order. `null` for a write.
   */
  readonly order: ReadOrder | null;
}

/** One key of a read's stated order: an output field and its direction. */
export interface OrderKey {
  readonly path: string;
  readonly desc: boolean;
}

/** A read's total order as the descriptor states it. */
export interface ReadOrder {
  readonly keys: readonly OrderKey[];
  readonly stated: boolean;
}
