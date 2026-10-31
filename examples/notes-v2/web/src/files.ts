/**
 * Files on notes, and the export, with no React.
 *
 * A file is two things. The row (`attachments`) says which note it is on,
 * its name, its media type and its size, and is written by an op like any
 * other. The bytes go through the blob door after the row exists, with the
 * SDK's and the hooks' file helpers (`uploadFile`, `useFile`,
 * `useFileDownload`): the page makes no blob door call of its own.
 *
 * The row keeps `kind` and `size` still. The door can answer a file's type
 * and size now (`fileInfo`), but an op cannot read them, and a list that
 * shows every file of every note would need one door call per file. So the
 * page writes both when it adds the row and reads them from the row.
 *
 * The download of everything is Ordo's own (`GET /download`): this file only
 * names and saves it.
 */

import { MAX_FILE_BYTES } from '@hominemai/ordo-react';
import { stamp } from './notes.js';

/**
 * The largest file the page sends, in bytes. The door does not tell a page
 * its cap, so this is the deployment's documented default (`max_object`,
 * 100 MiB, `docs/settings.md`), the same ceiling the SDK enforces. A
 * deployment that lowers it still refuses at the door; the page then shows
 * that refusal. Named here once.
 */
export const FILE_LIMIT_BYTES: number = MAX_FILE_BYTES;

/** The words for a file over the limit, or `null` when it fits. */
export function tooLargeText(file: { readonly name: string; readonly size: number }): string | null {
  return file.size > FILE_LIMIT_BYTES
    ? `${file.name} is ${sizeText(file.size)}. The limit is ${sizeText(FILE_LIMIT_BYTES)}, so it was not added.`
    : null;
}

/** One file on one note, as `myFiles` answers it. */
export interface Attachment {
  readonly id: string;
  readonly rev: number;
  readonly note: string;
  readonly name: string;
  /** The media type the browser gave the file, `image/png`. */
  readonly kind: string;
  readonly size: number;
  readonly created: string;
  /** The blob door's handle, or `null` while the bytes are not there yet. */
  readonly file: string | null;
}

export function attachmentOf(row: Record<string, unknown>): Attachment {
  const file = row['file'];
  return {
    id: String(row['id']),
    rev: Number(row['rev']),
    note: String(row['note'] ?? ''),
    name: String(row['name'] ?? ''),
    kind: String(row['kind'] ?? ''),
    size: Number(row['size'] ?? 0),
    created: String(row['created'] ?? ''),
    file: typeof file === 'string' && file !== '' ? file : null,
  };
}

/** The files of one note, oldest first. */
export function filesOf(files: readonly Attachment[], note: string): Attachment[] {
  return files.filter((f) => f.note === note).sort((a, b) => (a.created < b.created ? -1 : a.created > b.created ? 1 : 0));
}

/** Shown in the page, not offered as a download. */
export function isImage(a: Pick<Attachment, 'kind'>): boolean {
  return /^image\/(png|jpeg|gif|webp|avif)$/.test(a.kind);
}

/** `1.2 MB`, for a person. */
export function sizeText(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** The parameters of `addAttachment` for a file the person picked. */
export function attachParams(note: string, file: { readonly name: string; readonly type: string; readonly size: number }) {
  return {
    note,
    name: file.name.slice(0, 200) || 'file',
    kind: file.type || 'application/octet-stream',
    size: file.size,
    created: stamp(),
  };
}

/** The id a create answered. A create's summary row is `{ entity, created: <id> }`. */
export function createdId(rows: readonly Record<string, unknown>[] | undefined): string {
  for (const r of rows ?? []) {
    const id = r['created'];
    if (typeof id === 'string' && id !== '' && (r['entity'] === undefined || r['entity'] === 'attachments')) return id;
  }
  throw new Error('The deployment did not answer the new file’s id.');
}

// ── the download ─────────────────────────────────────────────────────────
//
// "Download all my notes" is Ordo's own download, `GET /download`, through the
// SDK (`useOwnDownload`). The program marks `allNotes` and `myFiles` with
// `download`, so the zip holds `notes.json`, `attachments.json` and each file
// under `files/attachments/`, read as this person under the row guard.

/** `notes-2026-10-22.zip`. */
export function exportName(now: Date = new Date()): string {
  return `notes-${stamp(now).slice(0, 10)}.zip`;
}
