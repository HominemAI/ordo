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

import { entity, g, lit, rel, release, synth, t } from '../../src/index.js';

const people = entity('people', {
  member: t.ref('members').unique(),
  manager: t.ref('members').optional(),
});

const invoices = entity('invoices', {
  owner: t.ref('members'),
  status: t.text().default(lit('open')),
});

const drafts = entity('drafts', {
  author: t.ref('members'),
  body: t.text(),
});

export const ordo: string = synth(
  people,
  invoices,
  drafts,
  release.member.when(rel.any(rel.all(rel.none('invoices', g.any(g.all(g.eq('owner'), g.eq('status', lit('open'))))))), [rel.remove('drafts.author'), rel.reassign('invoices.owner', rel.lookup('people', g.any(g.all(g.eq('member'))), 'manager'), rel.owner)]),
);
