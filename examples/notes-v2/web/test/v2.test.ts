/**
 * What v2 adds, with no render and no network: pins and the archive on the
 * device's rows, the files' rows, the name the download is saved under, and
 * which ops a guest may read. The download itself is Ordo's (`GET /download`),
 * tested in `ordo-host`.
 */

import { describe, expect, it } from 'vitest';
import {
  attachmentOf,
  attachParams,
  createdId,
  exportName,
  filesOf,
  isImage,
  type Attachment,
} from '../src/files.js';
import { arrange, noteOf, shareLinkParams, shareParams, type Note } from '../src/notes.js';
import { guestOrder, readableOf } from '../src/components/ReadNotes.js';
import { pageAddress } from '../src/config.js';

const note = (n: number, extra: Partial<Note> = {}): Note => ({
  id: `note_${n}`,
  rev: 1,
  title: `t${n}`,
  body: '',
  tags: [],
  created: `2026-10-22T10:00:0${n}Z`,
  pinned: false,
  archived: null,
  ...extra,
});

describe('a note under v2', () => {
  it('reads a v1 row as not pinned and not archived', () => {
    const v1 = noteOf({ id: 'note_1', rev: 3, title: 'old', body: '', tags: [], created: '2026-09-29T10:00:00Z' });
    expect(v1.pinned).toBe(false);
    expect(v1.archived).toBeNull();
  });

  it('reads the new fields as the door sends them', () => {
    const n = noteOf({ id: 'n', rev: 1, title: 't', pinned: true, archived: '2026-10-22 10:00:00+00' });
    expect(n.pinned).toBe(true);
    expect(n.archived).toBe('2026-10-22 10:00:00+00');
  });

  it('puts pinned notes first, each group newest first, and keeps the archive apart', () => {
    const notes = [note(1), note(2, { pinned: true }), note(3), note(4, { archived: '2026-10-22T11:00:00Z' }), note(5, { pinned: true })];
    expect(arrange(notes, 'notes').map((n) => n.id)).toEqual(['note_5', 'note_2', 'note_3', 'note_1']);
    expect(arrange(notes, 'archive').map((n) => n.id)).toEqual(['note_4']);
    // A guest sees the archived ones last, labelled, rather than not at all.
    expect(guestOrder(notes).map((n) => n.id)).toEqual(['note_5', 'note_2', 'note_3', 'note_1', 'note_4']);
  });
});

describe('what a share lists', () => {
  it('sends the reader to this page, under its own path', () => {
    expect(pageAddress({ origin: 'https://notes.example', pathname: '/v2/' })).toBe('https://notes.example/v2/');
  });

  it('lists the notes, the files and the row read that opens a file', () => {
    expect(shareParams(note(1)).ops).toBe('allNotes,myFiles,std.attachments.get');
    expect(shareLinkParams(note(1)).ops).toBe('allNotes,myFiles,std.attachments.get');
  });

  it('reads only what the share holds, so a v1 share is never read past what it lists', () => {
    expect(readableOf(['myNotes'])).toEqual({ op: 'myNotes', files: false });
    expect(readableOf(['allNotes', 'myFiles', 'std.attachments.get'])).toEqual({ op: 'allNotes', files: false });
    expect(readableOf(['allNotes', 'myFiles', 'std.attachments.get', 'std.$blob.get'])).toEqual({ op: 'allNotes', files: true });
  });
});

describe('a file on a note', () => {
  it('is a row first: name, media type, size and stamp', () => {
    const p = attachParams('note_1', { name: 'photo.png', type: 'image/png', size: 42 });
    expect(p).toMatchObject({ note: 'note_1', name: 'photo.png', kind: 'image/png', size: 42 });
    expect(p.created).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/);
    expect(attachParams('n', { name: '', type: '', size: 0 })).toMatchObject({ name: 'file', kind: 'application/octet-stream' });
  });

  it('takes the new row id from the write summary', () => {
    expect(createdId([{ entity: 'attachments', created: 'attachment_1' }])).toBe('attachment_1');
    expect(() => createdId([{ entity: 'notes', created: 'note_1' }])).toThrow(/id/);
    expect(() => createdId([])).toThrow(/id/);
  });

  it('has no handle until its bytes arrive, and is shown when it is an image', () => {
    const a = attachmentOf({ id: 'a', rev: 1, note: 'n', name: 'x.png', kind: 'image/png', size: 1, created: 'c', file: null });
    expect(a.file).toBeNull();
    expect(isImage(a)).toBe(true);
    expect(isImage({ kind: 'image/svg+xml' })).toBe(false);
    expect(isImage({ kind: 'application/pdf' })).toBe(false);
  });

  it('lists a note’s files oldest first', () => {
    const f = (id: string, note: string, created: string): Attachment =>
      ({ id, rev: 1, note, name: id, kind: '', size: 0, created, file: 'h' });
    expect(filesOf([f('b', 'n', '2'), f('x', 'm', '0'), f('a', 'n', '1')], 'n').map((a) => a.id)).toEqual(['a', 'b']);
  });
});

describe('the download', () => {
  it('is saved under the date', () => {
    expect(exportName(new Date('2026-10-22T12:00:00Z'))).toBe('notes-2026-10-22.zip');
  });
});
