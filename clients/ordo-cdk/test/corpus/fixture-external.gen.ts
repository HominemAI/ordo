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

import { external, g, key, synth, t } from '../../src/index.js';

const accounts = external('accounts', {
  id: key.uuid('ref'),
  label: t.text().column('label'),
}, { schema: 'crm', table: 'accounts', key: 'acct', shared: true });

const customers = external('customers', {
  id: key.int('customer_id'),
  name: t.text().public().column('full_name'),
  email: t.text().optional().column('email'),
  account: t.ref('accounts').optional().column('account_id'),
}, { schema: 'crm', table: 'customers', shared: true })
  .public()
  .accessRead(g.OPEN);

const tags = external('tags', {
  id: key.text('Code'),
  title: t.text().column('Title'),
}, { schema: 'Legacy', table: 'Tag Rows', shared: true });

export const ordo: string = synth(
  accounts,
  customers,
  tags,
);
