/**
 * Values: the plumbing under every expression and condition.
 *
 * **Skeleton half.** What is here is the *shape* of a value — that it wraps
 * one `ExprNode`, that a field path is built by dotting, that a condition
 * wraps one `CondNode`. What is NOT here is any operator, function or
 * aggregate name: those are the generated vocabulary in
 * `src/generated/lang.ts`.
 *
 * # The one seam between the halves
 *
 * {@link Value} is declared here as an interface with **no members**, and
 * `src/generated/lang.ts` fills it in by declaration merging. So the skeleton
 * can say "a parameter is a Value" and "`f.total` is a Value" without knowing
 * a single operator name, and the vocabulary decides what a Value can do.
 *
 * The run-time half of the same seam is {@link installValue}: the generated
 * file hands over its `Val` constructor at import time, and every value this
 * module mints — a field path, a parameter — is built with it. Without that,
 * `p.cap.eq(…)` would type-check against the merged interface and find no
 * method at run time, which is the worst of both.
 *
 * It is one mutable module-level binding, and it is the only one in the
 * package. The alternative was threading a constructor through every builder
 * signature, which would put the vocabulary's name in forty skeleton
 * signatures — the coupling this design exists to avoid.
 */

import type { CondNode, ExprNode } from './ir.js';

/** A value: one expression node, and whatever the vocabulary adds. */
export class ValueBase {
  constructor(readonly node: ExprNode) {}
}

/**
 * The value type as the skeleton sees it.
 *
 * Empty here; `src/generated/lang.ts` merges the operator methods in. A
 * reader looking for `.eq()` should look there — and the fact that it is not
 * here is the point: the operator set is the parser's, not this file's.
 */
// eslint-disable-next-line @typescript-eslint/no-empty-interface
export interface Value extends ValueBase {}

/** A condition: one condition node. */
export class Cond {
  constructor(readonly node: CondNode) {}
}

type Ctor = new (node: ExprNode) => Value;

let ctor: Ctor = ValueBase as unknown as Ctor;

/**
 * Install the vocabulary's value constructor. Called once, by
 * `src/generated/lang.ts`, at import time.
 */
export function installValue(c: Ctor): void {
  ctor = c;
}

/** Mint a value through whatever constructor the vocabulary installed. */
export function value(node: ExprNode): Value {
  return new ctor(node);
}

/**
 * The method names a field proxy must NOT swallow.
 *
 * A path is built by property access (`f.customer.name`), and the same object
 * carries the operator methods (`f.total.gt(…)`). A field genuinely called
 * `eq` would therefore be unreachable through the proxy — so it is reachable
 * through {@link path} instead, and the escape is documented rather than left
 * to be discovered. The list is filled in by the generated vocabulary at
 * import time, because it *is* the vocabulary: the words that are methods are
 * exactly the words the generator emitted.
 */
export const RESERVED_ACCESSORS = new Set<string>();

/** Register the vocabulary's method names, so the proxy stops shadowing. */
export function reserveAccessors(names: Iterable<string>): void {
  for (const n of names) RESERVED_ACCESSORS.add(n);
}

/** An explicit dotted path — the escape when a field name is a method name. */
export function pathNode(segments: readonly string[]): ExprNode {
  if (segments.length === 0) throw new Error('a path needs at least one segment');
  return { k: 'path', segments: [...segments] };
}

/**
 * Wrap a value so that unknown property access extends its path.
 *
 * Only `path` nodes are extensible: `f.customer.name` is a dot-path, and
 * `lit(1).name` is not a thing the language has, so the proxy leaves every
 * other node alone.
 */
function extend(v: Value): Value {
  if (v.node.k !== 'path') return v;
  const segments = v.node.segments;
  return new Proxy(v, {
    get(target, prop, receiver) {
      if (typeof prop !== 'string') return Reflect.get(target, prop, receiver);
      if (prop in target || RESERVED_ACCESSORS.has(prop)) {
        return Reflect.get(target, prop, receiver);
      }
      return extend(value(pathNode([...segments, prop])));
    },
  });
}

/** `path('customer', 'name')` — the explicit spelling of `f.customer.name`. */
export function makePath(segments: readonly string[]): Value {
  return extend(value(pathNode(segments)));
}

/**
 * The untyped field namespace: every property answers with a path, and so
 * does every property of that.
 *
 * Recursive because dot-paths are: `f.customer.name` is one path with two
 * segments, and `take[plants.id] 1` is a path whose first segment names a
 * source. A one-level `Record<string, Value>` would type the first hop and
 * refuse the second, which is the opposite of what the grammar allows.
 */
export type AnyFields = { readonly [k: string]: Value & AnyFields };

/**
 * `f` — the field namespace. `f.total`, `f.customer.name`.
 *
 * Untyped here on purpose: this is the schema-free core, and a schema-free
 * builder cannot know a deployment's field names. The generated
 * `src/generated/schema.ts` re-types the same object against one
 * `interface.json`, so an author with a descriptor gets completion and
 * ref-following and an author without one still gets a working builder.
 */
export function makeFields(): AnyFields {
  return new Proxy(Object.create(null) as AnyFields, {
    get(_t, prop) {
      if (typeof prop !== 'string') return undefined;
      return makePath([prop]) as Value & AnyFields;
    },
    // `'x' in f` must answer for any field name, or a probe would report the
    // namespace as empty and a caller would conclude the schema had none.
    has() {
      return true;
    },
  });
}

/** `param('caller')` — a parameter reference by name. */
export function param(name: string): Value {
  return value({ k: 'param', name });
}

/**
 * `p` — the parameter namespace inside an op body, built from the op's own
 * declared parameter names.
 */
export function makeParams(names: readonly string[]): Record<string, Value> {
  const out: Record<string, Value> = Object.create(null) as Record<string, Value>;
  for (const n of names) out[n] = param(n);
  return out;
}

/** Digits for an integer literal, refusing anything a `number` would round. */
export function intDigits(v: number | bigint | string): string {
  if (typeof v === 'string') {
    if (!/^-?\d+$/.test(v)) throw new Error(`not an integer literal: ${v}`);
    return v;
  }
  if (typeof v === 'bigint') return v.toString();
  if (!Number.isSafeInteger(v)) {
    throw new Error(
      `${v} is not a safe integer: Ordo's \`int\` is exact, so pass a bigint or a digit string`,
    );
  }
  return v.toString();
}
