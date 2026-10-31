/**
 * Two tabs on one replica. A write reads, decides and writes in separate
 * transactions, so two tabs can each decide from the same old record. The
 * replica takes a Web Lock named for its namespace around each write.
 */

import { IDBFactory } from 'fake-indexeddb';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { IndexedDbBackend, LocalReplica } from '../src/index.js';

/** The part of `navigator.locks` the replica uses: one holder per name at a time. */
function stubLocks() {
  const tails = new Map<string, Promise<unknown>>();
  const names: string[] = [];
  const request = <T>(name: string, callback: () => Promise<T>): Promise<T> => {
    names.push(name);
    const run = (tails.get(name) ?? Promise.resolve()).then(callback, callback);
    tails.set(name, run.catch(() => undefined));
    return run;
  };
  return { names, locks: { request } };
}

async function twoTabs() {
  const factory = new IDBFactory();
  const a = await LocalReplica.open(await IndexedDbBackend.open('tabs', factory), 'spc_a', 'usr_1');
  const b = await LocalReplica.open(await IndexedDbBackend.open('tabs', factory), 'spc_a', 'usr_1');
  return { a, b };
}

const frame = { position: 1, upserts: [{ entity: 'notes', id: 'n1', row: { id: 'n1' } }] };

afterEach(() => vi.unstubAllGlobals());

describe('two tabs on one replica', () => {
  // Whichever op the unlocked race happened to lose, one of the two orders
  // shows it: the row's record of holders names one op, so releasing that op
  // first removes a row the other still holds.
  it.each([['opA', 'opB'], ['opB', 'opA']] as const)(
    'with locks, both holders are kept: releasing %s first keeps the row for %s',
    async (first, second) => {
      const { locks, names } = stubLocks();
      vi.stubGlobal('navigator', { locks });
      const { a, b } = await twoTabs();
      await Promise.all([a.applyFrame('opA', frame), b.applyFrame('opB', frame)]);
      expect(await a.release(first, 'notes', 'n1', 2)).toBe(false);
      expect((await b.get('notes', 'n1'))?.row).toEqual({ id: 'n1' });
      expect(await b.release(second, 'notes', 'n1', 3)).toBe(true);
      expect(await a.get('notes', 'n1')).toBeUndefined();
      expect(new Set(names).size).toBe(1);
      expect(names[0]).toMatch(/^ordo-replica:/);
    },
  );

  it('without navigator.locks a write still runs', async () => {
    vi.stubGlobal('navigator', {});
    const { a } = await twoTabs();
    expect(await a.put({ entity: 'notes', id: 'n2', position: 1, row: { id: 'n2' } })).toBe(true);
  });
});
