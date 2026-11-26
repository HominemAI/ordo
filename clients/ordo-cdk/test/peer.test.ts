import { describe, expect, it } from 'vitest';

import { external, g, key, peer, synth, t } from '../src/index.js';

const entries = (journal = peer('journal', ['std.notes.list'])) =>
  external(
    'entries',
    {
      id: key.text('id'),
      title: t.text().column('title'),
      owner: t.text().column('owner'),
    },
    { peer: journal, table: 'notes', shared: true },
  ).accessRead(g.walk(g.any(g.all(g.eq('owner')))));

describe('peer', () => {
  it('prints the peer block and reads the table through its nickname', () => {
    const journal = peer('journal', ['std.notes.list']);
    expect(synth(journal, entries(journal))).toBe(
      [
        'peer journal {',
        '  std.notes.list',
        '}',
        '',
        'external entries from journal.notes shared {',
        '  id: text <- "id"',
        '  title: text <- "title"',
        '  owner: text <- "owner"',
        '  access read owner == $caller',
        '}',
        '',
      ].join('\n'),
    );
  });

  it('refuses an external whose peer is not passed to synth first', () => {
    const journal = peer('journal', ['std.notes.list']);
    expect(() => synth(entries(journal))).toThrow(/peer_undeclared/);
    expect(() => synth(entries(journal), journal)).toThrow(/peer_undeclared/);
  });

  it('refuses a peer passed twice', () => {
    const journal = peer('journal', ['std.notes.list']);
    expect(() => synth(journal, journal, entries(journal))).toThrow(/peer_name/);
  });

  it('takes lowercase letters and digits only, as the parser does', () => {
    expect(() => peer('notesV2', ['a'])).toThrow(/ORDO_PEER_<NAME>_URL/);
    expect(() => peer('my_peer', ['a'])).toThrow(/peer_name/);
    expect(peer('notes2', ['a']).name).toBe('notes2');
  });

  it('takes a schema or a peer, never both', () => {
    expect(() =>
      external(
        'x',
        { id: key.int('i') },
        // @ts-expect-error a schema and a peer at once
        { schema: 's', peer: peer('journal', ['a']), table: 't', shared: true },
      ),
    ).toThrow(/not both/);
    expect(() =>
      // @ts-expect-error neither a schema nor a peer
      external('x', { id: key.int('i') }, { table: 't', shared: true }),
    ).toThrow(/schema/);
  });

  it('prints a block of the holder ops it calls, read by no external', () => {
    expect(synth(peer('notes', ['std.notes.list', 'addComment']))).toBe(
      ['peer notes {', '  std.notes.list', '  addComment', '}', ''].join('\n'),
    );
  });

  // G8: the one-line form was the shared-database path, removed.
  it('refuses a peer with no ops, an empty block, a pattern and an op named twice', () => {
    // @ts-expect-error the ops are required
    expect(() => peer('notes')).toThrow(/names no op/);
    expect(() => peer('notes', [])).toThrow(/peer_block/);
    expect(() => peer('notes', ['/add.*/'])).toThrow(/peer_block/);
    expect(() => peer('notes', ['addComment', 'addComment'])).toThrow(/twice/);
  });
});
