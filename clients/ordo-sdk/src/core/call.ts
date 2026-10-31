/**
 * The terminal-verb split (`tooling-and-embedding-plan.md` §B3), as types.
 *
 * **Static half.** Three classes; the generated functions choose one per op
 * from `call.terminal` and `result.single_row`, so misuse is a compile error
 * in the caller's own language and every mutation in an app is
 * `grep '.commit('`.
 *
 * - {@link ReadCall}: `.fetch()` and `.pages()`.
 * - {@link SingleReadCall}: `.fetch()` only. A single-row op has no second
 *   page and now says so: `ordo.interface/2` narrowed `call.terminal` and
 *   `call.paged` on exactly these, and this is what that narrowing buys.
 * - {@link WriteCall}: `.commit()`, and **no read verbs**.
 *
 * Both read calls are lazy builders (§B2): `.where(...)` and `.pick(...)`
 * narrow them and the terminal verb is unchanged.
 *
 * ```ts
 * const page = await vendorSpend(client, { from: '2026-01-01' })
 *   .where((v) => [v.billed.gt('4000'), v.status.in(['open', 'late'])])
 *   .pick('name', 'billed')
 *   .fetch();
 * // page.rows: readonly Pick<VendorSpendRow, 'name' | 'billed'>[]
 * ```
 *
 * `.live()` exists on none of them. See the README: the descriptor carries no
 * log position on the read path, so a subscription cannot be ordered against
 * its own first frame, and a `.live()` that reconnected and refetched would
 * ship the exact tear `api-engine-design.md` §6 designs against.
 */

import { checkParamsOnce } from './client.js';
import type { OpResponse, OrdoClient, Params, RequestOptions, Row } from './client.js';
import { checkRefine, conditionsOf, fieldsProxy, type AnyFields, type Conditions, type Refine } from './refine.js';
import type { OpSpec } from './spec.js';
import type { RefusalClass } from './refusal.js';

/** What a read verb hands back, with the envelope's own metadata intact. */
export interface Page<R> extends OpResponse<R> {
  readonly hasMore: boolean;
}

/** A terminal verb's options. A read's refinement comes from the builder. */
export type CallOptions = Omit<RequestOptions, 'refine'>;

function page<R>(res: OpResponse<R>): Page<R> {
  return { ...res, hasMore: res.next !== undefined && res.next !== null && res.next !== '' };
}

abstract class Call {
  protected readonly client: OrdoClient;
  protected readonly spec: OpSpec;
  protected readonly params: Params;

  constructor(client: OrdoClient, spec: OpSpec, params: Params) {
    // Refused at the CALL SITE, not at the terminal verb.
    //
    // `cheapActive(client, { cap: 4000 })` is where the mistake is, and a
    // refusal that waited for `.fetch()` would point at the wrong line and
    // arrive as a rejected promise, which a caller who has not awaited yet
    // sees as an unhandled rejection somewhere else entirely. The door runs
    // the same check on the same values; this one just runs first.
    this.params = checkParamsOnce(spec, params);
    this.client = client;
    this.spec = spec;
  }

  /** The op's identity, for logs and bug reports. */
  describe(): { op: string; version: string; method: string } {
    return { op: this.spec.op, version: this.spec.version, method: this.spec.method };
  }
}

/**
 * A read, which a refinement may narrow. Each builder method returns a new
 * call and leaves this one as it was, so a `.pages()` loop holds one
 * refinement for its whole life.
 */
abstract class RefinableCall extends Call {
  /** The refinement in its wire form, checked. Empty when nothing narrows. */
  readonly refinement: Refine;

  constructor(client: OrdoClient, spec: OpSpec, params: Params, refinement: Refine = {}) {
    super(client, spec, params);
    this.refinement = refinement;
  }

  /** `where` appended and checked, as the door would check it. */
  protected narrowed(build: (fields: never) => Conditions): Refine {
    const added = conditionsOf(build(fieldsProxy<never>()));
    return checkRefine(this.spec, { ...this.refinement, where: [...(this.refinement.where ?? []), ...added] }, 'door');
  }

  /** `pick` replaced and checked. */
  protected picked(keys: readonly string[]): Refine {
    return checkRefine(this.spec, { ...this.refinement, pick: keys }, 'door');
  }

  protected options(options: CallOptions): RequestOptions {
    const narrows = (this.refinement.where?.length ?? 0) > 0 || this.refinement.pick !== undefined;
    return narrows ? { ...options, refine: this.refinement } : options;
  }
}

/**
 * A paged read: `.fetch()` for the first page, `.pages()` for all of them.
 *
 * - `R` is the op's whole row, which `.pick` chooses keys from.
 * - `C` is the op's declared refusal set.
 * - `F` is the op's field proxy, which `.where` hands its callback.
 * - `Out` is the row the verbs return: `R`, or `Pick<R, K>` after `.pick`.
 */
