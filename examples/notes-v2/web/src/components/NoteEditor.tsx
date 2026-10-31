import { useCommit, useRow } from '@hominemai/ordo-react';
import { useState } from 'react';
import { refusalText } from '../errors.js';
import { message } from '../hooks.js';
import { noteOf, parseTags, tagsText, type Note } from '../notes.js';
import { NoteFiles } from './Files.js';
import { ReminderControl } from './Reminders.js';
import { SharePanel } from './SharePanel.js';

interface Props {
  readonly note: Note;
  readonly online: boolean;
  readonly onClose: () => void;
}

/**
 * One note: edit, delete, share, and its files.
 *
 * A save asserts the revision the person opened. When somebody else changed
 * the note first (another tab, another device), `useCommit` reports a
 * conflict and nothing is overwritten: the editor shows what is stored now,
 * and the person keeps their text or takes the other.
 */
export function NoteEditor({ note, online, onClose }: Props) {
  const [base, setBase] = useState<Note>(note);
  const [title, setTitle] = useState(note.title);
  const [body, setBody] = useState(note.body);
  const [tagText, setTagText] = useState(tagsText(note.tags));
  const [thrown, setThrown] = useState<string | null>(null);
  const [sharing, setSharing] = useState(false);
  // `autoResolve` reads the stored row back when a save loses, and
  // `state.currentIs` says what that found.
  const edit = useCommit('editNote', { autoResolve: true });
  const del = useCommit('deleteNote');
  const { bind } = useRow(base.id);
  const busy = edit.committing || del.committing;

  const conflict = edit.state.status === 'conflict' ? edit.state : null;
  const theirs = conflict?.current ? noteOf({ ...conflict.current }) : null;

  const guard = async (act: () => Promise<{ readonly status: string }>) => {
    setThrown(null);
    try {
      if ((await act()).status === 'committed') onClose();
    } catch (e) {
      setThrown(message(e));
    }
  };

  const refusal = edit.refusal ?? del.refusal;
  const error = thrown ?? (refusal ? refusalText(refusal) : null);

  return (
    <section className="card" aria-label="Note">
      <label htmlFor="title">Title</label>
      <input id="title" value={title} onChange={(e) => setTitle(e.target.value)} disabled={busy} />
      <label htmlFor="body">Note</label>
      <textarea id="body" rows={8} value={body} onChange={(e) => setBody(e.target.value)} disabled={busy} />

      <label htmlFor="tags">Tags</label>
      <input
        id="tags"
        value={tagText}
        onChange={(e) => setTagText(e.target.value)}
        placeholder="work, ideas"
        disabled={busy}
      />

      <ReminderControl note={base.id} online={online} />

      <NoteFiles note={base.id} editable online={online} />

      {conflict !== null && conflict.currentIs === 'deleted' && (
        <div role="alert" className="conflict">
          <p>This note was deleted somewhere else. Your text is still here.</p>
        </div>
      )}
      {conflict !== null && conflict.currentIs === 'unavailable' && (
        <div role="alert" className="conflict">
          <p>
            This note was changed somewhere else, and what is stored now could not be read. Nothing was overwritten.
            Your text is still here.
          </p>
        </div>
      )}
      {theirs !== null && (
        <div role="alert" className="conflict">
          <p>
            <strong>Changed elsewhere.</strong> Someone saved this note after you opened it. Nothing was overwritten.
            This is what is stored now:
          </p>
          <blockquote>
            <strong>{theirs.title}</strong>
            <br />
            {theirs.body}
            {theirs.tags.length > 0 && <em> Tags: {tagsText(theirs.tags)}</em>}
          </blockquote>
          <div className="row">
            <button
              className="primary"
              disabled={busy || !online}
              onClick={() => {
                setBase(theirs);
                void guard(() => edit.retry({ id: theirs.id, title, body, tags: parseTags(tagText) }, theirs.rev));
              }}
            >
              Keep my text
            </button>
            <button
              disabled={busy}
              onClick={() => {
                setBase(theirs);
                setTitle(theirs.title);
                setBody(theirs.body);
                setTagText(tagsText(theirs.tags));
                edit.reset();
              }}
            >
              Use theirs
            </button>
          </div>
        </div>
      )}

      <div className="row">
        <button
          className="primary"
          disabled={busy || !online || conflict !== null}
          onClick={() => void guard(() => edit.commit({ id: base.id, title, body, tags: parseTags(tagText) }, base.rev))}
        >
          Save
        </button>
        <button disabled={busy || !online} onClick={() => void guard(() => del.commit(bind('deleteNote') ?? {}))}>
          Delete
        </button>
        <button onClick={() => setSharing((s) => !s)} aria-expanded={sharing}>
          Share
        </button>
        <button onClick={onClose}>Close</button>
      </div>
      {!online && (
        <p className="hint">
          You are offline. You can read your notes; saving needs the network, and what you typed stays here until it
          comes back.
        </p>
      )}
      {error !== null && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {sharing && <SharePanel note={base} online={online} />}
    </section>
  );
}
