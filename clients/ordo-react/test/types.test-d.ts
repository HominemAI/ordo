/**
 * Compile-time assertions. Not run by vitest — `tsc --noEmit` is the test,
 * and a `@ts-expect-error` that stops being an error fails the build.
 *
 * What is being proved: a consumer who imports their own `interface.json`
 * with `as const` gets each op's *declared* refusal set as a literal union,
 * not the whole closed inventory — so a `switch` over an op's refusals is
 * exhaustively checked against that op.
 */

import type { RefusalClass, RefusalOf } from '../src/refusal.js';
import type { InterfaceDocument, OpDescriptor, ParamDescriptor } from '../src/descriptor.js';
import { useOpQuery } from '../src/hooks/useOpQuery.js';
import { useOpPages } from '../src/hooks/useOpPages.js';
import { opRef, type OpenField, type SealedField } from '../src/refine.js';

const pinned = {
  ops: [
    { op: 'std.tasks.update', refusals: ['cas_miss', 'not_found', 'access_guard'] },
    { op: 'std.tasks.list', refusals: ['access_denied', 'invalid_cursor'] },
  ],
} as const;

type UpdateRefusal = RefusalOf<typeof pinned, 'std.tasks.update'>;
type ListRefusal = RefusalOf<typeof pinned, 'std.tasks.list'>;

export const declared: UpdateRefusal = 'cas_miss';
export const alsoDeclared: UpdateRefusal = 'access_guard';

// @ts-expect-error `duplicate` is a real refusal class, but not one this op declares.
export const notDeclaredByThisOp: UpdateRefusal = 'duplicate';

// @ts-expect-error not a refusal class at all.
export const notAClass: UpdateRefusal = 'made_up';

// @ts-expect-error the two ops have disjoint sets; neither absorbs the other.
export const wrongOp: ListRefusal = 'cas_miss';

/** Exhaustive handling of one op's set compiles with no default branch. */
export function handle(r: UpdateRefusal): string {
  switch (r) {
    case 'cas_miss':
      return 'refetch and remerge';
    case 'not_found':
      return 'gone';
    case 'access_guard':
      return 'not yours';
  }
}

/**
 * Without `as const` the JSON widens to `string[]`, and the helper degrades
 * to the closed inventory rather than to `string`. Still sound, just less
 * sharp — which is the honest outcome and worth pinning so a future change
 * cannot silently make it `any`.
 */
declare const widened: { ops: { op: string; refusals: string[] }[] };
export const widenedRefusal: RefusalOf<typeof widened, string> = 'cas_miss' satisfies RefusalClass;

// @ts-expect-error even widened, it is not `string`.
export const stillClosed: RefusalOf<typeof widened, string> = 'not_a_refusal_class';

/** The document type is structural: an emitted descriptor satisfies it. */
declare const emitted: InterfaceDocument;
export const anOp: OpDescriptor | undefined = emitted.ops[0];
export const aParam: ParamDescriptor | undefined = anOp?.params[0];

// @ts-expect-error the descriptor is deeply readonly; a runtime must not edit it.
emitted.ops[0]!.params = [];

// ── typed refinements on the hooks ─────────────────────────────────────

interface SpendRow {
  readonly name: string;
  readonly billed: string;
  readonly memo: string;
}
interface SpendFields {
  readonly name: OpenField<'name', string>;
  readonly billed: OpenField<'billed', string>;
  readonly memo: SealedField<'memo', string>;
}
const spend = opRef<SpendRow, SpendFields>('vendorSpend');

/** Never called: `tsc` checks the hook calls without React rendering them. */
export function typedHooks(): void {
  const q = useOpQuery(spend, { refine: (v) => [v.billed.gt('4000'), v.memo.eq('x')], pick: ['name', 'billed'] });
  const narrowed: readonly Pick<SpendRow, 'name' | 'billed'>[] = q.rows;
  void narrowed;
  // @ts-expect-error a picked-away field is gone from the row type.
  void q.rows[0]?.memo;

  const whole = useOpQuery(spend, {});
  const all: readonly SpendRow[] = whole.rows;
  void all;

  // @ts-expect-error `margin` is not a field of the op.
  useOpQuery(spend, { refine: (v) => v.margin.eq('1') });
  // @ts-expect-error money is a decimal string.
  useOpQuery(spend, { refine: (v) => v.billed.gt(4000) });
  // @ts-expect-error a sealed field takes no ordering.
  useOpQuery(spend, { refine: (v) => v.memo.gt('x') });
  // @ts-expect-error `margin` is not an output to pick.
  useOpQuery(spend, { pick: ['margin'] });

  const pages = useOpPages(spend, { refine: (v) => v.name.isNull(), pick: ['name'] });
  const paged: readonly Pick<SpendRow, 'name'>[] = pages.rows;
  void paged;

  // A plain name and the wire shape still work, untyped.
  useOpQuery('std.tasks.list', { refine: { where: [{ field: 'status', op: 'eq', value: 'open' }] } });
}
