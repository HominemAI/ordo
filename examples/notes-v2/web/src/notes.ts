/**
 * What the page knows about a note, with no React and no network: the shape
 * of a note, and the parameters and answers of the sharing ops. Creating,
 * editing, deleting, unsharing and sharing are the hooks' (`useCommit`), and
 * so are the list, the readers and the links (`useOpQuery`); this file only
 * builds what they send and reads what they answer, so it is tested with no
 * render.
 */

export interface Note {
  readonly id: string;
  readonly rev: number;
  readonly title: string;
  readonly body: string;
  readonly tags: readonly string[];
  /** When the page that made the note stamped it, `YYYY-MM-DDTHH:MM:SSZ`. */
  readonly created: string;
  /** Shown before the others. A note written under v1 reads `false`. */
  readonly pinned: boolean;
  /** When it was archived, or `null` for a note in the list. A v1 note reads `null`. */
  readonly archived: string | null;
}

/** A share lasts a year and is renewed by sharing again (plan, section 10). */
export const SHARE_LIFETIME = '365d';

/** What `datetime` parameters look like on the wire: seconds, and a `Z`. */
export function stamp(now: Date = new Date()): string {
  return `${now.toISOString().slice(0, 19)}Z`;
}

export function noteOf(row: Record<string, unknown>): Note {
  return {
    id: String(row['id']),
    rev: Number(row['rev']),
    title: String(row['title'] ?? ''),
    body: String(row['body'] ?? ''),
    tags: Array.isArray(row['tags']) ? (row['tags'] as unknown[]).filter((t): t is string => typeof t === 'string') : [],
    created: String(row['created'] ?? ''),
    pinned: row['pinned'] === true || row['pinned'] === 'true' || row['pinned'] === 't',
    archived: typeof row['archived'] === 'string' && row['archived'] !== '' ? row['archived'] : null,
  };
}

/** Which notes the list shows: the ones in use, or the archive. */
export type Shelf = 'notes' | 'archive';

/**
 * The list as the page shows it: the shelf's notes, pinned first, each group
 * newest first. It runs on whatever rows the device holds, so it works offline.
 */
export function arrange(notes: readonly Note[], shelf: Shelf): Note[] {
  const on = notes.filter((n) => (shelf === 'archive') === (n.archived !== null));
  const sorted = newestFirst(on);
  return [...sorted.filter((n) => n.pinned), ...sorted.filter((n) => !n.pinned)];
}

/** Tags typed as `work, ideas`: trimmed, lower case, no empties or repeats, at most 20 of at most 40 characters. */
export function parseTags(text: string): string[] {
  const out: string[] = [];
  for (const part of text.split(',')) {
    const tag = part.trim().toLowerCase();
    if (tag === '' || tag.length > 40 || out.includes(tag)) continue;
    out.push(tag);
    if (out.length === 20) break;
  }
  return out;
}

export function tagsText(tags: readonly string[]): string {
  return tags.join(', ');
}

/** Newest first. Ties keep the order the door sent. */
export function newestFirst(notes: readonly Note[]): Note[] {
  return notes
    .map((note, i) => ({ note, i }))
    .sort((a, b) => (a.note.created < b.note.created ? 1 : a.note.created > b.note.created ? -1 : a.i - b.i))
    .map((e) => e.note);
}

// ── sharing, as the person who owns the note ─────────────────────────────

/**
 * The ops a share or a link made under v2 lists: the notes with their pins,
 * the files on them, and the row read that lets the guest open a file (a
 * share that lists a `std.<entity>.get` carries the file door's check).
 * A share made under v1 listed `myNotes` alone and still works; it shows no
 * files and no pins (`SHARED_V1`).
 */
export const SHARED_OPS = 'allNotes,myFiles,std.attachments.get';

/** What a share made under v1 lists. */
export const SHARED_V1 = 'myNotes';

/** The rows a share or a link opens: this one note, read-only. */
const oneNote = (note: Note) => JSON.stringify([{ entity: 'notes', id: note.id, level: 'read' }]);

/** What a note is called in the share, at most 200 characters. */
const labelOf = (note: Note) => note.title.slice(0, 200) || 'A note';

export interface Link {
  readonly ticket: string;
  readonly url: string;
}

/** The address the recipient opens. The ticket rides in the fragment, so no server ever receives it. */
export function linkFor(origin: string, ticket: string): string {
  return `${origin.replace(/\/+$/, '')}/#/accept/${ticket}`;
}

