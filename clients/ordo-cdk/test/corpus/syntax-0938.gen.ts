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

import { asc, entity, f, op, src, synth, t } from '../../src/index.js';

const notices = entity('notices', {
  title: t.text().public(),
  body: t.text().public(),
  postedBy: t.text(),
});

const noticeFiles = entity('noticeFiles', {
  notice: t.ref('notices').owned(),
  file: t.blob().optional().public(),
})
  .public();

const noticeBoard = op('noticeBoard', {}, (b) =>
  b
    .source(src('notices').pick('title', 'body'))
    .sort(asc(f.title))
    .take(50))
  .public();

const noticeAuthors = op('noticeAuthors', {}, (b) =>
  b
    .source(src('notices').pick('title', 'postedBy'))
    .sort(asc(f.title))
    .take(50));

export const ordo: string = synth(
  notices,
  noticeFiles,
  noticeBoard,
  noticeAuthors,
);
