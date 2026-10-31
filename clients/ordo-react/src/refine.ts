/**
 * The refinement envelope (`api-engine-design.md` §7): a filter and a pick
 * over an op's own outputs.
 *
 * Every program read accepts one, by default and with no off switch (ruled
 * 2026-10-05). The descriptor's `refine.tier` says so: `basic` for a program
 * read, `none` for a write, a platform op or a public read. This module does
 * not refuse on the tier. The door is the authority, and its refusal is
 * typed. `refinementSurface` still reads the tier, to decide whether a
 * renderer draws filter chrome.
 *
 * ## One validator
 *
 * The checker is `@hominemai/ordo-sdk`'s `checkRefine` (`src/core/refine.ts`),
 * re-exported: it takes a descriptor op as well as a generated spec (owner
 * ruling, 2026-10-23; audit-service-interfaces.md, fix order step 7).
 * `test/refine-agreement.test.ts` runs it over every read in the golden
 * descriptor in both forms and requires the same verdicts. `encodeRefine`
 * with an op is this package's 0.3 form, kept and deprecated.
 *
 * ## Typed refinements
 *
 * A hook takes the wire shape, `{ where, pick }`, which a generated UI builds
 * from state. It also takes a callback over a field proxy, the form the
 * SDK's builders use: `refine: (v) => [v.billed.gt('4000')]`. Give the hook
 * the SDK's generated `<Op>Row` and `<Op>Fields` types through
 * {@link opRef} and the callback and `pick` are checked by the compiler.
 */

import type { OpDescriptor } from './descriptor.js';

import {
  REFINE_OPS,
  SEALED_OPS,
  checkRefine,
  encodeRefine,
  fieldsProxy,
  type AnyFields,
  type Conditions,
  type DescriptorRefineOp,
  type OpenField,
  type Refine,
  type RefineCondition,
  type RefineMode,
  type RefineOp,
  type RefineTarget,
  type SealedField,
} from '@hominemai/ordo-sdk/core/refine.js';
export { REFINE_OPS, SEALED_OPS, checkRefine, encodeRefine, fieldsProxy };
export type {
  AnyFields,
  Conditions,
  DescriptorRefineOp,
  OpenField,
  Refine,
  RefineCondition,
  RefineMode,
  RefineOp,
  RefineTarget,
  SealedField,
};

export interface RefinementSurface {
  /** Whether to draw filter chrome: the tier is `basic`. */
  readonly enabled: boolean;
  /** Whether to draw sort/column chrome only: a read whose tier is `none`. */
  readonly sortOnly: boolean;
  /** The output fields a filter may name. */
  readonly fields: readonly string[];
  readonly tier: 'none' | 'basic';
  readonly why: string;
}

/**
 * What chrome a surface may draw. Named rather than inlined so a skin has one
 * place to read, and so the "sort-only" answer is a value rather than a
 * `false` a renderer has to interpret.
 */
export function refinementSurface(op: OpDescriptor): RefinementSurface {
  // One source. `call.refinable` is a projection of the tier as of `/2`, so
  // reading both would be reading one fact twice.
  const enabled = op.refine.tier !== 'none';
  const isRead = op.kind === 'read';
  return {
    enabled,
    sortOnly: !enabled && isRead,
    fields: op.refine.fields,
    tier: op.refine.tier,
    why: enabled
      ? `tier ${op.refine.tier}: every program read takes a filter and a pick`
      : isRead
        ? 'a platform or public read: the door takes declared parameters only, so sort-only chrome'
        : 'writes are never refinable',
  };
}


// ── the typed form ───────────────────────────────────────────────────────

/** A hook's `refine`: the wire shape, or a callback over the field proxy. */
export type RefineInput<F = AnyFields> = Refine | ((fields: F) => Conditions);

/**
 * An op name that carries its row and field types, for the hooks to infer
 * from. Nothing at run time: it is the name.
 *
 * ```ts
 * import type { VendorSpendRow, VendorSpendFields } from './generated';
 * const vendorSpend = opRef<VendorSpendRow, VendorSpendFields>('vendorSpend');
 * const q = useOpQuery(vendorSpend, {
 *   refine: (v) => [v.billed.gt('4000')],
 *   pick: ['name', 'billed'],
 * });
 * // q.rows: readonly Pick<VendorSpendRow, 'name' | 'billed'>[]
 * ```
 */
export type OpRef<R, F = AnyFields> = string & { readonly __ordoRow?: R; readonly __ordoFields?: F };

export function opRef<R, F = AnyFields>(name: string): OpRef<R, F> {
  return name as OpRef<R, F>;
}

/** The row a hook returns: `R`, or `Pick<R, K>` when a pick narrows it. */
export type Picked<R, K extends PropertyKey> = [keyof R] extends [K] ? R : Pick<R, Extract<K, keyof R>>;

/**
 * A hook's `refine` and `pick` options as one wire-shaped envelope, or
 * `undefined` when nothing narrows. `pick` replaces a pick in `refine`. Not
 * checked here: the network path checks it before the wire and refuses with
 * a typed refusal, which a hook reports rather than throws during render.
 */
export function resolveRefine<F>(
  refine: RefineInput<F> | undefined,
  pick: readonly string[] | undefined,
): Refine | undefined {
  let base: Refine = {};
  if (typeof refine === 'function') {
    const got = refine(fieldsProxy<F>());
    base = { where: Array.isArray(got) ? (got as readonly RefineCondition[]) : [got as RefineCondition] };
  } else if (refine) {
    base = refine;
  }
  const out: { where?: readonly RefineCondition[]; pick?: readonly string[] } = {};
  if (base.where && base.where.length > 0) out.where = base.where;
  const p = pick ?? base.pick;
  if (p !== undefined) out.pick = p;
  return out.where || out.pick ? out : undefined;
}
