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

import { and, daily, durationLit, each, f, lit, routine, src, synth, t } from '../../src/index.js';

const leaseExpiry = routine('leaseExpiry', { in: 'estate', trigger: daily('08:00Z'), params: { today: t.date() } }, (b, p) =>
  each(b
    .source(src('leases').args(and(f.status.eq(lit('active')), f.end.isNotNull(), f.end.le(p.today.add(durationLit(60, 'd'))))).pick('id', 'end', 'unit.name')), [
    b.set('alerts', { kind: lit('lease'), key: lit('leaseExpiry:').add(f.id), severity: lit('warning'), status: lit('active'), raised: p.today, title: f.unit.name.add(lit(' lease ends ')).add(f.end) }),
  ]));

export const ordo: string = synth(
  leaseExpiry,
);
