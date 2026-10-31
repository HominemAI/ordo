/**
 * Search, with no React and no network.
 *
 * Search runs on the device copy: the rows `allNotes` (or `myNotes` for a
 * share made under v1) holds for this person's own tenant and for each share.
 * Each copy holds only what the door's row guard served to it, so a search
 * can never reach a note a share does not grant. It runs on every key and
 * with the network off.
 *
 * A note matches when every word typed is found in its title, its body or
 * one of its tags. Case and accents are ignored, and a word matches inside a
 * longer one, so `plan` finds `planning`.
 */

import { arrange, type Note } from './notes.js';

/** Lower case, with accents taken off, so `Café` and `cafe` match. */
export function fold(text: string): string {
  return text.normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase();
}

/** The words to look for. No words means no search. At most 20, each once. */
export function searchWords(query: string): string[] {
  const out: string[] = [];
  for (const w of fold(query).split(/\s+/)) {
    if (w === '' || out.includes(w)) continue;
    out.push(w);
    if (out.length === 20) break;
  }
  return out;
}

/** Whether every word is in the title, the body or a tag. */
export function matches(note: Note, words: readonly string[]): boolean {
  if (words.length === 0) return false;
  const hay = [note.title, note.body, ...note.tags].map(fold);
  return words.every((w) => hay.some((h) => h.includes(w)));
}

/** The notes that match, pinned first, then newest, archived ones last. */
export function search(notes: readonly Note[], query: string): Note[] {
  const words = searchWords(query);
  if (words.length === 0) return [];
  const found = notes.filter((n) => matches(n, words));
  return [...arrange(found, 'notes'), ...arrange(found, 'archive')];
}

/** What the page says about how many matched. */
export function countText(n: number, where: string): string {
  if (n === 0) return `No note ${where} matches.`;
  return `${n} ${n === 1 ? 'note' : 'notes'} ${where} ${n === 1 ? 'matches' : 'match'}.`;
}
