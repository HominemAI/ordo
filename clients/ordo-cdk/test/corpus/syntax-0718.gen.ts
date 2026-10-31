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

import { entity, g, lit, synth, t } from '../../src/index.js';

const leases = entity('leases', {
  unit: t.ref('units'),
  start: t.date(),
  end: t.date().optional(),
  rent: t.money().sealed(),
  status: t.text().default(lit('active')),
})
  .accessRead(g.walk(g.any(g.all(g.step('unit.property.propertiesAccess', g.any(g.all(g.eq('member'))))))))
  .accessWrite(g.walk(g.any(g.all(g.step('unit.property.propertiesAccess', g.any(g.all(g.eq('member'), g.eq('level', lit('write'))), g.all(g.eq('member'), g.eq('level', lit('admin')))))))));

export const ordo: string = synth(
  leases,
);
