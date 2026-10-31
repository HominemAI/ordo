/**
 * A screen with no `FilesProvider` renders no file section and reads no
 * files (so it needs no client either). The bytes themselves are the SDK's
 * and the hooks' to test.
 */

import { createElement } from 'react';
import { create } from 'react-test-renderer';
import { describe, expect, it } from 'vitest';
import { NoteFiles, fileError } from '../src/components/Files.js';
import { OrdoRefusal } from '@hominemai/ordo-react';

describe('the files marker', () => {
  it('renders nothing without it, and reads nothing', () => {
    const r = create(createElement(NoteFiles, { note: 'note_1', editable: true }));
    expect(r.toJSON()).toBeNull();
  });

  it('says a failed file call in words', () => {
    expect(fileError(new Error('Failed to fetch'))).toBe('Failed to fetch');
    const refused = new OrdoRefusal({ class: 'not_found', message: 'no', op: 'std.$blob.get', param: null, status: 404, declared: false } as never);
    expect(fileError(refused).length).toBeGreaterThan(0);
  });
});