/** The parameters of `std.space.share` for one note, read-only, for a year. */
export function shareParams(note: Note) {
  return { ops: SHARED_OPS, rows: oneNote(note), expires: SHARE_LIFETIME, label: labelOf(note) };
}

/** The link to send, from the row `std.space.share` answered. */
export function linkOfShare(row: { readonly ticket?: unknown } | undefined, origin: string): Link {
  const ticket = row?.ticket;
  if (typeof ticket !== 'string') throw new Error('The deployment did not answer a share ticket.');
  return { ticket, url: linkFor(origin, ticket) };
}

/** One guest who can read one note. `member` is their seat in this tenant. */
export interface Reader {
  readonly member: string;
}

/**
 * The name to show for each reader, by seat. `std.space.shares` is the
 * owner's list of their own grants: it carries the display name the guest
 * agreed to show the sharer, and no principal id. A seat with no name there
 * is left out, and the page says "A reader" for it.
 */
export function readerNames(rows: readonly Record<string, unknown>[]): Map<string, string> {
  const names = new Map<string, string>();
  for (const r of rows) {
    const seat = r['seat'];
    const name = r['displayName'];
    if (typeof seat === 'string' && typeof name === 'string' && name.trim() !== '') names.set(seat, name.trim());
  }
  return names;
}

/** Who holds `read` on a note, from the access rows: the rows other than the owner's own. */
export function readersIn(rows: readonly Record<string, unknown>[], note: Note): Reader[] {
  return rows
    .filter((r) => String(r['note']) === note.id && String(r['level']) === 'read')
    .map((r) => ({ member: String(r['member']) }));
}

// ── share links: anyone with the link reads, with no account ─────────────

/** A share link lasts a week. */
export const LINK_LIFETIME = '7d';

/** A link just made. The secret is shown once and the door keeps only its hash. */
export interface MadeLink {
  readonly id: string;
  readonly url: string;
  /** Seconds since the epoch. */
  readonly expiresAt: number;
}

/** The address a reader opens. The secret rides in the fragment, so no server receives it. */
export function readLinkFor(origin: string, secret: string): string {
  return `${origin.replace(/\/+$/, '')}/#/link/${secret}`;
}

/**
 * The parameters of `std.space.shareLink` for one note, read-only, for a
 * week. A password, when given, must be typed by whoever opens the link. It
 * is sent as typed; an empty one is no password.
 */
export function shareLinkParams(note: Note, password = '') {
  const base = { ops: SHARED_OPS, rows: oneNote(note), expires: LINK_LIFETIME, label: labelOf(note) };
  return password === '' ? base : { ...base, password };
}

/** The parameters of `std.space.setLinkPassword`: an empty password takes it off. */
export function linkPasswordParams(link: string, password = '') {
  return password === '' ? { link } : { link, password };
}

/** The link to show once, from the row `std.space.shareLink` answered. */
export function madeLinkOf(
  row: { readonly link?: unknown; readonly id?: unknown; readonly expiresAt?: unknown } | undefined,
  origin: string,
): MadeLink {
  if (typeof row?.link !== 'string' || typeof row.id !== 'string' || typeof row.expiresAt !== 'number') {
    throw new Error('The deployment did not answer a share link.');
  }
  return { id: row.id, url: readLinkFor(origin, row.link), expiresAt: row.expiresAt };
}

/** One of this tenant's links, as the door lists it. Never the secret. */
export interface ReadLink {
  readonly id: string;
  readonly label: string;
  readonly opens: number;
  readonly expiresAt: number;
  readonly state: 'live' | 'expired' | 'revoked';
  /** Whether opening it asks for a password. */
  readonly hasPassword: boolean;
  /** Seconds since the epoch when a lock after too many wrong passwords ends, or `null`. */
  readonly lockedUntil: number | null;
}

/** The links that open this note, from the rows `std.space.links` answered. */
export function readLinksOf(rows: readonly Record<string, unknown>[], note: Note): ReadLink[] {
  return rows
    .filter((r) => Array.isArray(r['rows']) && (r['rows'] as { id?: unknown }[]).some((x) => x.id === note.id))
    .map((r) => ({
      id: String(r['id']),
      label: String(r['label'] ?? ''),
      opens: Number(r['opens'] ?? 0),
      expiresAt: Number(r['expiresAt'] ?? 0),
      state: r['state'] === 'revoked' ? 'revoked' : r['state'] === 'expired' ? 'expired' : 'live',
      hasPassword: r['hasPassword'] === true,
      lockedUntil: typeof r['lockedUntil'] === 'number' ? r['lockedUntil'] : null,
    }));
}
