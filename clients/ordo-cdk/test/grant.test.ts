import { describe, expect, it } from 'vitest';

import { entity, grant, grantProvider, synth, t } from '../src/index.js';

const notes = entity('notes', { title: t.text() });

describe('grant', () => {
  it('prints each header and its lines in order', () => {
    expect(
      synth(
        grant('note_viewer', ['std.notes.get', '/add.*/', '!addAdmin']),
        grant('billing', ['markPaid'], { asItself: true }),
        grantProvider('hominem', ['std.notes.get']),
        notes,
      ),
    ).toBe(
      [
        'grant note_viewer {',
        '  std.notes.get',
        '  /add.*/',
        '  !addAdmin',
        '}',
        '',
        'grant billing as itself {',
        '  markPaid',
        '}',
        '',
        'grant provider hominem {',
        '  std.notes.get',
        '}',
        '',
        'entity notes {',
        '  title: text',
        '}',
        '',
      ].join('\n'),
    );
  });

  it('takes every op with *', () => {
    expect(synth(grant('all', ['*', '!/std\\..*/']), notes)).toContain(
      'grant all {\n  *\n  !/std\\..*/\n}',
    );
  });

  it('checks the name, the lines and duplicates as the parser does', () => {
    expect(() => grant('noteViewer', ['a'])).toThrow(/grant_block.*ORDO_CONSUMER_<NAME>_CLIENT/);
    expect(() => grant('v', [])).toThrow(/grants nothing/);
    expect(() => grant('v', ['two words'])).toThrow(/is not a line/);
    expect(() => synth(grant('v', ['a']), grant('v', ['b']), notes)).toThrow(/passed twice/);
    // A grant and a provider grant may share a name.
    expect(() => synth(grant('v', ['a']), grantProvider('v', ['b']), notes)).not.toThrow();
  });
});
