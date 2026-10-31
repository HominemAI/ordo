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

import { agg, desc, f, lit, op, src, synth, t } from '../../src/index.js';

const vendorSpend = op('vendorSpend', { from: t.date() }, (b, p) =>
  b
    .source(src('vendors').pick('name'), src('invoices').args(f.issued.ge(p.from)).pick('total'))
    .group([f.vendors.id, f.vendors.name], { n: agg.count(), billed: agg.sum(f.total), large: agg.count(f.total.gt(lit(100000))) })
    .where(f.billed.gt(lit(0)))
    .sort(desc(f.billed)))
  .unbounded();

export const ordo: string = synth(
  vendorSpend,
);
