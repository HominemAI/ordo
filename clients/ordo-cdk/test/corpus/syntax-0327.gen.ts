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

import { agg, f, op, src, synth } from '../../src/index.js';

const overpaidInvoices = op('overpaidInvoices', {}, (b) =>
  b
    .source(src('invoices').pick('number', 'total'), src('payments').pick('amount'))
    .group([f.invoices.id, f.invoices.number, f.invoices.total], { paid: agg.sum(f.amount) })
    .where(f.paid.gt(f.total)))
  .unbounded();

export const ordo: string = synth(
  overpaidInvoices,
);
