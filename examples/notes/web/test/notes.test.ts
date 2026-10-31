/**
 * What the page sends for a note, and what it does when the door answers.
 *
 * Writes go through `@hominemai/ordo-react`'s commit path, which is what
 * `useCommit` runs, so the wire is checked here with no render. The editor's
 * own behaviour over it is in `editor.test.tsx`.
 */

import { commitOnce, retryAfterConflict } from '@hominemai/ordo-react';
import { describe, expect, it } from 'vitest';
import { linkFor, linkOfShare, newestFirst, noteOf, parseTags, readersIn, shareParams, stamp, type Note } from '../src/notes.js';
import { door, hooksOver, refusal } from './support.js';

const note = (n: number, created = `2026-09-29T10:00:0${n}Z`): Note => ({
  id: `note_w${n}_${n}`,
  rev: n,
  title: `t${n}`,
  body: `b${n}`,
  tags: [`g${n}`],
  created,
});

const row = (n: Note) => ({ id: n.id, rev: n.rev, title: n.title, body: n.body, tags: n.tags, created: n.created });

describe('a note', () => {
  it('is stamped by the page, to the second, in the form the door binds', () => {
    expect(stamp(new Date('2026-09-29T12:34:56.789Z'))).toBe('2026-09-29T12:34:56Z');
  });

  it('is created with its title, body and stamp', async () => {
    const d = door({ json: { rows: [] } });
    const out = await commitOnce(hooksOver(d), 'createNote', {
      values: { title: 'Groceries', body: 'milk', tags: ['food'], created: stamp(new Date('2026-09-29T12:34:56Z')) },
    });
    expect(out.status).toBe('committed');
    const sent = d.sent[0];
    expect(sent?.method).toBe('POST');
    expect(sent?.url).toBe('https://door.example/op/createNote');
    expect(sent?.body).toEqual({ title: 'Groceries', body: 'milk', tags: ['food'], created: '2026-09-29T12:34:56Z' });
    expect(sent?.headers['X-Ordo-Space']).toBe('spc_ada');
    expect(sent?.headers['Idempotency-Key']).toBe('key');
  });

  it('is listed as the door sent it, with its revision', async () => {
    const d = door({ json: { rows: [row(note(1)), row(note(2))] } });
    const res = await hooksOver(d).fetchOp('myNotes');
    expect(res.rows.map((r) => noteOf({ ...r })).map((n) => [n.id, n.rev])).toEqual([
      ['note_w1_1', 1],
      ['note_w2_2', 2],
    ]);
  });

  it('is read from a row the way the list draws it', () => {
    expect(noteOf({ ...row(note(1)), body: undefined })).toEqual({ ...note(1), body: '' });
  });

  it('reads only the string tags of an array, and nothing from anything else', () => {
    expect(noteOf({ ...row(note(1)), tags: ['a', 3, null, 'b'] }).tags).toEqual(['a', 'b']);
    expect(noteOf({ ...row(note(1)), tags: 'a' }).tags).toEqual([]);
    expect(noteOf({ ...row(note(1)), tags: undefined }).tags).toEqual([]);
  });

  it('is shown newest first, and ties keep the door order', () => {
    const a = note(1, '2026-09-29T10:00:00Z');
    const b = note(2, '2026-09-29T10:00:00Z');
    const c = note(3, '2026-09-29T11:00:00Z');
    expect(newestFirst([a, b, c]).map((n) => n.id)).toEqual([c.id, a.id, b.id]);
  });
});

describe('saving', () => {
  const edit = { id: 'note_w3_3', title: 'New title', body: 'New body', tags: ['a'] };

  it('asserts the revision the person opened', async () => {
    const d = door({ json: { rows: [] } });
    await commitOnce(hooksOver(d), 'editNote', { values: edit, rev: 3 });
    expect(d.sent[0]?.body).toEqual({ id: 'note_w3_3', rev: 3, title: 'New title', body: 'New body', tags: ['a'] });
  });

  it('is refused here, before the wire, when no revision is given', async () => {
    const d = door({ json: { rows: [] } });
    const out = await commitOnce(hooksOver(d), 'editNote', { values: edit });
    expect(out).toMatchObject({ status: 'refused', refusal: { class: 'missing_param' } });
    expect(d.sent).toEqual([]);
  });

  it('refuses to overwrite when the note changed elsewhere, and hands back what is stored', async () => {
    const stored = { ...note(3), rev: 4, body: 'changed in the other tab' };
    const d = door(refusal(409, 'cas_miss'), { json: { rows: [row(stored)] } });
    const out = await commitOnce(hooksOver(d), 'editNote', { values: edit, rev: 3 });
    expect(out.status).toBe('conflict');
    if (out.status !== 'conflict') return;
    expect(noteOf({ ...((await out.conflict.refetch()) ?? {}) })).toEqual(stored);
    // One write, refused, then a read. Nothing was written over it.
    expect(d.sent.map((s) => s.method)).toEqual(['POST', 'GET']);
  });

  it('lands when retried with the revision that is stored now', async () => {
    const d = door(refusal(409, 'cas_miss'), { json: { rows: [] } });
    const client = hooksOver(d);
    const out = await commitOnce(client, 'editNote', { values: edit, rev: 3 });
    if (out.status !== 'conflict') throw new Error('expected a conflict');
    const retried = await retryAfterConflict(client, 'editNote', out.conflict, { values: edit, rev: 4 });
    expect(retried.status).toBe('committed');
    expect(d.sent[1]?.body).toMatchObject({ rev: 4 });
  });

  it('will not retry the losing revision', async () => {
    const d = door(refusal(409, 'cas_miss'), { json: { rows: [] } });
    const client = hooksOver(d);
    const out = await commitOnce(client, 'editNote', { values: edit, rev: 3 });
    if (out.status !== 'conflict') throw new Error('expected a conflict');
    const retried = await retryAfterConflict(client, 'editNote', out.conflict, { values: edit, rev: 3 });
    expect(retried.status).toBe('refused');
    expect(d.sent).toHaveLength(1);
  });

  it('says a note was deleted elsewhere when it is not stored any more, apart from not being readable', async () => {
    const d = door(refusal(409, 'cas_miss'), { json: { rows: [] } });
    const out = await commitOnce(hooksOver(d), 'editNote', { values: edit, rev: 3 });
    if (out.status !== 'conflict') throw new Error('expected a conflict');
    expect(await out.conflict.lookup()).toEqual({ status: 'deleted' });
  });

  it('passes any other refusal on by its class', async () => {
    const d = door(refusal(403, 'access_denied'));
    const out = await commitOnce(hooksOver(d), 'editNote', { values: edit, rev: 3 });
    expect(out).toMatchObject({ status: 'refused', refusal: { class: 'access_denied' } });
  });
});

