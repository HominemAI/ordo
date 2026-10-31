/**
 * Keyset pagination.
 *
 * The whole of the difficulty is one sentence in `api-engine-design.md` §7,
 * and it is the sentence every naive client gets wrong:
 *
 * > **Pages may come back short, or empty, while `next` is still present.**
 * > An empty page is not the end of the results; **absence of `next` is the
 * > only terminator**.
 *
 * So `hasMore` here is `next !== undefined`, never `rows.length > 0`, and the
 * empty state is a fact about the *whole iteration* rather than about the
 * first page — `ui-gen.md` §2 says the same thing from the runtime side.
 *
 * The second thing this module owns is the cursor's own staleness. A token
 * minted under a different op version answers `version_skew` (409), and the
 * repair is to drop the cursor and restart from page one — the token is not
 * migrated. That is **not** the same as a `cas_miss`: no data conflicted, the
 * listing is simply being read against a program that moved. Conflating the
 * two is how a refetch-and-remerge dialog ends up in front of a user who was
 * scrolling a table.
 */

import type { DescriptorCallOptions, OpResponse, Params, Row } from './client.js';
import type { DescriptorClient } from './client.js';
import { OrdoRefusal, dispositionOf, isRefusalClass, type Refusal } from './refusal.js';

export interface PageState<R = Row> {
  readonly rows: readonly R[];
  readonly cursor: string | null;
  /** `next` was present on the last page. The only legal terminator check. */
  readonly hasMore: boolean;
  /** The whole iteration so far produced nothing. */
  readonly empty: boolean;
  readonly pagesFetched: number;
  readonly stale: boolean;
  /**
   * Counts every change to this listing. `more()` appends to one array
   * rather than copying it each page, so `rows` keeps its reference while it
   * grows. Anything keyed on the rows (a memo, an effect) keys on `version`.
   */
  readonly version: number;
}

/**
 * The state that last grew each array. Only that state may append to its
 * array in place. An older state, or a state built by hand, copies instead.
 */
const tips = new WeakMap<readonly unknown[], unknown>();

/** How many rows each state held when it was made; its array may have grown since. */
const lengths = new WeakMap<object, number>();

function startState<R>(make: (rows: R[]) => PageState<R>): PageState<R> {
  const rows: R[] = [];
  const state = make(rows);
  tips.set(rows, state);
  lengths.set(state, 0);
  return state;
}

export const emptyPageState = <R = Row>(): PageState<R> =>
  startState<R>((rows) => ({
    rows,
    cursor: null,
    hasMore: true,
    empty: true,
    pagesFetched: 0,
    stale: false,
    version: 0,
  }));

/**
 * Fold one response into the accumulated listing. The latest state appends
 * to its array in place, so a scroll of N pages copies each row once. The
 * returned state has a new `version`. A state that is not the latest copies.
 */
export function advance<R>(state: PageState<R>, res: OpResponse<R>): PageState<R> {
  const owned = tips.get(state.rows) === state;
  const rows = owned ? (state.rows as R[]) : state.rows.slice(0, lengths.get(state) ?? state.rows.length);
  for (const r of res.rows) rows.push(r);
  const next: PageState<R> = {
    rows,
    cursor: res.next ?? null,
    hasMore: res.next !== undefined && res.next !== null && res.next !== '',
    empty: rows.length === 0,
    pagesFetched: state.pagesFetched + 1,
    stale: state.stale || res.stale,
    version: state.version + 1,
  };
  tips.set(rows, next);
  lengths.set(next, rows.length);
  return next;
}

export type PageStep<R = Row> =
  | { readonly status: 'page'; readonly state: PageState<R> }
  /** The cursor is stale or forged. Drop it and start over; the rows were fine. */
  | { readonly status: 'restart'; readonly refusal: Refusal; readonly state: PageState<R> }
  | { readonly status: 'refused'; readonly refusal: Refusal; readonly state: PageState<R> };

/**
 * Fetch the next page onto `state`.
 *
 * Refuses to advance past the end: once `hasMore` is false, calling again is
 * a caller bug and re-sending `null` as a cursor would silently restart the
 * listing from the top, appending every row a second time.
 */
