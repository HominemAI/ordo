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

import { and, daily, durationLit, every, f, lit, routine, synth, t, when } from '../../src/index.js';

const staleAlerts = routine('staleAlerts', { in: 'estate', trigger: every('15m'), params: { today: t.date() } }, (b, p) => [
    b.set('alerts', { status: lit('dismissed') }).where(and(f.status.eq(lit('active')), f.raised.lt(p.today.sub(durationLit(30, 'd'))))),
  ]);

const dailyResolve = routine('dailyResolve', { in: 'estate', trigger: daily('02:00Z') }, (b) => [
    b.del('alerts', f.status.eq(lit('dismissed'))),
  ]);

const motionSeen = routine('motionSeen', { in: 'estate', trigger: when('estate.events.created', { debounce: '30s' }) }, (b) => [
    b.set('events', { kind: lit('motion-seen') }).where(f.kind.eq(lit('motion'))),
  ]);

export const ordo: string = synth(
  staleAlerts,
  dailyResolve,
  motionSeen,
);
