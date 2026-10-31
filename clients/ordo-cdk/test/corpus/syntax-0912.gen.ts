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

import { asc, entity, f, lit, op, src, synth, t } from '../../src/index.js';

const invoices = entity('invoices', {
  number: t.text().unique(),
  status: t.text().default(lit('open')),
  total: t.int(),
});

const openInvoices = op('openInvoices', {}, (b) =>
  b
    .source(src('invoices').args(f.status.eq(lit('open'))).pick('number', 'total'))
    .sort(asc(f.number))
    .take(50))
  .nopause();

const markPaid = op('markPaid', { number: t.text() }, (b, p) =>
  [
    b.set('invoices', { status: lit('paid') }).where(f.number.eq(p.number)),
  ])
  .nopause();

const addInvoice = op('addInvoice', { number: t.text(), total: t.int() }, (b, p) =>
  [
    b.set('invoices', { number: p.number, total: p.total }),
  ]);

export const ordo: string = synth(
  invoices,
  openInvoices,
  markPaid,
  addInvoice,
);
