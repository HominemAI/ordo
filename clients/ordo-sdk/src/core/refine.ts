/**
 * The refinement envelope, checked once for every place it is evaluated.
 *
 * A refinement is a filter and a pick over an op's own outputs
 * (`api-engine-design.md` §7, `plan-refinement.md` §5). Every program read
 * accepts one, by default and with no off switch (owner, 2026-10-05).
 *
 * Two evaluators read the same envelope: the door, over the wire, and the
 * device copy, over synced rows (`replica/refine.ts`). Both are checked here,
 * by one function, so the two cannot drift. The device path calls
 * {@link checkRefine} with `mode: 'device'`; the builder in `core/call.ts`
 * calls it with `mode: 'door'`.
 *
 * ## What the two modes share
 *
 * - The closed operator set: `eq ne lt le gt ge in is_null`.
 * - The field manifest. A condition names one of `refine.fields`; a pick names
 *   any output.
 * - The value shapes: one non-null scalar, an array for `in`, nothing for
 *   `is_null`.
 * - The value types, when the spec carries them (`refine.types`): `int` and
 *   `duration` take an integer, `bool` a boolean, `money` and `decimal` a
 *   decimal string or an integer (never a float), everything else a string.
 * - Text is NFC, as the door makes it before it compares.
 *
 * ## What only the door adds
 *
 * - **Sealed fields take `eq` and `in` only.** A sealed condition rides the
 *   blind index, which answers "same digest?" and nothing else. The device
 *   holds opened values, so the rule protects nothing there.
 * - **The size limits**, at most 16 conditions and 200 values in one `in` by
 *   default, are settings on the server (ruled 2026-10-05, ask 6). The client
 *   checks them only when the spec carries them (`refine.limits`); otherwise
 *   the door decides.
 *
 * A value finer than a money field's places is NOT refused. A comparison
 * value is never stored, so `price le "2.505"` on a `money(2)` is a fair
 * question, and the shared parity table proves the server answers it.
 *
 * Refusals wear `invalid_refinement`, the door's own class, because the check
 * is the door's check run early.
 */

import { OrdoLocalRefusal } from './refusal.js';
import type { OpSpec } from './spec.js';

/**
 * An op as the interface descriptor states it (`ordo.interface`), the part a
 * refinement reads: `@hominemai/ordo-react`'s `OpDescriptor` is one. Its
 * outputs carry their types and the sealed mark, where an {@link OpSpec}
 * carries them in its `refine` manifest.
 */
export interface DescriptorRefineOp {
  readonly op: string;
  readonly kind: string;
  readonly refine: {
    readonly fields: readonly string[];
    readonly limits?: { readonly conditions: number; readonly in_values: number } | null | undefined;
  };
  readonly result: {
    readonly outputs: readonly { readonly name: string; readonly type: string | null; readonly sealed: boolean }[];
  };
}

/** What {@link checkRefine} checks against: a generated spec, or a descriptor op. */
export type RefineTarget = Pick<OpSpec, 'op' | 'kind' | 'refine'> | DescriptorRefineOp;

/** The spec-shaped manifest of either form. One set of rules reads it. */
function specOf(target: RefineTarget): Pick<OpSpec, 'op' | 'kind' | 'refine'> {
  if (!('result' in target)) return target;
  const types: Record<string, string> = {};
  // An output of no stated type takes any value, as a spec without a type
  // table does; it is still one a pick may name.
  for (const o of target.result.outputs) types[o.name] = o.type ?? '';
  return {
    op: target.op,
    kind: target.kind,
    refine: {
      tier: 'basic',
      fields: target.refine.fields,
      types,
      sealed: target.result.outputs.filter((o) => o.sealed).map((o) => o.name),
      limits: target.refine.limits
        ? { conditions: target.refine.limits.conditions, inValues: target.refine.limits.in_values }
        : null,
    },
  };
}

/** The closed Tier-0 operator set (`api-engine-design.md` §7). */
export const REFINE_OPS = ['eq', 'ne', 'lt', 'le', 'gt', 'ge', 'in', 'is_null'] as const;
export type RefineOp = (typeof REFINE_OPS)[number];

