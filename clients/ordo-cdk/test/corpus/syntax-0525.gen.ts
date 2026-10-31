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

import { bare, f, lit, op, synth, t } from '../../src/index.js';

const removeItem = op('removeItem', { item: t.id() }, (b, p) =>
  [
    b.del('workItems', f.id.eq(p.item)),
  ]);

const pruneTelemetry = op('pruneTelemetry', { before: t.date() }, (b, p) =>
  [
    b.del('events', f.at.le(p.before)),
  ]);

const removeOrder = op('removeOrder', { id: t.id() }, (b, p) =>
  [
    b.del('workOrders', f.id.eq(p.id)),
  ]);

const removeBuilding = op('removeBuilding', { bldg: t.id() }, (b, p) =>
  [
    b.del('locations', f.id.eq(p.bldg)).subtree(),
  ]);

const purgeEvents = op('purgeEvents', {}, (b) =>
  [
    b.del('events', bare(lit(true))),
  ]);

export const ordo: string = synth(
  removeItem,
  pruneTelemetry,
  removeOrder,
  removeBuilding,
  purgeEvents,
);
