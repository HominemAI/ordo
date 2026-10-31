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

import { fn, lit, op, synth, t } from '../../src/index.js';

const setRent = op('setRent', { lease: t.id(), rev: t.int(), annual: t.money() }, (b, p) =>
  [
    b.set('leases', { id: p.lease, rev: p.rev, rent: fn.round(p.annual.div(lit(12)), lit(0)) }),
  ]);

export const ordo: string = synth(
  setRent,
);