/** The two operators a sealed field takes: the blind index answers equality only. */
export const SEALED_OPS: readonly RefineOp[] = ['eq', 'in'];

/** One condition, in the wire's own shape. */
export interface RefineCondition<F extends string = string> {
  readonly field: F;
  readonly op: RefineOp;
  /** Omitted for `is_null`; an array for `in`; one non-null scalar otherwise. */
  readonly value?: unknown;
}

/**
 * The envelope as the door reads it: `$refine={"where":[...],"pick":[...]}`.
 * Conditions combine as AND. `pick`, when present, is non-empty.
 */
export interface Refine {
  readonly where?: readonly RefineCondition[];
  readonly pick?: readonly string[];
}

/** Where a refinement will be evaluated. */
export type RefineMode = 'door' | 'device';

/** The size limits, when the descriptor states them. */
export interface RefineLimits {
  /** At most this many conditions in one refinement. */
  readonly conditions: number;
  /** At most this many values in one `in`. */
  readonly inValues: number;
}

function refuse(op: string, message: string, param: string | null = null): never {
  throw new OrdoLocalRefusal({ class: 'invalid_refinement', message, op, param });
}

// The door's own shape (`ordo-api/src/bind.rs`, `is_decimal`): digits, an
// optional point and digits, a leading minus and never a plus.
const DECIMAL = /^-?\d+(\.\d+)?$/;

/**
 * An exact decimal's digits: a decimal string as given, an integer as its
 * digits. `null` for anything else, a float included: a float has already
 * been rounded, so it is not a value an exact comparison can use.
 */
export function asDecimal(v: unknown): string | null {
  if (typeof v === 'string' && DECIMAL.test(v)) return v;
  if (typeof v === 'number' && Number.isSafeInteger(v)) return String(v);
  return null;
}

/** A filter value in its stored form: strings are NFC, as the door makes them. */
export function storedForm(value: unknown): unknown {
  if (typeof value === 'string') return value.normalize('NFC');
  if (Array.isArray(value)) return value.map(storedForm);
  return value;
}

/** Why `v` is not a value of type `ty`, or `null` when it is. */
function typeError(ty: string, v: unknown): string | null {
  switch (ty) {
    case 'int':
    case 'duration':
      return typeof v === 'number' && Number.isSafeInteger(v) ? null : `is \`${ty}\`: compare it with an integer`;
    case 'bool':
      return typeof v === 'boolean' ? null : 'is `bool`: compare it with true or false';
    case 'money':
    case 'decimal':
      return asDecimal(v) !== null
        ? null
        : `is an exact decimal (\`${ty}\`): compare it with a decimal string like "29.99", never a float`;
    case 'text':
    case 'date':
    case 'datetime':
    case 'id':
      return typeof v === 'string' ? null : `is \`${ty}\`: compare it with a string`;
    default:
      // A type this client was generated before. The door is the authority.
      return null;
  }
}

/**
 * Check a refinement against the op and return it in its wire form: text
 * NFC, `value` omitted for `is_null`, empty parts dropped.
 *
 * Throws {@link OrdoLocalRefusal} with class `invalid_refinement`, naming the
 * field in `param`.
 */
