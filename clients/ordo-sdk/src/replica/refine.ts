/**
 * Answering new questions on the device (`plan-client-cache.md` C6).
 *
 * The replica holds rows a synced op returned (C4). This filters, picks and
 * sorts them **locally**, with the Tier-0 operators Ordo already defines
 * (`api-engine-design.md` §7), so typing into a filter or re-sorting a column
 * asks the server for nothing.
 *
 * ## What is the same as the door, and what is not
 *
 * The operator set, the AND-only combination, the value shapes and the
 * unknown-field refusal are the door's own rules. They are checked by one
 * function, `checkRefine` in `core/refine.ts`, which the wire builder calls
 * too, so one envelope means one thing wherever it is evaluated.
 *
 * Three things differ, each for a stated reason:
 *
 * - **`refine.tier` is not consulted.** The tier governs work the *server*
 *   does. Rows already on the device cost the server nothing. Since the
 *   2026-10-05 ruling every program read is `basic`, so the device and the
 *   door now say yes to the same ops and this difference no longer shows.
 * - **Sorting is local only.** The envelope has no sort key and never has:
 *   ordering belongs to the op's compiled query, and `ordo-api/src/page.rs`
 *   depends on refinement not touching it, because a cursor addresses a row
 *   by sort key. Sorting rows already on the device moves no cursor, so it is
 *   safe here and has no wire form to match.
 * - **Sealed-field rules do not apply.** They stop an oracle over values the
 *   server holds encrypted; the device only ever receives values the executor
 *   already opened, so there is nothing here those rules would protect.
 *
 * ## Comparing values
 *
 * Rows arrive typed by `canonical()` in the executor: `int` and `duration`
 * are JSON numbers, `bool` a boolean, `money` and `decimal` exact decimal
 * strings (`"29.99"`, 27ak), everything else (text, date, datetime, id) a
 * string, and NULL a null. Comparison follows the type: numbers numerically,
 * strings by code point, which is the right order for the ISO-8601 dates and
 * datetimes Ordo emits. Two values of different types never order, because
 * the server would never have been asked to compare them.
 *
 * **Exact decimals are the one place the JSON type is not the type.** A
 * money is a string on the wire so it is never rounded, and comparing
 * `"100.00"` with `"29.99"` by code point would put 100 below 29. So the
 * op's spec names its decimal outputs (`refine.decimals`), and those compare
 * as numbers, exactly, with no float anywhere: `"29.90"` equals `"29.9"`, as
 * it does in Postgres's `numeric`. A filter value for one is a decimal string
 * or an integer; a float is refused, as the client refuses one for a money
 * parameter.
 *
 * ## Parity with the server
 *
 * The device must give the answer the server gives, so the server's rules are
 * copied here and pinned by `test/refine-parity.test.ts`, which shares its
 * case table with the server's own test:
 *
 * - **Two-valued null logic** (`definition-language.md` §4). `null == null` is
 *   true, `x != y` is exactly `not (x == y)` so `null != 5` is true, and an
 *   ordering test with a null operand is false. `in` is a chain of `==`, so a
 *   null in the list matches a null value.
 * - **Text orders by code point** (`COLLATE "C"`), not by UTF-16 code unit.
 *   The two disagree when a character outside the basic plane, such as an
 *   emoji, meets one between U+E000 and U+FFFF.
 * - **A filter value is NFC**, as the door makes it before it compares. Stored
 *   text is already NFC, so only the value the app passes is normalized.
 * - **Sorts put nulls last in both directions**, then break ties by `id`.
 */

import type { Row } from '../core/client.js';
import { compareDecimal } from '../core/client.js';
import { asDecimal, checkRefine, storedForm, type RefineCondition } from '../core/refine.js';
import { OrdoLocalRefusal } from '../core/refusal.js';
import type { OpSpec, ReadOrder } from '../core/spec.js';
import type { LocalReplica } from './replica.js';

export { REFINE_OPS } from '../core/refine.js';
export type { RefineCondition, RefineOp } from '../core/refine.js';

