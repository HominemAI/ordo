/**
 * Reminders on notes, with no React and no network (owner ruling
 * 2026-10-22: a note can have a due time, and the app tells the person when
 * it comes, in the app and by email).
 *
 * A reminder is a row of its own (`reminders` in notes.ordo), one per note.
 * The program's `fireReminders` routine marks it `fired` when it is due and
 * emails the note's owner. The page shows what fired as a notice, live.
 */

export interface Reminder {
  readonly id: string;
  readonly rev: number;
  /** The note it is on. */
  readonly note: string;
  /** When it is due, `YYYY-MM-DDTHH:MM:SSZ`. */
  readonly due: string;
  /** When the routine fired it, or `null` while it is still to come. */
  readonly fired: string | null;
}

export function reminderOf(row: Record<string, unknown>): Reminder {
  const fired = row['fired'];
  return {
    id: String(row['id']),
    rev: Number(row['rev']),
    note: String(row['note']),
    due: String(row['due'] ?? ''),
    fired: typeof fired === 'string' && fired !== '' ? fired : null,
  };
}

/** Each note's reminder, by note id. */
export function byNote(reminders: readonly Reminder[]): ReadonlyMap<string, Reminder> {
  return new Map(reminders.map((r) => [r.note, r]));
}

/** The reminders that fired, the oldest due first. */
export function firedOnly(reminders: readonly Reminder[]): Reminder[] {
  return reminders.filter((r) => r.fired !== null).sort((a, b) => (a.due < b.due ? -1 : a.due > b.due ? 1 : 0));
}

const pad = (n: number) => String(n).padStart(2, '0');

/**
 * What a `datetime-local` input shows for a stamp, in the person's own time
 * zone: `YYYY-MM-DDTHH:MM`. Empty for a stamp that does not parse.
 */
export function localInputOf(stamp: string): string {
  const d = new Date(stamp);
  if (Number.isNaN(d.getTime())) return '';
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/**
 * The stamp to send for what a `datetime-local` input holds, read in the
 * person's own time zone: `YYYY-MM-DDTHH:MM:SSZ`. `null` when it is empty or
 * does not parse.
 */
export function stampOfLocalInput(value: string): string | null {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2})?$/.test(value)) return null;
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return null;
  return `${d.toISOString().slice(0, 19)}Z`;
}

/** A due time as the person reads it, in their own time zone. */
export function whenText(stamp: string, locale?: string): string {
  const d = new Date(stamp);
  if (Number.isNaN(d.getTime())) return stamp;
  return d.toLocaleString(locale, { dateStyle: 'medium', timeStyle: 'short' });
}