export async function nextPage<R = Row>(
  client: DescriptorClient,
  op: string,
  state: PageState<R>,
  options: Omit<DescriptorCallOptions, 'page'> = {},
): Promise<PageStep<R>> {
  try {
    const call: DescriptorCallOptions = { ...options };
    if (state.cursor !== null) (call as { page?: string }).page = state.cursor;
    const res = await client.fetchPage<R>(op, call);
    return { status: 'page', state: advance(state, res) };
  } catch (e) {
    if (!(e instanceof OrdoRefusal)) throw e;
    const refusal = e.refusal as Refusal;
    const disposition = isRefusalClass(refusal.class) ? dispositionOf(refusal.class) : 'fault';
    if (disposition === 'restart') {
      return { status: 'restart', refusal, state: emptyPageState<R>() };
    }
    return { status: 'refused', refusal, state };
  }
}

/**
 * Page to exhaustion, accumulating. `maxPages` is a runaway guard, not a
 * pagination concept: a selective Tier-0 filter still walks the whole base
 * set (§7), so an infinite-scroll surface over a large entity can legitimately
 * ask for many pages and a bug can ask for unbounded ones.
 */
export async function collectAll<R = Row>(
  client: DescriptorClient,
  op: string,
  options: Omit<DescriptorCallOptions, 'page'> & CollectOptions<R> = {},
): Promise<PageStep<R>> {
  const { maxPages = DEFAULT_MAX_PAGES, publishEvery = DEFAULT_PUBLISH_EVERY, onProgress, ...call } = options;
  // One array, appended to, so a long walk copies each row once (M4). A
  // state handed out is a snapshot of it at that moment.
  const rows: R[] = [];
  let state = emptyPageState<R>();
  const snapshot = (): PageState<R> => ({ ...state, rows: rows.slice(), empty: rows.length === 0 });
  while (state.hasMore && state.pagesFetched < maxPages) {
    if (call.signal?.aborted) break;
    let res: OpResponse<R>;
    try {
      const page: DescriptorCallOptions = { ...call };
      if (state.cursor !== null) (page as { page?: string }).page = state.cursor;
      res = await client.fetchPage<R>(op, page);
    } catch (e) {
      // An aborted walk ends with what it has.
      if (call.signal?.aborted) break;
      if (!(e instanceof OrdoRefusal)) throw e;
      const refusal = e.refusal as Refusal;
      const disposition = isRefusalClass(refusal.class) ? dispositionOf(refusal.class) : 'fault';
      if (disposition === 'restart') return { status: 'restart', refusal, state: emptyPageState<R>() };
      return { status: 'refused', refusal, state: snapshot() };
    }
    for (const r of res.rows) rows.push(r);
    state = {
      rows: state.rows,
      cursor: res.next ?? null,
      hasMore: res.next !== undefined && res.next !== null && res.next !== '',
      empty: rows.length === 0,
      pagesFetched: state.pagesFetched + 1,
      stale: state.stale || res.stale,
      version: state.version + 1,
    };
    if (onProgress && state.hasMore && state.pagesFetched % Math.max(1, publishEvery) === 0) onProgress(snapshot());
  }
  return { status: 'page', state: snapshot() };
}

/**
 * Whether two answers hold the same rows, in the same order, field for
 * field (L4). A refetch that changed nothing keeps the array on screen, so
 * the list does not render again. Linear in the rows' size.
 */
export function sameRows(a: readonly unknown[], b: readonly unknown[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (!sameValue(a[i], b[i])) return false;
  return true;
}

function sameValue(x: unknown, y: unknown): boolean {
  if (x === y) return true;
  if (typeof x !== 'object' || typeof y !== 'object' || x === null || y === null) return false;
  if (Array.isArray(x) !== Array.isArray(y)) return false;
  if (Array.isArray(x)) return sameRows(x, y as unknown[]);
  const kx = Object.keys(x);
  const ky = Object.keys(y);
  if (kx.length !== ky.length) return false;
  for (const k of kx) {
    if (!Object.prototype.hasOwnProperty.call(y, k)) return false;
    if (!sameValue((x as Record<string, unknown>)[k], (y as Record<string, unknown>)[k])) return false;
  }
  return true;
}

/** {@link collectAll}'s runaway guard when the caller names none. */
export const DEFAULT_MAX_PAGES = 10_000;
/** How many pages {@link collectAll} reads between two progress reports, when the caller names none. */
export const DEFAULT_PUBLISH_EVERY = 10;

export interface CollectOptions<R> {
  /** A runaway guard, {@link DEFAULT_MAX_PAGES} by default. */
  readonly maxPages?: number;
  /** Called with the rows so far every this many pages; {@link DEFAULT_PUBLISH_EVERY} by default. */
  readonly publishEvery?: number;
  /** Rows so far, while the walk goes on, so a screen shows them before the end. */
  readonly onProgress?: (state: PageState<R>) => void;
}

export type { Params };