export interface SortKey {
  readonly field: string;
  readonly direction?: 'asc' | 'desc';
}

/**
 * A question asked of rows already on the device.
 *
 * `where` and `pick` are the wire envelope's own shape. `sort` has no wire
 * form and is device-only; see the note above.
 */
export interface LocalQuery {
  readonly where?: readonly RefineCondition[];
  /** A projection subset. When present, must be non-empty. */
  readonly pick?: readonly string[];
  /** Nulls sort last in both directions and ties fall to `id`, as on the server. */
  readonly sort?: readonly SortKey[];
}

/** Refuse the way the door refuses, before anything is scanned. */
function refuse(op: string, message: string, param: string | null = null): never {
  throw new OrdoLocalRefusal({ class: 'invalid_refinement', message, op, param });
}

/**
 * Check a query against the op's manifest. Separated from the scan so a
 * caller can validate a filter as it is typed without touching the store.
 *
 * `where` and `pick` go through the one validator the wire path uses
 * (`core/refine.ts`, device mode), so the device and the builder refuse the
 * same envelopes with the same messages. Only `sort` is checked here, since
 * it has no wire form.
 */
export function checkQuery(spec: OpSpec, query: LocalQuery): void {
  const envelope: { where?: readonly RefineCondition[]; pick?: readonly string[] } = {};
  if (query.where !== undefined) envelope.where = query.where;
  if (query.pick !== undefined) envelope.pick = query.pick;
  checkRefine(spec, envelope, 'device');
  const known = new Set(spec.refine.fields);
  for (const key of query.sort ?? []) {
    if (known.has(key.field)) continue;
    refuse(
      spec.op,
      `unknown refinement field ${JSON.stringify(key.field)} in sort; ` +
        `${spec.op} offers [${[...known].join(', ')}]`,
      key.field,
    );
  }
}

/** Does one row satisfy every condition? Conditions combine as AND, and only AND. */
function matches(row: Row, conditions: readonly RefineCondition[], decimals: ReadonlySet<string>): boolean {
  return conditions.every((c) => {
    const value = (row as Record<string, unknown>)[c.field] ?? null;
    const exact = decimals.has(c.field);
    switch (c.op) {
      case 'is_null':
        return value === null;
      case 'eq':
        return same(value, c.value, exact);
      case 'ne':
        // Exactly `not (x == y)`, so a null is unequal to everything but null.
        return !same(value, c.value, exact);
      case 'in':
        // A chain of `==`: a null in the list matches a null value.
        return Array.isArray(c.value) && c.value.some((v) => same(value, v, exact));
      default: {
        const ord = compare(value, c.value, exact);
        if (ord === null) return false;
        return c.op === 'lt' ? ord < 0 : c.op === 'le' ? ord <= 0 : c.op === 'gt' ? ord > 0 : ord >= 0;
      }
    }
  });
}

/** Two-valued equality: `null == null` is true and nothing else equals null. */
function same(a: unknown, b: unknown, exact = false): boolean {
  const x = a ?? null;
  const y = b ?? null;
  if (exact && x !== null && y !== null) {
    // `"29.90"` is `"29.9"`: equal as numbers, as `numeric` says (27ak).
    const dx = asDecimal(x);
    const dy = asDecimal(y);
    if (dx !== null && dy !== null) return compareDecimal(dx, dy) === 0;
  }
  return x === y;
}

/**
 * Order two strings by code point, which is `COLLATE "C"` over UTF-8.
 *
 * `<` compares UTF-16 code units, so a surrogate pair (U+D800 to U+DFFF)
 * sorts below U+E000 to U+FFFF although its code point is above them. When
 * both code units are at least U+D800, lifting the surrogates above the rest
 * restores code point order without decoding the string.
 */
export function compareText(a: string, b: string): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    let x = a.charCodeAt(i);
    let y = b.charCodeAt(i);
    if (x === y) continue;
    if (x >= 0xd800 && y >= 0xd800) {
      x = x >= 0xe000 ? x - 0x800 : x + 0x2000;
      y = y >= 0xe000 ? y - 0x800 : y + 0x2000;
    }
    return x < y ? -1 : 1;
  }
  return a.length < b.length ? -1 : a.length > b.length ? 1 : 0;
}

