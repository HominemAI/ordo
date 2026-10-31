import { useCommit, useOpQuery, useOwnDownload } from '@hominemai/ordo-react';
import { useMemo, useState } from 'react';
import { refusalText } from '../errors.js';
import { message, useOnline } from '../hooks.js';
import { saveBlob } from '@hominemai/ordo-react';
import { exportName } from '../files.js';
import { arrange, noteOf, parseTags, stamp, type Note, type Shelf } from '../notes.js';
import { useFilesOn } from './Files.js';
import { NoteEditor } from './NoteEditor.js';
import { ReminderBadge, ReminderNotices, RemindersOf, useReminders } from './Reminders.js';

/**
 * The person's own notes, drawn from the device: pinned first, then newest
 * first. The archive is a second shelf. Both are sorted and filtered here,
 * from the rows the device holds, so they work with the network off.
 */
export function NotesView() {
  const online = useOnline();
  // `allNotes` is syncable, so the provider's device copy answers it, with
  // or without a network.
  const query = useOpQuery('allNotes', { live: true });
  const [tag, setTag] = useState<string | null>(null);
  const tagged = useOpQuery('notesTagged', { params: { tag: tag ?? '' }, live: true, enabled: tag !== null });
  const create = useCommit('createNote');
  const source = tag === null ? query : tagged;
  const notes = useMemo(() => source.rows.map((r) => noteOf({ ...r })), [source.rows]);
  const [filter, setFilter] = useState('');
  const [shelf, setShelf] = useState<Shelf>('notes');
  const pin = useCommit('pinNote');
  const archive = useCommit('archiveNote');
  const unarchive = useCommit('unarchiveNote');
  const filesOn = useFilesOn();
  // "Download all my notes" is Ordo's own download: the program marks
  // `allNotes` and `myFiles` with `download`, and the door builds the zip.
  const own = useOwnDownload();
  const { onNote } = useReminders();
  const [open, setOpen] = useState<Note | null>(null);
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [tagText, setTagText] = useState('');
  const [thrown, setThrown] = useState<string | null>(null);

  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const on = arrange(notes, shelf);
    if (q === '') return on;
    return on.filter((n) => n.title.toLowerCase().includes(q) || n.body.toLowerCase().includes(q));
  }, [notes, filter, shelf]);
  const archivedCount = useMemo(() => notes.filter((n) => n.archived !== null).length, [notes]);

  /** Pin, unpin, archive or restore one note, at the revision the list holds. */
  const change = async (act: () => Promise<{ readonly status: string }>) => {
    setThrown(null);
    pin.reset();
    archive.reset();
    unarchive.reset();
    try {
      await act();
    } catch (e) {
      setThrown(message(e));
    }
  };

  /** Every note, archived ones too, and every file, as one zip from the door. */
  const download = async () => {
    if (!filesOn) return;
    setThrown(null);
    const got = await own.download();
    if (got !== null) saveBlob(got.blob, exportName());
  };

  const add = async () => {
    setThrown(null);
    try {
      const out = await create.commit({ title: title.trim(), body, tags: parseTags(tagText), created: stamp() });
      if (out.status === 'committed') {
        setTitle('');
        setBody('');
        setTagText('');
        create.reset();
      }
      // Otherwise the text stays in the box and the refusal is shown.
    } catch (e) {
      setThrown(message(e));
    }
  };

  const refused = create.refusal ?? pin.refusal ?? archive.refusal ?? unarchive.refusal;
  const error = thrown ?? (refused ? refusalText(refused) : own.error ? message(own.error) : null);
  const changing = pin.committing || archive.committing || unarchive.committing;
  const busy = create.committing;

  if (open !== null) {
    return (
      <RemindersOf value={onNote}>
        <NoteEditor key={open.id} note={open} online={online} onClose={() => setOpen(null)} />
      </RemindersOf>
    );
  }

  return (
    <section aria-label="Your notes">
      <ReminderNotices notes={notes} online={online} />
      {!online && (
        <p className="banner">
          You are offline. These are the notes this device holds. Writing needs the network.
        </p>
      )}
      <div className="card">
        <label htmlFor="new-title">New note</label>
        <input
          id="new-title"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="Title"
          disabled={busy}
        />
        <textarea
          aria-label="New note text"
          rows={3}
          value={body}
          onChange={(e) => setBody(e.target.value)}
          placeholder="Write something"
          disabled={busy}
        />
        <input
          aria-label="Tags"
          value={tagText}
          onChange={(e) => setTagText(e.target.value)}
          placeholder="work, ideas"
          disabled={busy}
        />
        <div className="row">
          <button className="primary" disabled={busy || !online || title.trim() === ''} onClick={() => void add()}>
            Add note
          </button>
        </div>
        {!online && <p className="hint">Adding a note needs the network. What you typed stays here.</p>}
        {error !== null && (
          <p role="alert" className="error">
            {error}
          </p>
        )}
      </div>

      <div className="row" role="group" aria-label="Which notes">
        <button aria-pressed={shelf === 'notes'} onClick={() => setShelf('notes')}>
          Notes
        </button>
        <button aria-pressed={shelf === 'archive'} onClick={() => setShelf('archive')}>
          Archived ({archivedCount})
        </button>
        <button disabled={!online || own.busy || !filesOn} onClick={() => void download()}>
          {own.busy ? 'Preparing your download' : 'Download all my notes'}
        </button>
      </div>

      <label htmlFor="filter">Find</label>
      <input
        id="filter"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        placeholder="Filter the notes on this device"
      />

      {tag !== null && (
        <p className="row">
          <span>Notes tagged {tag}</span>
          <button onClick={() => setTag(null)}>Show all</button>
        </p>
      )}

      {shown.length === 0 && (
        <p className="hint">
          {!source.complete && notes.length === 0
            ? 'Loading your notes.'
            : notes.length === 0
              ? tag !== null
                ? 'No note carries this tag.'
                : 'No notes yet. Write your first one above.'
              : shelf === 'archive' && archivedCount === 0
                ? 'Nothing is archived.'
                : 'Nothing matches.'}
        </p>
      )}
      <ul className="notes">
        {shown.map((n) => (
          <li key={n.id}>
            <button className="note" onClick={() => setOpen(n)}>
              <strong>
                {n.pinned && <span className="pin">Pinned: </span>}
                {n.title || 'Untitled'}
              </strong>
              <span>{n.body.split('\n')[0]}</span>
              <ReminderBadge reminder={onNote.get(n.id)} />
            </button>
            {n.tags.length > 0 && (
              <span className="tags">
                {n.tags.map((t) => (
                  <button key={t} className="tag" onClick={() => setTag(t)}>
                    {t}
                  </button>
                ))}
              </span>
            )}
            <span className="row actions">
              <button
                disabled={!online || changing}
                onClick={() => void change(() => pin.commit({ id: n.id, pinned: !n.pinned }, n.rev))}
              >
                {n.pinned ? 'Unpin' : 'Pin'} <span className="visually-hidden">{n.title || 'Untitled'}</span>
              </button>
              {n.archived === null ? (
                <button
                  disabled={!online || changing}
                  onClick={() => void change(() => archive.commit({ id: n.id, at: stamp() }, n.rev))}
                >
                  Archive <span className="visually-hidden">{n.title || 'Untitled'}</span>
                </button>
              ) : (
                <button disabled={!online || changing} onClick={() => void change(() => unarchive.commit({ id: n.id }, n.rev))}>
                  Restore <span className="visually-hidden">{n.title || 'Untitled'}</span>
                </button>
              )}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}
