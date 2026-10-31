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

import { and, dateLit, f, lit, op, synth, t } from '../../src/index.js';

const createOrder = op('createOrder', { loc: t.id(), opened: t.date(), descr: t.text(), priority: t.int().max(lit(5)) }, (b, p) =>
  [
    b.set('workOrders', { location: p.loc, opened: p.opened, descr: p.descr, priority: p.priority }),
  ]);

const assignVendor = op('assignVendor', { order: t.id(), rev: t.int(), vendor: t.id() }, (b, p) =>
  [
    b.set('workOrders', { id: p.order, rev: p.rev, vendor: p.vendor, status: lit('scheduled') }),
  ]);

const ingestReading = op('ingestReading', { meter: t.id(), at: t.datetime(), value: t.int() }, (b, p) =>
  [
    b.set('readings', { meter: p.meter, at: p.at, value: p.value }),
  ]);

const closeStale = op('closeStale', {}, (b) =>
  [
    b.set('workOrders', { status: lit('void') }).where(and(f.status.eq(lit('open')), f.opened.le(dateLit('2025-12-31')))),
  ]);

const completeItem = op('completeItem', { item: t.id() }, (b, p) =>
  [
    b.set('workItems', { id: p.item, done: lit(true) }),
  ]);

const raiseAlert = op('raiseAlert', { kind: t.text(), key: t.text(), title: t.text(), sev: t.text(), today: t.date() }, (b, p) =>
  [
    b.set('alerts', { kind: p.kind, key: p.key, title: p.title, severity: p.sev, status: lit('active'), raised: p.today }),
  ]);

export const ordo: string = synth(
  createOrder,
  assignVendor,
  ingestReading,
  closeStale,
  completeItem,
  raiseAlert,
);