export function checkRefine(target: RefineTarget, refine: Refine, mode: RefineMode): Refine {
  const spec = specOf(target);
  const op = spec.op;
  if (spec.kind !== 'read') refuse(op, `${op} is a ${spec.kind} op and writes are never refinable`);

  const manifest = spec.refine;
  const known = new Set(manifest.fields);
  const types = manifest.types ?? {};
  const outputs = new Set([...manifest.fields, ...Object.keys(types)]);
  const sealed = new Set(manifest.sealed ?? []);
  const decimals = new Set(manifest.decimals ?? []);
  const limits = manifest.limits ?? null;

  const where = refine.where ?? [];
  if (mode === 'door' && limits && where.length > limits.conditions) {
    refuse(
      op,
      `a refinement takes at most ${limits.conditions} conditions and this one has ${where.length} ` +
        '(a deployment setting the descriptor states)',
    );
  }

  const checkValue = (c: RefineCondition, v: unknown): void => {
    if (v === null) return;
    const ty = types[c.field];
    if (ty !== undefined) {
      const why = typeError(ty, v);
      if (why) refuse(op, `\`${c.field}\` ${why}, not ${JSON.stringify(v)}`, c.field);
      return;
    }
    // No type table: an exact decimal output still refuses a float (27ak).
    if (decimals.has(c.field) && asDecimal(v) === null) {
      refuse(op, `\`${c.field}\` is an exact decimal: compare it with a decimal string like "29.99", never a float`, c.field);
    }
  };

  const out: RefineCondition[] = [];
  for (const c of where) {
    if (!known.has(c.field)) {
      if (sealed.has(c.field)) {
        refuse(
          op,
          `\`${c.field}\` is a sealed field and is not refinable: refining a sealed field is opt-in at ` +
            'its declaration (`sealed indexed refinable`)',
          c.field,
        );
      }
      refuse(
        op,
        `unknown refinement field ${JSON.stringify(c.field)} in where; ${op} offers [${[...known].join(', ')}]`,
        c.field,
      );
    }
    if (!(REFINE_OPS as readonly string[]).includes(c.op)) {
      refuse(
        op,
        `unknown refinement operator ${JSON.stringify(c.op)}; the Tier-0 set is [${REFINE_OPS.join(', ')}]`,
        c.field,
      );
    }
    if (mode === 'door' && sealed.has(c.field) && !SEALED_OPS.includes(c.op)) {
      refuse(
        op,
        `sealed field \`${c.field}\` accepts equality only (\`eq\`, \`in\`): a sealed condition rides the ` +
          'blind index, and ordering over sealed values is an oracle',
        c.field,
      );
    }
    if (c.op === 'is_null') {
      if (c.value !== undefined && c.value !== null) refuse(op, `\`is_null\` on \`${c.field}\` takes no value`, c.field);
      out.push({ field: c.field, op: c.op });
      continue;
    }
    if (c.op === 'in') {
      if (!Array.isArray(c.value)) refuse(op, `\`in\` on \`${c.field}\` takes an array of values`, c.field);
      const values = c.value as readonly unknown[];
      if (mode === 'door' && limits && values.length > limits.inValues) {
        refuse(
          op,
          `\`in\` on \`${c.field}\` takes at most ${limits.inValues} values and this one has ${values.length} ` +
            '(a deployment setting the descriptor states)',
          c.field,
        );
      }
      for (const v of values) {
        if (v !== null && typeof v === 'object') refuse(op, `\`in\` on \`${c.field}\` takes scalar values`, c.field);
        checkValue(c, v);
      }
    } else if (c.value === undefined || c.value === null || typeof c.value === 'object') {
      refuse(op, `\`${c.op}\` on \`${c.field}\` needs one non-null scalar value (null tests are \`is_null\`)`, c.field);
    } else {
      checkValue(c, c.value);
    }
    out.push({ field: c.field, op: c.op, value: storedForm(c.value) });
  }

  let pick: string[] | undefined;
  if (refine.pick !== undefined) {
    if (refine.pick.length === 0) refuse(op, '`pick`, when present, must name at least one field');
    for (const [i, field] of refine.pick.entries()) {
      if (!outputs.has(field)) {
        refuse(
          op,
          `unknown refinement field ${JSON.stringify(field)} in pick; ${op} returns [${[...outputs].join(', ')}]`,
          field,
        );
      }
      // The door refuses a repeated name (its projection would carry the
      // column twice), so the device refuses it too.
      if (refine.pick.indexOf(field) !== i) refuse(op, `\`pick\` names \`${field}\` twice`, field);
    }
    pick = [...refine.pick];
  }

  const result: { where?: RefineCondition[]; pick?: string[] } = {};
  if (out.length > 0) result.where = out;
  if (pick) result.pick = pick;
  return result;
}

