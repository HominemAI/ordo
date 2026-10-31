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

import { and, asc, f, lit, op, or, src, synth, t } from '../../src/index.js';

const searchVendors = op('searchVendors', { q: t.text() }, (b, p) =>
  b
    .source(src('vendors').args(and(f.active.eq(lit(true)), or(f.name.contains(p.q), f.name.starts(p.q)))).pick('name'))
    .sort(asc(f.name)));

export const ordo: string = synth(
  searchVendors,
);