export class ReadCall<
  R = Row,
  C extends RefusalClass = RefusalClass,
  F = AnyFields,
  Out = R,
> extends RefinableCall {
  /** The op's declared refusal set, as a phantom. Never read at run time. */
  declare readonly refusal: C;

  /**
   * Narrow the rows. The callback gets one proxy per refinable output and
   * returns one condition or several. Conditions combine as AND, and a
   * second `.where` appends to the first.
   *
   * Checked here, at the call site: a field the op does not return, an
   * operator a sealed field does not take, or a value of the wrong type is
   * an `invalid_refinement` refusal thrown from this line.
   */
  where(build: (fields: F) => Conditions): ReadCall<R, C, F, Out> {
    return new ReadCall(this.client, this.spec, this.params, this.narrowed(build as (f: never) => Conditions));
  }

  /**
   * Keep only these fields. The row type narrows to `Pick<R, K>`. A later
   * `.pick` replaces this one. An empty pick does not compile.
   */
  pick<K extends keyof R & string>(...keys: [K, ...K[]]): ReadCall<R, C, F, Pick<R, K>> {
    return new ReadCall(this.client, this.spec, this.params, this.picked(keys));
  }

  /** One shot. The first page, and nothing about the rest. */
  async fetch(options: CallOptions = {}): Promise<Page<Out>> {
    return page(await this.client.request<Out>(this.spec, this.params, this.options(options)));
  }

  /**
   * Page to exhaustion.
   *
   * Stops **only** when `next` is absent, never on an empty page. Every page
   * request carries the same refinement and the cursor from the page before,
   * so the loop is correct whether the door fills pages under a refinement or
   * returns them short.
   */
  async *pages(options: CallOptions = {}): AsyncGenerator<Page<Out>, void, void> {
    let cursor = options.page;
    for (;;) {
      const res = await this.client.request<Out>(this.spec, this.params, this.options({ ...options, page: cursor }));
      const p = page(res);
      yield p;
      if (!p.hasMore) return;
      cursor = res.next;
    }
  }

  /** Every row, in order. Convenience over {@link pages}, not a third verb. */
  async rows(options: CallOptions = {}): Promise<Out[]> {
    const out: Out[] = [];
    for await (const p of this.pages(options)) {
      // A loop, not `push(...rows)`: a huge page overflows the argument limit.
      for (const row of p.rows) out.push(row);
    }
    return out;
  }
}

/**
 * A read the executor answers whole: `count`-shaped ops and addressed `get`s.
 *
 * No `.pages()`, because there is no second page, and the descriptor now
 * says so rather than leaving `result.single_row` to contradict
 * `call.terminal`. `.where` and `.pick` work as on {@link ReadCall}: a
 * condition on a single row answers that row or nothing.
 */
export class SingleReadCall<
  R = Row,
  C extends RefusalClass = RefusalClass,
  F = AnyFields,
  Out = R,
> extends RefinableCall {
  /** The op's declared refusal set, as a phantom. Never read at run time. */
  declare readonly refusal: C;

  /** As {@link ReadCall.where}. */
  where(build: (fields: F) => Conditions): SingleReadCall<R, C, F, Out> {
    return new SingleReadCall(this.client, this.spec, this.params, this.narrowed(build as (f: never) => Conditions));
  }

  /** As {@link ReadCall.pick}. */
  pick<K extends keyof R & string>(...keys: [K, ...K[]]): SingleReadCall<R, C, F, Pick<R, K>> {
    return new SingleReadCall(this.client, this.spec, this.params, this.picked(keys));
  }

  async fetch(options: CallOptions = {}): Promise<Page<Out>> {
    return page(await this.client.request<Out>(this.spec, this.params, this.options(options)));
  }

  /** The one row, or `null`. `rows` is always an array on the wire (§3). */
  async row(options: CallOptions = {}): Promise<Out | null> {
    const res = await this.fetch(options);
    return res.rows[0] ?? null;
  }
}

/** A write or a delete. One verb, and no read verbs. */
export class WriteCall<R = Row, C extends RefusalClass = RefusalClass> extends Call {
  /** The op's declared refusal set, as a phantom. Never read at run time. */
  declare readonly refusal: C;

  /**
   * `.commit()`.
   *
   * An idempotency key is minted per call unless one is supplied. Supply one
   * to make a retry *replay* rather than re-execute. The key is honoured on
   * POST and DELETE only, and the door ignores it on reads; sending one on a
   * read would still be a lie about what the call is, so the client does not.
   */
  async commit(options: CallOptions = {}): Promise<OpResponse<R>> {
    return this.client.request<R>(this.spec, this.params, options);
  }
}