/**
 * The `$refine` query value: the envelope as JSON, `where` then `pick`, each
 * condition `field`, `op`, `value`. `URLSearchParams` does the URL encoding.
 * `null` when the refinement narrows nothing, so no key is sent.
 */
export function encodeRefine(refine: Refine | undefined): string | null;
/**
 * @deprecated `@hominemai/ordo-react` 0.3's form: check a refinement against
 * a descriptor op and answer the checked envelope, or `undefined` when it
 * narrows nothing. Call `checkRefine(op, refine, 'door')`.
 */
export function encodeRefine(op: DescriptorRefineOp, refine: Refine): Refine;
export function encodeRefine(
  first: Refine | DescriptorRefineOp | undefined,
  second?: Refine,
): string | null | Refine {
  if (second !== undefined) return checkRefine(first as DescriptorRefineOp, second, 'door');
  const refine = first as Refine | undefined;
  if (!refine) return null;
  const where = refine.where ?? [];
  if (where.length === 0 && refine.pick === undefined) return null;
  const body: { where?: RefineCondition[]; pick?: readonly string[] } = {};
  if (where.length > 0) {
    body.where = where.map((c) =>
      c.op === 'is_null' ? { field: c.field, op: c.op } : { field: c.field, op: c.op, value: c.value },
    );
  }
  if (refine.pick !== undefined) body.pick = refine.pick;
  return JSON.stringify(body);
}

// ── the typed proxy ──────────────────────────────────────────────────────

/**
 * One output field, offered to `.where(v => ...)`. Every operator, typed by
 * the field's value type `V`. `isNull()` is the wire's `is_null`.
 */
export interface OpenField<F extends string, V> {
  eq(value: V): RefineCondition<F>;
  ne(value: V): RefineCondition<F>;
  lt(value: V): RefineCondition<F>;
  le(value: V): RefineCondition<F>;
  gt(value: V): RefineCondition<F>;
  ge(value: V): RefineCondition<F>;
  in(values: readonly (V | null)[]): RefineCondition<F>;
  isNull(): RefineCondition<F>;
}

/**
 * A sealed field opted in to refinement (`sealed indexed refinable`): `eq` and
 * `in` only, because a sealed condition rides the blind index.
 */
export interface SealedField<F extends string, V> {
  eq(value: V): RefineCondition<F>;
  in(values: readonly V[]): RefineCondition<F>;
}

/** What a `.where` callback may return: one condition, or several to AND. */
export type Conditions = RefineCondition | readonly RefineCondition[];

/** The field proxy for an op with no generated type: any name, any value. */
export type AnyFields = Readonly<Record<string, OpenField<string, unknown>>>;

function field(name: string): OpenField<string, unknown> {
  const cond = (op: RefineOp, value: unknown): RefineCondition => ({ field: name, op, value });
  return {
    eq: (v) => cond('eq', v),
    ne: (v) => cond('ne', v),
    lt: (v) => cond('lt', v),
    le: (v) => cond('le', v),
    gt: (v) => cond('gt', v),
    ge: (v) => cond('ge', v),
    in: (vs) => cond('in', vs),
    isNull: () => ({ field: name, op: 'is_null' }),
  };
}

/**
 * The runtime half of a field proxy. Any property is a field, so a name the
 * types did not catch (plain JavaScript, a cast) reaches {@link checkRefine}
 * and is refused there by name, never as a `TypeError` on `undefined`. A
 * sealed field's proxy has every operator at run time; the check refuses the
 * ones it does not take.
 */
export function fieldsProxy<F>(): F {
  return new Proxy({} as Record<string, unknown>, {
    get: (_target, key) => (typeof key === 'string' ? field(key) : undefined),
  }) as F;
}

/** Flatten what a `.where` callback returned. */
export function conditionsOf(got: Conditions): readonly RefineCondition[] {
  return Array.isArray(got) ? (got as readonly RefineCondition[]) : [got as RefineCondition];
}
