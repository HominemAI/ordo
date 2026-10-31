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

import { agg, as_, f, fn, lit, op, src, synth, t } from '../../src/index.js';

const orderCostRollup = op('orderCostRollup', { order: t.id() }, (b, p) =>
  b
    .source(src('workItems').args(f.workOrder.eq(p.order)).pick('descr', as_('cost', fn.coalesce(f.cost, lit(0)))))
    .reduce({ items: agg.count(), est: agg.sum(f.cost) }));

export const ordo: string = synth(
  orderCostRollup,
);
