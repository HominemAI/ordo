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

import { and, f, lit, op, or, path, src, synth, t } from '../../src/index.js';

const darkPages = op('darkPages', {}, (b) =>
  b
    .source(src('pages').args(and(f.settings.theme.eq(lit('dark')), path('settings', 'font size').gt(lit(12)))).pick('title')))
  .unbounded();

const taggedPages = op('taggedPages', { tag: t.text() }, (b, p) =>
  b
    .source(src('pages').args(or(f.settings.tags.contains(p.tag), path('settings', 'tags', '0').eq(p.tag))).pick('title')))
  .unbounded();

export const ordo: string = synth(
  darkPages,
  taggedPages,
);
