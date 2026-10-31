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

import { desc, f, op, src, synth } from '../../src/index.js';

const allLeases = op('allLeases', {}, (b) =>
  b
    .source(src('leases').pick('start', 'end', 'status', 'unit.name'))
    .sort(desc(f.start)))
  .unbounded()
  .unguarded();

export const ordo: string = synth(
  allLeases,
);
