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

import { asc, f, op, param, src, synth } from '../../src/index.js';

const myUnits = op('myUnits', {}, (b) =>
  b
    .source(src('leaseParties').args(f.person.eq(param('caller'))).pick('lease.unit.name', 'lease.unit.property.name'))
    .sort(asc(f.name)))
  .unbounded();

export const ordo: string = synth(
  myUnits,
);
