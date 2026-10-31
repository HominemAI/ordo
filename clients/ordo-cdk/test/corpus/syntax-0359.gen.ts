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

import { and, asc, dateLit, f, lit, op, src, synth } from '../../src/index.js';

const expiringSoon = op('expiringSoon', {}, (b) =>
  b
    .source(src('leases').args(and(f.status.eq(lit('active')), f.end.isNotNull(), f.end.le(dateLit('2026-12-31')))).pick('start', 'end', 'rent', 'unit.name', 'unit.property.name'))
    .sort(asc(f.end)));

export const ordo: string = synth(
  expiringSoon,
);
