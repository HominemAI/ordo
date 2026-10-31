// GENERATED — do not edit; regenerate with `ordo gen ts`.
//
// One corpus program, rebuilt through the builder.
//
// Source of truth: the program's own parsed AST, translated by `ordo_gen::emit`
//
// This file is the GENERATED half of this package. The hand-written
// half is under `src/core/` and is versioned by hand; the boundary
// between them is documented in the package README. An edit here is
// lost the next time the generator runs.

import { agg, as_, asc, f, fn, lit, op, src, synth } from '../../src/index.js';

const vendorsBillingLag = op('vendorsBillingLag', {}, (b) =>
  b
    .let_('orderCount', (b1) => b1
    .source(src('vendors').pick('id'), src('workOrders').args(f.status.eq(lit('done'))).pick(as_('wo', f.id)))
    .group([f.vendors.id], { done: agg.count() }))
    .let_('invoiceCount', (b2) => b2
    .source(src('vendors').pick('id'), src('invoices').pick(as_('inv', f.id)))
    .group([f.vendors.id], { billed: agg.count() }))
    .source(src('vendors').pick('name'), src('orderCount').pick('done'), src('invoiceCount').pick('billed'))
    .where(fn.coalesce(f.billed, lit(0)).mul(lit(2)).lt(fn.coalesce(f.done, lit(0))))
    .sort(asc(f.name)));

export const ordo: string = synth(
  vendorsBillingLag,
);
