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

import { as_, desc, f, op, src, synth, t } from '../../src/index.js';

const activityFeed = op('activityFeed', { vendor: t.id() }, (b, p) =>
  b
    .source(src('workOrders').args(f.vendor.eq(p.vendor)).pick(as_('title', f.status), as_('at', f.opened)))
    .append(src('invoices').args(f.vendor.eq(p.vendor)).pick(as_('title', f.number), as_('at', f.issued)))
    .sort(desc(f.at))
    .take(50));

export const ordo: string = synth(
  activityFeed,
);
