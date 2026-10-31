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

import { dateLit, f, named, op, src, synth, t } from '../../src/index.js';

const bigSpenders = op('bigSpenders', { min: t.int() }, (b, p) =>
  b
    .source(src('vendorSpend').args(named('from', dateLit('2026-01-01')), f.billed.gt(p.min)).pick('name', 'billed'))
    .take(10));

export const ordo: string = synth(
  bigSpenders,
);
