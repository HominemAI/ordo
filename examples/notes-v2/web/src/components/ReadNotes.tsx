import { useOpQuery } from '@hominemai/ordo-react';
import { useMemo } from 'react';
import { arrange, noteOf, type Note } from '../notes.js';
import { NoteFiles } from './Files.js';

/** What a share lets its guest read, from the ops the door named. */
export interface Readable {
  /** `allNotes` for a share made under v2, `myNotes` for one made under v1. */
  readonly op: 'allNotes' | 'myNotes';
  /** Whether `myFiles` and the file door are in the share. */
  readonly files: boolean;
}

/**
 * Pick the ops to read from the ones the share lists. The page never reads an
 * op the share does not list: the door refuses it as `access_denied`, and a
 * device copy reads that as the share being closed.
 */
export function readableOf(ops: readonly string[]): Readable {
  return {
    op: ops.includes('allNotes') ? 'allNotes' : 'myNotes',
    files: ops.includes('myFiles') && ops.includes('std.$blob.get'),
  };
}

/** A guest's order: pinned first, then newest, then the archived ones. */
export function guestOrder(notes: readonly Note[]): Note[] {
  return [...arrange(notes, 'notes'), ...arrange(notes, 'archive')];
}

/** The notes a share or a link shows, read-only, with their files when the share carries them. */
export function useSharedNotes(readable: Readable) {
  const query = useOpQuery(readable.op);
  const notes = useMemo(() => guestOrder(query.rows.map((r) => noteOf({ ...r }))), [query.rows]);
  return { query, notes };
}

export function SharedList({ notes, readable }: { readonly notes: readonly Note[]; readonly readable: Readable }) {
  return (
    <ul className="notes">
      {notes.map((n) => (
        <li key={n.id} className="readonly">
          <strong>
            {n.pinned && <span className="pin">Pinned: </span>}
            {n.title || 'Untitled'}
            {n.archived !== null && <span className="hint"> (archived)</span>}
          </strong>
          <p>{n.body}</p>
          {readable.files && <NoteFiles note={n.id} />}
        </li>
      ))}
    </ul>
  );
}
