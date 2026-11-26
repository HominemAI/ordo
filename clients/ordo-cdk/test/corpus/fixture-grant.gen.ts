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

import { entity, grant, grantProvider, peer, synth, t } from '../../src/index.js';

const journal = peer('journal', ['std.entries.list', 'addEntry']);

const notes = entity('notes', {
  title: t.text(),
});

export const ordo: string = synth(
  journal,
  grant('note_viewer', ['std.notes.get', 'std.notes.list', '/std\\.notesAccess\\..*/', '/a\\/b/', '!std.notes.delete']),
  grant('billing', ['*', '!/std\\..*/'], { asItself: true }),
  grantProvider('hominem', ['std.notes.get']),
  notes,
);
