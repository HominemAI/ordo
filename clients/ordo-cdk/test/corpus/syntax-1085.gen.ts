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

import { entity, g, reader, synth, t } from '../../src/index.js';

const notes = entity('notes', {
  title: t.text(),
  owner: t.ref('members').optional(),
})
  .accessRead(g.walk(g.any(g.all(g.eq('owner')))))
  .accessWrite(g.walk(g.any(g.all(g.eq('owner')))));

const notesAccess = entity('notesAccess', {
  note: t.ref('notes').owned(),
  member: t.ref('members'),
});

export const ordo: string = synth(
  reader('note_viewer', ['notes', 'notesAccess']),
  notes,
  notesAccess,
);
