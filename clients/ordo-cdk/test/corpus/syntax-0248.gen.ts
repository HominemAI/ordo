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

import { asc, f, lit, op, src, synth } from '../../src/index.js';

const openOrders = op('openOrders', {}, (b) =>
  b
    .source(src('workOrders').args(f.status.eq(lit('open'))).pick('opened', 'priority', 'descr', 'location.name', 'location.property.name'))
    .sort(asc(f.priority), asc(f.opened)))
  .unbounded();

export const ordo: string = synth(
  openOrders,
);