describe('deleting', () => {
  it('deletes the note by its id', async () => {
    const d = door({ json: { rows: [] } });
    await commitOnce(hooksOver(d), 'deleteNote', { values: { id: note(2).id } });
    expect(d.sent[0]?.url).toBe('https://door.example/op/deleteNote?id=note_w2_2');
  });
});

describe('sharing a note', () => {
  it('asks the platform for a read-only share of that one note for a year, and builds the link', async () => {
    const d = door({ json: { rows: [{ ticket: 'tkt_abc-123.x', outcome: 'issued' }] } });
    const out = await commitOnce(hooksOver(d), 'std.space.share', { values: shareParams(note(1)) });
    if (out.status !== 'committed') throw new Error(out.status);
    const sent = d.sent[0];
    expect(sent?.url).toBe('https://door.example/op/std.space.share');
    expect(sent?.method).toBe('POST');
    expect(sent?.body).toEqual({
      ops: 'myNotes',
      rows: JSON.stringify([{ entity: 'notes', id: 'note_w1_1', level: 'read' }]),
      expires: '365d',
      label: 't1',
    });
    const link = linkOfShare(out.response.rows[0], 'https://notes.inf.smg.icu/');
    expect(link.url).toBe('https://notes.inf.smg.icu/#/accept/tkt_abc-123.x');
    expect(linkFor('http://localhost:5173', 'T')).toBe('http://localhost:5173/#/accept/T');
  });

  it('says so when the door answers no ticket', () => {
    expect(() => linkOfShare({ outcome: 'issued' } as never, 'http://x')).toThrow('did not answer a share ticket');
  });

  it('lists the readers of one note and not the owner or another note', () => {
    const access = (id: string, n: string, member: string, level: string) => ({ id, note: n, member, level, rev: 1 });
    const rows = [
      access('a1', 'note_w1_1', 'usr_owner', 'admin'),
      access('a2', 'note_w1_1', 'mem_guest', 'read'),
      access('a3', 'note_w2_2', 'mem_other', 'read'),
    ];
    expect(readersIn(rows, note(1))).toEqual([{ member: 'mem_guest' }]);
  });

  it('takes one note back from one guest', async () => {
    const d = door({ json: { rows: [] } });
    await commitOnce(hooksOver(d), 'unshareNote', { values: { note: note(1).id, member: 'mem_guest' } });
    const sent = d.sent[0];
    expect(sent?.method).toBe('DELETE');
    expect(sent?.url).toBe('https://door.example/op/unshareNote?note=note_w1_1&member=mem_guest');
  });

  it('closes the whole window of one guest', async () => {
    const d = door({ json: { rows: [] } });
    await commitOnce(hooksOver(d), 'std.space.revokeShare', { values: { seat: 'mem_guest' } });
    expect(d.sent[0]?.url).toBe('https://door.example/op/std.space.revokeShare');
    expect(d.sent[0]?.body).toEqual({ seat: 'mem_guest' });
  });
});

describe('typed tags', () => {
  it('are split on commas, trimmed, lower cased, and kept once', () => {
    expect(parseTags(' Work, ideas ,,WORK,  ')).toEqual(['work', 'ideas']);
    expect(parseTags('')).toEqual([]);
  });

  it('drop a tag over 40 characters and stop at 20 tags', () => {
    expect(parseTags(`${'x'.repeat(41)}, ${'y'.repeat(40)}`)).toEqual(['y'.repeat(40)]);
    const many = Array.from({ length: 30 }, (_, i) => `t${i}`).join(',');
    expect(parseTags(many)).toHaveLength(20);
  });
});
