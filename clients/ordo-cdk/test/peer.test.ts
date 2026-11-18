import { describe, expect, it } from 'vitest';

import { external, g, key, peer, synth, t } from '../src/index.js';

const entries = (journal = peer('journal')) =>
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
  it('prints the peer line and reads the table through its nickname', () => {
    const journal = peer('journal');
    expect(synth(journal, entries(journal))).toBe(
      [
        'peer journal',
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
    const journal = peer('journal');
    expect(() => synth(entries(journal))).toThrow(/peer_undeclared/);
    expect(() => synth(entries(journal), journal)).toThrow(/peer_undeclared/);
  });

  it('refuses a peer no external reads, and a peer passed twice', () => {
    const journal = peer('journal');
    expect(() => synth(journal)).toThrow(/peer_unused/);
    expect(() => synth(journal, journal, entries(journal))).toThrow(/peer_name/);
  });

  it('takes lowercase letters and digits only, as the parser does', () => {
    expect(() => peer('notesV2')).toThrow(/peer_name/);
    expect(() => peer('my_peer')).toThrow(/peer_name/);
    expect(peer('notes2').name).toBe('notes2');
  });

  it('takes a schema or a peer, never both', () => {
    expect(() =>
      external(
        'x',
        { id: key.int('i') },
        // @ts-expect-error a schema and a peer at once
        { schema: 's', peer: peer('journal'), table: 't', shared: true },
      ),
    ).toThrow(/not both/);
    expect(() =>
      // @ts-expect-error neither a schema nor a peer
      external('x', { id: key.int('i') }, { table: 't', shared: true }),
    ).toThrow(/schema/);
  });
});
