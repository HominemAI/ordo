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

import { and, as_, each, f, lit, routine, src, synth, t, when } from '../../src/index.js';

const securityWatch = routine('securityWatch', { in: 'estate', trigger: when('estate.events.created'), params: { today: t.date() } }, (b, p) =>
  each(b
    .source(src('events').args(and(f.kind.eq(lit('motion')), f.device.armed.eq(lit(true)))).pick('id', 'at', as_('deviceId', f.device.id), 'device.location.name')), [
    b.set('alerts', { kind: lit('security'), key: lit('motion:').add(f.deviceId), severity: lit('critical'), status: lit('active'), raised: p.today, title: lit('Motion at ').add(f.name) }),
  ]))
  .unguarded();

export const ordo: string = synth(
  securityWatch,
);
