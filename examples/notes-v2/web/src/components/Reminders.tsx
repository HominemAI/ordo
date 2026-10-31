import { useCommit, useOpQuery } from '@hominemai/ordo-react';
import { createContext, useContext, useMemo, useState, type ReactNode } from 'react';
import { refusalText } from '../errors.js';
import { message } from '../hooks.js';
import type { Note } from '../notes.js';
import {
  byNote,
  firedOnly,
  localInputOf,
  reminderOf,
  stampOfLocalInput,
  whenText,
  type Reminder,
} from '../reminders.js';

/**
 * Every reminder on the person's notes, live from `myReminders`. No share
 * lists that op, so a guest reading a shared note is refused it and sees no
 * reminder: only the owner is reminded.
 */
export function useReminders(): { readonly reminders: readonly Reminder[]; readonly onNote: ReadonlyMap<string, Reminder> } {
  const query = useOpQuery('myReminders', { live: true });
  const reminders = useMemo(
    () => (query.refusal ? [] : query.rows.map((r) => reminderOf({ ...r }))),
    [query.rows, query.refusal],
  );
  return { reminders, onNote: useMemo(() => byNote(reminders), [reminders]) };
}

/**
 * The reminders that fired, at the top of the list. It is a polite live
 * region that stays in the page, so a reminder firing while the page is open
 * is announced as well as shown. "Done" takes the reminder off the note.
 */
export function ReminderNotices({ notes, online }: { readonly notes: readonly Note[]; readonly online: boolean }) {
  const { reminders } = useReminders();
  const clear = useCommit('clearReminder');
  const [thrown, setThrown] = useState<string | null>(null);
  const due = firedOnly(reminders);
  const title = (id: string) => notes.find((n) => n.id === id)?.title || 'Untitled';

  const done = async (r: Reminder) => {
    setThrown(null);
    clear.reset();
    try {
      await clear.commit({ note: r.note });
    } catch (e) {
      setThrown(message(e));
    }
  };
  const error = thrown ?? (clear.refusal ? refusalText(clear.refusal) : null);

  return (
    <div role="status" aria-live="polite" className="reminders">
      {due.map((r) => (
        <p key={r.id} className="banner reminder">
          <span>
            Reminder: <strong>{title(r.note)}</strong> was due {whenText(r.due)}.
          </span>
          <button disabled={!online || clear.committing} onClick={() => void done(r)}>
            Done <span className="visually-hidden">with the reminder on {title(r.note)}</span>
          </button>
        </p>
      ))}
      {error !== null && <p className="error">{error}</p>}
    </div>
  );
}

/** The mark on a note in the list: when its reminder is due, or that it is due now. */
export function ReminderBadge({ reminder }: { readonly reminder: Reminder | undefined }) {
  if (reminder === undefined) return null;
  return reminder.fired !== null ? (
    <span className="reminder-mark due">Reminder due</span>
  ) : (
    <span className="reminder-mark">Reminder {whenText(reminder.due)}</span>
  );
}

const RemindersContext = createContext<ReadonlyMap<string, Reminder> | null>(null);

/** The person's reminders, for the editor below. The list provides them. */
export function RemindersOf({ value, children }: { readonly value: ReadonlyMap<string, Reminder>; readonly children: ReactNode }) {
  return <RemindersContext.Provider value={value}>{children}</RemindersContext.Provider>;
}

/**
 * Set, move or take off a note's reminder, in the note's editor. The time
 * is read in the person's own time zone. Setting it never touches the note,
 * so it never causes a conflict with the text being edited. With no
 * reminders above it (an editor shown on its own), it renders nothing.
 */
export function ReminderControl(props: { readonly note: string; readonly online: boolean }) {
  const onNote = useContext(RemindersContext);
  return onNote === null ? null : <Control {...props} current={onNote.get(props.note)} />;
}

function Control({
  note,
  online,
  current,
}: {
  readonly note: string;
  readonly online: boolean;
  readonly current: Reminder | undefined;
}) {
  const [value, setValue] = useState<string | null>(null);
  const [thrown, setThrown] = useState<string | null>(null);
  const set = useCommit('setReminder');
  const clear = useCommit('clearReminder');
  const shown = value ?? (current ? localInputOf(current.due) : '');
  const due = stampOfLocalInput(shown);
  const busy = set.committing || clear.committing;

  const act = async (go: () => Promise<{ readonly status: string }>) => {
    setThrown(null);
    set.reset();
    clear.reset();
    try {
      if ((await go()).status === 'committed') setValue(null);
    } catch (e) {
      setThrown(message(e));
    }
  };
  const refusal = set.refusal ?? clear.refusal;
  const error = thrown ?? (refusal ? refusalText(refusal) : null);

  return (
    <div className="reminder-control">
      <label htmlFor={`remind-${note}`}>Remind me at</label>
      <p id={`remind-hint-${note}`} className="hint">
        {current === undefined
          ? 'No reminder. You are told here when it comes, and by email if you sign in by email.'
          : current.fired !== null
            ? `The reminder was due ${whenText(current.due)}.`
            : `A reminder is set for ${whenText(current.due)}.`}
      </p>
      <div className="row">
        <input
          id={`remind-${note}`}
          type="datetime-local"
          aria-describedby={`remind-hint-${note}`}
          value={shown}
          onChange={(e) => setValue(e.target.value)}
          disabled={busy}
        />
        <button
          disabled={busy || !online || due === null}
          onClick={() => due !== null && void act(() => set.commit({ note, due }))}
        >
          {current === undefined ? 'Set reminder' : 'Change reminder'}
        </button>
        {current !== undefined && (
          <button disabled={busy || !online} onClick={() => void act(() => clear.commit({ note }))}>
            Remove reminder
          </button>
        )}
      </div>
      {error !== null && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </div>
  );
}
