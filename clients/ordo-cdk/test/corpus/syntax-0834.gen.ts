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

import { lit, op, synth, t } from '../../src/index.js';

const assignProperty = op('assignProperty', { property: t.id(), rev: t.int(), name: t.text(), manager: t.id() }, (b, p) =>
  [
    b.set('properties', { id: p.property, rev: p.rev, name: p.name }),
    b.set('propertiesAccess', { propertie: p.property, member: p.manager, level: lit('write') }),
  ]);

export const ordo: string = synth(
  assignProperty,
);