/** Order two values, or `null` when they do not order at all. */
function compare(a: unknown, b: unknown, exact = false): number | null {
  if (a === null || a === undefined || b === null || b === undefined) return null;
  if (exact) {
    // Money and decimals order as numbers, exactly (27ak): never by code
    // point, which would put "100.00" below "29.99".
    const x = asDecimal(a);
    const y = asDecimal(b);
    return x !== null && y !== null ? compareDecimal(x, y) : null;
  }
  if (typeof a === 'number' && typeof b === 'number') return a < b ? -1 : a > b ? 1 : 0;
  if (typeof a === 'string' && typeof b === 'string') return compareText(a, b);
  if (typeof a === 'boolean' && typeof b === 'boolean') return a === b ? 0 : a ? 1 : -1;
  // Different types never order. The server was never asked to compare them,
  // because a column has one type.
  return null;
}

// ── the plan of one refinement, shared by every path below ─────────────

interface SortPlan {
  readonly field: string;
  readonly sign: 1 | -1;
  readonly exact: boolean;
}

interface Plan {
  readonly exact: ReadonlySet<string>;
  readonly conditions: readonly RefineCondition[];
  readonly keys: readonly SortPlan[];
  readonly pick: readonly string[] | undefined;
}

function planOf(query: LocalQuery, order: ReadOrder | null | undefined, decimals: readonly string[]): Plan {
  const exact = new Set(decimals);
  const conditions = (query.where ?? []).map((c) =>
    c.value === undefined ? c : { ...c, value: storedForm(c.value) },
  );
  // The server's order is total: the declared keys, then the row's id.
  const idKey = order?.keys.at(-1)?.path ?? 'id';
  const keys: SortKey[] = [...(query.sort ?? [])];
  if (keys.length > 0) keys.push({ field: idKey });
  else if (order?.stated) {
    for (const k of order.keys) keys.push({ field: k.path, direction: k.desc ? 'desc' : 'asc' });
  }
  return {
    exact,
    conditions,
    keys: keys.map((k) => ({ field: k.field, sign: k.direction === 'desc' ? -1 : 1, exact: exact.has(k.field) })),
    pick: query.pick,
  };
}

// ── sorting: each key parsed once per row (H2) ─────────────────────────

/** A decimal value that is not a decimal: it orders with nothing, as `compare` says. */
const UNORDERED = Symbol('unordered');
/** A string holding a code unit at or above U+D800, where `<` and code point order can differ. */
const HIGH = /[\uD800-￿]/;
/** Digits a double holds exactly, with room to spare. */
const EXACT_DIGITS = 15;

/** One sort key's value for every row, parsed once. */
interface Column {
  readonly values: unknown[];
  /** For text: 1 where the value needs code point comparison. */
  readonly high: Uint8Array;
  readonly sign: 1 | -1;
  /** Set when the column is read row by row as the sort reaches it. */
  readonly lazy?: { readonly rows: readonly Row[]; readonly field: string };
}

/**
 * The first pass over a decimal column: each value's digits and scale, the
 * largest scale, and the most integer digits. The second pass
 * ({@link liftDecimals}) puts every value on the largest scale, so two
 * values compare as two integers, exactly.
 */
interface DecimalScan {
  readonly digits: (string | undefined)[];
  readonly scales: Int32Array;
  max: number;
  width: number;
}

function newScan(n: number): DecimalScan {
  return { digits: new Array<string | undefined>(n), scales: new Int32Array(n), max: 0, width: 0 };
}

