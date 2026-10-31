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

import { domain, entity, f, lit, synth, t } from '../../src/index.js';

const properties = entity('properties', {
  name: t.text(),
  address: t.text().sealed(),
  acquired: t.date(),
});

const locations = entity('locations', {
  property: t.ref('properties').owned(),
  parent: t.ref('locations').optional().owned().tree(),
  name: t.text(),
  kind: t.text(),
})
  .index('property', 'kind');

const units = entity('units', {
  property: t.ref('properties').owned(),
  location: t.ref('locations').optional(),
  name: t.text(),
})
  .unique('property', 'name');

const people = entity('people', {
  name: t.text().sealed(),
  email: t.text().optional().unique().sealed().indexed(),
  phone: t.text().optional().sealed(),
});

const leases = entity('leases', {
  unit: t.ref('units'),
  start: t.date(),
  end: t.date().optional(),
  rent: t.money().sealed(),
  status: t.text().default(lit('active')),
});

const leaseParties = entity('leaseParties', {
  lease: t.ref('leases').owned(),
  person: t.ref('people').owned(),
  role: t.text().default(lit('tenant')),
})
  .unique('lease', 'person');

const trades = entity('trades', {
  name: t.text().unique(),
});

const vendors = entity('vendors', {
  name: t.text(),
  email: t.text().optional().sealed(),
  active: t.bool().default(lit(true)),
});

const vendorTrades = entity('vendorTrades', {
  vendor: t.ref('vendors').owned(),
  trade: t.ref('trades').owned(),
})
  .unique('vendor', 'trade');

const workOrders = entity('workOrders', {
  location: t.ref('locations'),
  vendor: t.ref('vendors').optional(),
  opened: t.date(),
  status: t.text().default(lit('open')),
  priority: t.int().default(lit(3)),
  descr: t.text().sealed(),
});

const workItems = entity('workItems', {
  workOrder: t.ref('workOrders').owned(),
  descr: t.text().sealed(),
  cost: t.money().optional().sealed(),
  done: t.bool().default(lit(false)),
});

const inspections = entity('inspections', {
  unit: t.ref('units'),
  workOrder: t.ref('workOrders'),
  at: t.date(),
});

const buildings = entity('buildings', {
  property: t.ref('properties').owned(),
  name: t.text(),
  seats: t.int(),
  used: t.int().default(lit(0)),
})
  .check(f.used.le(f.seats))
  .check(f.used.ge(lit(0)));

const invoices = entity('invoices', {
  vendor: t.ref('vendors'),
  number: t.text(),
  issued: t.date(),
  total: t.money().sealed(),
})
  .unique('vendor', 'number');

const invoiceLines = entity('invoiceLines', {
  invoice: t.ref('invoices').owned(),
  workItem: t.ref('workItems').optional(),
  amount: t.money().sealed(),
});

const payments = entity('payments', {
  invoice: t.ref('invoices'),
  paid: t.date(),
  amount: t.money().sealed(),
});

const meters = entity('meters', {
  unit: t.ref('units').owned(),
  kind: t.text(),
});

const readings = entity('readings', {
  meter: t.ref('meters'),
  at: t.datetime(),
  value: t.int(),
})
  .unique('meter', 'at');

const devices = entity('devices', {
  location: t.ref('locations'),
  kind: t.text(),
  externalId: t.text().unique(),
  armed: t.bool().default(lit(false)),
});

const events = entity('events', {
  device: t.ref('devices'),
  at: t.datetime(),
  kind: t.text(),
});

const alerts = entity('alerts', {
  kind: t.text(),
  key: t.text().unique(),
  severity: t.text().default(lit('warning')),
  title: t.text().sealed(),
  status: t.text().default(lit('active')),
  raised: t.date(),
});

const pages = entity('pages', {
  title: t.text(),
  settings: t.json().default(lit('{}')),
  draft: t.json().optional(),
});

export const ordo: string = synth(
  domain('estate', '1.0.0'),
  properties,
  locations,
  units,
  people,
  leases,
  leaseParties,
  trades,
  vendors,
  vendorTrades,
  workOrders,
  workItems,
  inspections,
  buildings,
  invoices,
  invoiceLines,
  payments,
  meters,
  readings,
  devices,
  events,
  alerts,
  pages,
);
