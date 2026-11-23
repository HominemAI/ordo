import { describe, expect, it } from 'vitest';

import { entity, external, key, reader, ref, synth, t } from '../src/index.js';

const notes = entity('notes', { title: t.text() });
const notesAccess = entity('notesAccess', { note: ref('notes') });

describe('reader', () => {
  it('prints the reader line with its tables', () => {
    expect(synth(reader('note_viewer', [notes, 'notesAccess']), notes, notesAccess)).toBe(
      [
        'reader note_viewer reads notes, notesAccess',
        '',
        'entity notes {',
        '  title: text',
        '}',
        '',
        'entity notesAccess {',
        '  note -> notes',
        '}',
        '',
      ].join('\n'),
    );
  });

  it('may name the platform members table', () => {
    expect(synth(reader('viewer', ['notes', 'members']), notes)).toContain(
      'reader viewer reads notes, members',
    );
  });

  it('refuses a table that is not an entity of the file', () => {
    expect(() => synth(reader('viewer', ['notez']), notes)).toThrow(/reader_unknown_table.*notez/);
    const customers = external(
      'customers',
      { id: key.int('customer_id') },
      { schema: 'crm', table: 'customers', shared: true },
    );
    expect(() => synth(reader('viewer', ['customers']), customers)).toThrow(/an external/);
  });

  it('checks the name and the list as the parser does', () => {
    expect(() => reader('noteViewer', ['notes'])).toThrow(/reader_line/);
    expect(() => reader('viewer', [])).toThrow(/names no table/);
    expect(() => reader('viewer', ['notes', notes])).toThrow(/twice/);
    expect(() => synth(reader('viewer', ['notes']), reader('viewer', ['notes']), notes)).toThrow(
      /passed twice/,
    );
    expect(reader('viewer_2', ['notes']).name).toBe('viewer_2');
  });
});
