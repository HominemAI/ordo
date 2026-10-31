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

import { entity, synth, t } from '../../src/index.js';

const properties = entity('properties', {
  name: t.text(),
  address: t.text().sealed(),
  acquired: t.date(),
})
  .acl('read', 'write', 'admin');

export const ordo: string = synth(
  properties,
);
