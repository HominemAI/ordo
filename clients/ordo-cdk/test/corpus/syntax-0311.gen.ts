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

import { agg, and, desc, f, lit, op, src, synth, t } from '../../src/index.js';

const anomalousReadings = op('anomalousReadings', { meter: t.id() }, (b, p) =>
  b
    .let_('baseline', (b1) => b1
    .source(src('readings').args(f.meter.eq(p.meter)))
    .reduce({ a: agg.avg(f.value) }))
    .source(src('readings').args(and(f.meter.eq(p.meter), f.value.gt(lit(3).mul(f.baseline.a)))).pick('at', 'value'))
    .sort(desc(f.at)));

export const ordo: string = synth(
  anomalousReadings,
);