function scanDecimals(
  rows: readonly Row[],
  field: string,
  values: unknown[],
  scan: DecimalScan,
  from: number,
  to: number,
): void {
  for (let i = from; i < to; i++) {
    const v = (rows[i] as Record<string, unknown>)[field] ?? null;
    if (v === null) {
      values[i] = null;
      continue;
    }
    const d = asDecimal(v);
    if (d === null) {
      values[i] = UNORDERED;
      continue;
    }
    const dot = d.indexOf('.');
    const digits = dot < 0 ? d : d.slice(0, dot) + d.slice(dot + 1);
    const scale = dot < 0 ? 0 : d.length - dot - 1;
    scan.digits[i] = digits;
    scan.scales[i] = scale;
    if (scale > scan.max) scan.max = scale;
    const width = digits.length - scale;
    if (width > scan.width) scan.width = width;
  }
}

function liftDecimals(values: unknown[], scan: DecimalScan, from: number, to: number): void {
  // Every value fits in `width + max` digits once lifted; up to 15, a double
  // holds it exactly and compares fastest. Past that, a BigInt.
  const small = scan.width + scan.max <= EXACT_DIGITS;
  for (let i = from; i < to; i++) {
    const digits = scan.digits[i];
    if (digits === undefined) continue;
    const up = scan.max - scan.scales[i]!;
    values[i] = small ? Number(digits) * 10 ** up : BigInt(digits) * 10n ** BigInt(up);
  }
}

function plainRange(
  rows: readonly Row[],
  field: string,
  values: unknown[],
  high: Uint8Array,
  from: number,
  to: number,
): void {
  for (let i = from; i < to; i++) {
    const v = (rows[i] as Record<string, unknown>)[field] ?? null;
    values[i] = v;
    if (typeof v === 'string' && HIGH.test(v)) high[i] = 1;
  }
}

/**
 * The sort keys' columns. The first, and every decimal key, is parsed for
 * every row up front. A later text or number key is read only for the rows
 * whose earlier keys tie, which in most sorts is few of them.
 */
function columnsOf(rows: readonly Row[], keys: readonly SortPlan[]): Column[] {
  const n = rows.length;
  return keys.map((k, at) => {
    const values = new Array<unknown>(n);
    const high = new Uint8Array(k.exact ? 0 : n);
    if (k.exact) {
      const scan = newScan(n);
      scanDecimals(rows, k.field, values, scan, 0, n);
      liftDecimals(values, scan, 0, n);
    } else if (at > 0) {
      return { values, high, sign: k.sign, lazy: { rows, field: k.field } };
    } else {
      plainRange(rows, k.field, values, high, 0, n);
    }
    return { values, high, sign: k.sign };
  });
}

/** A column's value for one row, read now if the column is read lazily. */
function valueAt(col: Column, i: number): unknown {
  const v = col.values[i];
  if (v !== undefined || !col.lazy) return v;
  const read = (col.lazy.rows[i] as Record<string, unknown>)[col.lazy.field] ?? null;
  col.values[i] = read;
  if (typeof read === 'string' && HIGH.test(read)) col.high[i] = 1;
  return read;
}

/**
 * The one comparator over row indexes: every key in turn, nulls last in
 * both directions, values of different types unordered, ties to the index.
 * The same order the stable sort per key gave, in one sort.
 */
function comparatorOf(columns: readonly Column[]): (a: number, b: number) => number {
  const count = columns.length;
  const from = (start: number, a: number, b: number): number => {
    for (let k = start; k < count; k++) {
      const col = columns[k]!;
      const x = valueAt(col, a);
      const y = valueAt(col, b);
      if (x === y) continue;
      if (x === null) return 1;
      if (y === null) return -1;
      const tx = typeof x;
      if (tx !== typeof y) continue;
      let ord: number;
      if (tx === 'string') {
        // `<` is code point order unless both strings reach U+D800.
        ord = col.high[a]! & col.high[b]! ? compareText(x as string, y as string) : (x as string) < (y as string) ? -1 : 1;
      } else if (tx === 'number' || tx === 'bigint') {
        if ((x as number) < (y as number)) ord = -1;
        else if ((x as number) > (y as number)) ord = 1;
        else continue;
      } else if (tx === 'boolean') {
        ord = x ? 1 : -1;
      } else {
        continue;
      }
      return ord * col.sign;
    }
    return a - b;
  };
  const first = columns[0];
  if (!first || first.lazy || !isUniform(first)) return (a, b) => from(0, a, b);
  // The first key holds one type that `<` orders exactly: its short form.
  const values = first.values as number[];
  const sign = first.sign;
  return (a, b) => {
    const x = values[a]!;
    const y = values[b]!;
    if (x === y) return from(1, a, b);
    if (x === null) return 1;
    if (y === null) return -1;
    if (x < y) return -sign;
    if (x > y) return sign;
    return from(1, a, b);
  };
}

