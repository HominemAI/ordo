import { useCommit, useOpQuery } from '@hominemai/ordo-react';
import { useMemo, useState } from 'react';
import { refusalText } from '../errors.js';
import { message, useOnline } from '../hooks.js';
import { noteOf, parseTags, stamp, type Note } from '../notes.js';
import { NoteEditor } from './NoteEditor.js';

/** The person's own notes, drawn from the device, newest first. */
export function NotesView() {
  const online = useOnline();
  // `myNotes` is syncable, so the provider's device copy answers it, in the
  // order the descriptor states (newest first), with or without a network.
  const query = useOpQuery('myNotes', { live: true });
  const [tag, setTag] = useState<string | null>(null);
  const tagged = useOpQuery('notesTagged', { params: { tag: tag ?? '' }, live: true, enabled: tag !== null });
  const create = useCommit('createNote');
  const source = tag === null ? query : tagged;
  const notes = useMemo(() => source.rows.map((r) => noteOf({ ...r })), [source.rows]);
  const [filter, setFilter] = useState('');
  const [open, setOpen] = useState<Note | null>(null);
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [tagText, setTagText] = useState('');
  const [thrown, setThrown] = useState<string | null>(null);

  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (q === '') return notes;
    return notes.filter((n) => n.title.toLowerCase().includes(q) || n.body.toLowerCase().includes(q));
  }, [notes, filter]);

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

  const error = thrown ?? (create.refusal ? refusalText(create.refusal) : null);
  const busy = create.committing;

  if (open !== null) {
    return (
      <NoteEditor key={open.id} note={open} online={online} onClose={() => setOpen(null)} />
    );
  }

  return (
    <section aria-label="Your notes">
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
              : 'Nothing matches.'}
        </p>
      )}
      <ul className="notes">
        {shown.map((n) => (
          <li key={n.id}>
            <button className="note" onClick={() => setOpen(n)}>
              <strong>{n.title || 'Untitled'}</strong>
              <span>{n.body.split('\n')[0]}</span>
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
          </li>
        ))}
      </ul>
    </section>
  );
}
