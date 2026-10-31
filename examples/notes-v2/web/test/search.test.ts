/**
 * Search over the notes a device holds: every word typed, in the title, the
 * body or a tag, ignoring case and accents.
 */

import { describe, expect, it } from 'vitest';
import type { Note } from '../src/notes.js';
import { countText, fold, matches, search, searchWords } from '../src/search.js';

const note = (over: Partial<Note>): Note => ({
  id: 'n',
  rev: 1,
  title: '',
  body: '',
  tags: [],
  created: '2026-10-01T00:00:00Z',
  pinned: false,
  archived: null,
  ...over,
});

describe('search', () => {
  const groceries = note({ id: 'g', title: 'Groceries', body: 'Milk and eggs', tags: ['home'], created: '2026-10-01T00:00:00Z' });
  const plan = note({ id: 'p', title: 'Planning', body: 'The café on Friday', tags: ['work'], created: '2026-10-03T00:00:00Z' });
  const old = note({ id: 'o', title: 'Old plan', archived: '2026-10-04T00:00:00Z', created: '2026-10-05T00:00:00Z' });
  const pinned = note({ id: 'x', title: 'Pinned plan', pinned: true, created: '2026-09-01T00:00:00Z' });

  it('finds a word in the title, the body or a tag, whatever its case', () => {
    expect(search([groceries, plan], 'MILK').map((n) => n.id)).toEqual(['g']);
    expect(search([groceries, plan], 'groceries').map((n) => n.id)).toEqual(['g']);
    expect(search([groceries, plan], 'work').map((n) => n.id)).toEqual(['p']);
  });

  it('wants every word, each anywhere in the note', () => {
    expect(search([groceries, plan], 'milk home').map((n) => n.id)).toEqual(['g']);
    expect(search([groceries, plan], 'milk work')).toEqual([]);
  });

  it('matches inside a longer word and ignores accents', () => {
    expect(matches(plan, ['plan'])).toBe(true);
    expect(matches(plan, ['cafe'])).toBe(true);
    expect(fold('Café')).toBe('cafe');
  });

  it('finds nothing for nothing typed', () => {
    expect(searchWords('   ')).toEqual([]);
    expect(search([groceries], '  ')).toEqual([]);
  });

  it('shows pinned first, then newest, archived last', () => {
    expect(search([plan, old, pinned], 'plan').map((n) => n.id)).toEqual(['x', 'p', 'o']);
  });

  it('keeps each word once and at most 20', () => {
    expect(searchWords('a A a b')).toEqual(['a', 'b']);
    expect(searchWords(Array.from({ length: 30 }, (_, i) => `w${i}`).join(' '))).toHaveLength(20);
  });

  it('says how many matched', () => {
    expect(countText(0, 'here')).toBe('No note here matches.');
    expect(countText(1, 'here')).toBe('1 note here matches.');
    expect(countText(3, 'here')).toBe('3 notes here match.');
  });
});