/**
 * True when every value of a column, nulls aside, is of one type that `<`
 * and `>` order exactly as `compare` does: numbers, BigInts, or strings
 * below U+D800 throughout.
 */
function isUniform(col: Column): boolean {
  let kind: string | null = null;
  const { values, high } = col;
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (v === null) continue;
    const t = typeof v;
    if (t !== 'number' && t !== 'bigint' && t !== 'string') return false;
    if (t === 'string' && high[i]) return false;
    if (kind === null) kind = t;
    else if (kind !== t) return false;
  }
  return true;
}

/** True when rows `from - 1` to `to - 1` are already in order. */
function inOrder(n: number, cmp: (a: number, b: number) => number, from = 1, to = n): boolean {
  for (let i = Math.max(from, 1); i < to; i++) if (cmp(i - 1, i) > 0) return false;
  return true;
}

/** Rows in the plan's order; rows already in it are not sorted at all. */
function sortRows(rows: readonly Row[], keys: readonly SortPlan[]): Row[] {
  if (keys.length === 0 || rows.length < 2) return [...rows];
  const cmp = comparatorOf(columnsOf(rows, keys));
  if (inOrder(rows.length, cmp)) return [...rows];
  const idx = Array.from({ length: rows.length }, (_, i) => i);
  idx.sort(cmp);
  return idx.map((i) => rows[i]!);
}

/** Two rows in the plan's order, parsing as it goes: for a few comparisons, not a sort. */
function compareRows(a: Row, b: Row, keys: readonly SortPlan[]): number {
  for (const key of keys) {
    const x = (a as Record<string, unknown>)[key.field] ?? null;
    const y = (b as Record<string, unknown>)[key.field] ?? null;
    if (x === null && y === null) continue;
    if (x === null) return 1;
    if (y === null) return -1;
    const ord = compare(x, y, key.exact);
    if (ord === null || ord === 0) continue;
    return ord * key.sign;
  }
  return 0;
}

function pickRow(row: Row, wanted: readonly string[]): Row {
  const picked: Record<string, unknown> = {};
  for (const field of wanted) picked[field] = (row as Record<string, unknown>)[field] ?? null;
  return picked as Row;
}

/**
 * Apply a validated query to rows. Pure, so it can answer a keystroke.
 *
 * `order` is the read's own order as the descriptor states it (`OpSpec.order`,
 * 27o). Without a `sort` in the query the rows come back in that order when
 * it is stated, so the device copy answers in the order the server would;
 * when it is not stated, or no order is given, they come back as held. A
 * `sort` in the query replaces the stated keys and ties fall to the id, which
 * the stated order names (its last key) and which is `id` otherwise.
 *
 * `decimals` names the fields holding exact decimals, `money` and `decimal`
 * (`OpSpec.refine.decimals`, 27ak). They filter and sort as numbers.
 */
export function refineRows(
  rows: readonly Row[],
  query: LocalQuery,
  order?: ReadOrder | null,
  decimals: readonly string[] = [],
): Row[] {
  const plan = planOf(query, order, decimals);
  const kept = plan.conditions.length > 0 ? rows.filter((r) => matches(r, plan.conditions, plan.exact)) : rows;
  const out = sortRows(kept, plan.keys);
  if (!plan.pick) return out;
  const wanted = plan.pick;
  return out.map((row) => pickRow(row, wanted));
}

// ── the same answer, in slices (H2) ───────────────────────────────────

/** Rows per slice of {@link refineRowsAsync} when the caller names none. */
export const DEFAULT_REFINE_CHUNK = 5_000;

