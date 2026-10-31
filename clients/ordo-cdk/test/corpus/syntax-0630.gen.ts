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

import { agg, as_, each, f, lit, routine, src, synth, t, when } from '../../src/index.js';

const meterAnomaly = routine('meterAnomaly', { in: 'estate', trigger: when('estate.readings.created', { debounce: '5m' }), params: { today: t.date() } }, (b, p) =>
  each(b
    .let_('baseline', (b1) => b1
    .source(src('readings'))
    .reduce({ a: agg.avg(f.value) }))
    .source(src('readings').args(f.value.gt(lit(3).mul(f.baseline.a))).pick('id', 'value', as_('meterId', f.meter.id), 'meter.kind', 'meter.unit.name')), [
    b.set('alerts', { kind: lit('meter'), key: lit('meter:').add(f.meterId), severity: lit('warning'), status: lit('active'), raised: p.today, title: f.kind.add(lit(' anomaly in ')).add(f.name) }),
  ]))
  .unguarded();

export const ordo: string = synth(
  meterAnomaly,
);
