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

import { asc, external, f, g, key, op, peer, src, synth, t } from '../../src/index.js';

const journal = peer('journal', ['std.notes.list', 'addComment']);

const entries = external('entries', {
  id: key.text('id'),
  title: t.text().column('title'),
  owner: t.text().column('owner'),
}, { peer: journal, table: 'notes', shared: true })
  .accessRead(g.walk(g.any(g.all(g.eq('owner')))));

const customers = external('customers', {
  id: key.int('customer_id'),
  name: t.text().column('full_name'),
}, { schema: 'crm', table: 'customers', shared: true });

const myEntries = op('myEntries', {}, (b) =>
  b
    .source(src('entries').pick('title'))
    .sort(asc(f.title))
    .take(50));

export const ordo: string = synth(
  journal,
  entries,
  customers,
  myEntries,
);