export interface RefineAsyncOptions {
  /** Abandons the refinement at its next slice, rejecting with the signal's reason. */
  readonly signal?: AbortSignal;
  /** Rows per slice: each slice is one task. {@link DEFAULT_REFINE_CHUNK} by default. */
  readonly chunk?: number;
  /** How a slice gives the page back. `scheduler.yield()` where there is one, else a zero timer. */
  readonly yieldTo?: () => Promise<void>;
}

function defaultYield(): Promise<void> {
  const scheduler = (globalThis as { scheduler?: { yield?: () => Promise<void> } }).scheduler;
  if (scheduler?.yield) return scheduler.yield();
  return new Promise((resolve) => setTimeout(resolve, 0));
}

/**
 * {@link refineRows}, in slices of `chunk` rows with the page given back
 * between them, so no step holds the page for long however many rows there
 * are. The answer is the same, row for row. Up to `chunk` rows it is one
 * synchronous call.
 *
 * The filter, the parse of each sort key, the order check and the pick run
 * a slice at a time. The sort sorts slices, then merges them a slice of
 * output at a time.
 */
export async function refineRowsAsync(
  rows: readonly Row[],
  query: LocalQuery,
  order?: ReadOrder | null,
  decimals: readonly string[] = [],
  options: RefineAsyncOptions = {},
): Promise<Row[]> {
  const chunk = Math.max(1, Math.floor(options.chunk ?? DEFAULT_REFINE_CHUNK));
  const signal = options.signal;
  signal?.throwIfAborted();
  if (rows.length <= chunk) return refineRows(rows, query, order, decimals);
  const pause = options.yieldTo ?? defaultYield;
  const turn = async (): Promise<void> => {
    await pause();
    signal?.throwIfAborted();
  };
  const plan = planOf(query, order, decimals);

  let kept: readonly Row[] = rows;
  if (plan.conditions.length > 0) {
    const out: Row[] = [];
    for (let at = 0; at < rows.length; at += chunk) {
      const end = Math.min(rows.length, at + chunk);
      for (let i = at; i < end; i++) if (matches(rows[i]!, plan.conditions, plan.exact)) out.push(rows[i]!);
      await turn();
    }
    kept = out;
  }

  let sorted: readonly Row[] = kept;
  const n = kept.length;
  if (plan.keys.length > 0 && n > 1) {
    const columns: Column[] = [];
    for (const k of plan.keys) {
      const values = new Array<unknown>(n);
      const high = new Uint8Array(k.exact ? 0 : n);
      if (k.exact) {
        const scan = newScan(n);
        for (let at = 0; at < n; at += chunk) {
          scanDecimals(kept, k.field, values, scan, at, Math.min(n, at + chunk));
          await turn();
        }
        for (let at = 0; at < n; at += chunk) {
          liftDecimals(values, scan, at, Math.min(n, at + chunk));
          await turn();
        }
      } else {
        for (let at = 0; at < n; at += chunk) {
          plainRange(kept, k.field, values, high, at, Math.min(n, at + chunk));
          await turn();
        }
      }
      columns.push({ values, high, sign: k.sign });
    }
    const cmp = comparatorOf(columns);
    let ordered = true;
    for (let at = 1; at < n && ordered; at += chunk) {
      ordered = inOrder(n, cmp, at, Math.min(n, at + chunk));
      await turn();
    }
    if (!ordered) {
      let idx = Array.from({ length: n }, (_, i) => i);
      // Sorted runs of one slice each, then merged pairwise.
      for (let at = 0; at < n; at += chunk) {
        const run = idx.slice(at, at + chunk).sort(cmp);
        for (let i = 0; i < run.length; i++) idx[at + i] = run[i]!;
        await turn();
      }
      let into = new Array<number>(n);
      for (let width = chunk; width < n; width *= 2) {
        let done = 0;
        for (let lo = 0; lo < n; lo += 2 * width) {
          const mid = Math.min(n, lo + width);
          const hi = Math.min(n, lo + 2 * width);
          let i = lo;
          let j = mid;
          let k = lo;
          while (i < mid || j < hi) {
            if (j >= hi || (i < mid && cmp(idx[i]!, idx[j]!) <= 0)) into[k++] = idx[i++]!;
            else into[k++] = idx[j++]!;
            if (++done >= chunk) {
              done = 0;
              await turn();
            }
          }
        }
        [idx, into] = [into, idx];
      }
      const out = new Array<Row>(n);
      for (let at = 0; at < n; at += chunk) {
        const end = Math.min(n, at + chunk);
        for (let i = at; i < end; i++) out[i] = kept[idx[i]!]!;
        await turn();
      }
      sorted = out;
    }
  }

  if (!plan.pick) return sorted === rows ? [...rows] : (sorted as Row[]);
  const wanted = plan.pick;
  const out = new Array<Row>(sorted.length);
  for (let at = 0; at < sorted.length; at += chunk) {
    const end = Math.min(sorted.length, at + chunk);
    for (let i = at; i < end; i++) out[i] = pickRow(sorted[i]!, wanted);
    await turn();
  }
  return out;
}

