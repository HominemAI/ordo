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

import { and, desc, f, lit, op, src, synth, t } from '../../src/index.js';

const ordersUnderBuilding = op('ordersUnderBuilding', { loc: t.id() }, (b, p) =>
  b
    .source(src('workOrders').args(and(f.location.under(p.loc), f.status.ne(lit('done')))).pick('opened', 'status', 'descr'))
    .sort(desc(f.opened)))
  .unbounded();

export const ordo: string = synth(
  ordersUnderBuilding,
);
