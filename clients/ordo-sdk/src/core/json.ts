/**
 * A JSON value: what a `json` field holds and how it travels (checklist 28b).
 *
 * A document goes to the door as JSON and comes back as JSON, never as a
 * string holding JSON. This type is that set exactly, so a caller cannot
 * hand the client a `Date`, a `Map`, a function or `undefined`, each of
 * which `JSON.stringify` would quietly change or drop.
 */
export type JsonValue = null | boolean | number | string | readonly JsonValue[] | { readonly [key: string]: JsonValue };

/**
 * Is `v` a JSON value that survives the wire unchanged?
 *
 * Finite numbers only (`NaN` and `Infinity` serialize as `null`), plain
 * objects and arrays only, and no `undefined` anywhere. Iterative, so a
 * deep document cannot exhaust the stack proving it is fine. The door's
 * size and depth limits are the deployment's and are checked there.
 */
export function isJsonValue(v: unknown): v is JsonValue {
  const stack: unknown[] = [v];
  while (stack.length > 0) {
    const x = stack.pop();
    if (x === null || typeof x === 'boolean' || typeof x === 'string') continue;
    if (typeof x === 'number') {
      if (!Number.isFinite(x)) return false;
      continue;
    }
    if (Array.isArray(x)) {
      for (const i of x) stack.push(i);
      continue;
    }
    if (typeof x === 'object') {
      const proto = Object.getPrototypeOf(x) as unknown;
      if (proto !== Object.prototype && proto !== null) return false;
      for (const i of Object.values(x as Record<string, unknown>)) stack.push(i);
      continue;
    }
    return false;
  }
  return true;
}