// ── keeping an answer current: a delta merged into it ─────────────────

/**
 * Bring an earlier answer up to date with what changed, in one pass: the
 * `removed` rows leave it, and the `added` rows that pass the filter are
 * sorted and merged in. The answer is what {@link refineRows} gives over the
 * new rows, without a `pick`: the caller picks, so it can keep the picked
 * object of each row that did not change.
 *
 * `sorted` is an earlier answer of this same query, order and decimals, with
 * no pick. A changed row is a removal of its old object and an addition of
 * its new one. The cost is the rows kept, plus the added rows' sort, plus a
 * binary search per added row. Rows that did not change keep their objects,
 * and when nothing changed the answer is `sorted` itself.
 *
 * With no sort and no stated order, rows are kept by `id`, which is how the
 * device holds them.
 */
export function refineMerge(
  sorted: readonly Row[],
  added: readonly Row[],
  removed: ReadonlySet<Row>,
  query: LocalQuery,
  order?: ReadOrder | null,
  decimals: readonly string[] = [],
): readonly Row[] {
  const plan = planOf({ ...(query.where ? { where: query.where } : {}), ...(query.sort ? { sort: query.sort } : {}) }, order, decimals);
  const keys: readonly SortPlan[] =
    plan.keys.length > 0 ? plan.keys : [{ field: order?.keys.at(-1)?.path ?? 'id', sign: 1, exact: false }];
  const kept = removed.size > 0 ? sorted.filter((r) => !removed.has(r)) : sorted;
  const fresh = sortRows(
    plan.conditions.length > 0 ? added.filter((r) => matches(r, plan.conditions, plan.exact)) : added,
    keys,
  );
  if (fresh.length === 0) return kept.length === sorted.length ? sorted : kept;
  const out: Row[] = [];
  let from = 0;
  for (const row of fresh) {
    // The first kept row that sorts after this one, searched from the last place.
    let lo = from;
    let hi = kept.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (compareRows(kept[mid]!, row, keys) <= 0) lo = mid + 1;
      else hi = mid;
    }
    for (let i = from; i < lo; i++) out.push(kept[i]!);
    out.push(row);
    from = lo;
  }
  for (let i = from; i < kept.length; i++) out.push(kept[i]!);
  return out;
}

/**
 * Answer `query` from the rows this op holds on the device.
 *
 * `holder` is the sync manager's handle when one op is synced at more than one
 * set of parameters; it defaults to the op's name, which is what {@link syncOp}
 * holds rows under.
 */
export async function refineOp(
  replica: LocalReplica,
  spec: OpSpec,
  query: LocalQuery = {},
  holder?: string,
): Promise<Row[]> {
  checkQuery(spec, query);
  if (!spec.sync) {
    refuse(spec.op, `${spec.op} is not synced, so the device holds no rows of its own to refine`);
  }
  const rows = (await replica.held(holder ?? spec.op)).map((r) => r.row);
  return refineRows(rows, query, spec.order, spec.refine.decimals ?? []);
}
