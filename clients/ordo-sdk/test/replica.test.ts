/**
 * C3 of `plan-client-cache.md`: the device replica's store.
 *
 * Most of this runs on the in-memory backend, because everything that has to
 * understand a record lives above the backend. The last test runs the same
 * store on the IndexedDB backend (through `fake-indexeddb`), to show the real
 * adapter keeps records and the replica's keys across a reopen.
 */

import { IDBFactory, IDBKeyRange } from 'fake-indexeddb';
import { IndexedDbBackend, LocalReplica, MemoryBackend } from '../src/index.js';
import { digestOf } from '../src/replica/seal.js';
import { CountingBackend } from './counting.js';

const order = (n: number) => ({ id: `ord_${n}`, total: `${n}.00`, email: `buyer${n}@example.com` });

async function replica(backend = new MemoryBackend(), space = 'spc_a', principal = 'usr_1') {
  return { backend, r: await LocalReplica.open(backend, space, principal) };
}

describe('the device replica', () => {
  it('a row round-trips', async () => {
    const { r } = await replica();
    expect(await r.put({ entity: 'orders', id: 'ord_1', position: 3, row: order(1) })).toBe(true);
    expect(await r.get('orders', 'ord_1')).toEqual({
      entity: 'orders',
      id: 'ord_1',
      position: 3,
      row: order(1),
    });
    expect(await r.get('orders', 'ord_2')).toBeUndefined();
  });

  it('an older version never replaces a newer one', async () => {
    const { r } = await replica();
    await r.put({ entity: 'orders', id: 'ord_1', position: 5, row: order(5) });
    expect(await r.put({ entity: 'orders', id: 'ord_1', position: 3, row: order(3) })).toBe(false);
    expect((await r.get('orders', 'ord_1'))?.position).toBe(5);
    expect(await r.put({ entity: 'orders', id: 'ord_1', position: 5, row: order(6) })).toBe(true);
  });

  it('a removal holds its position, so an older version cannot bring the row back', async () => {
    const { r } = await replica();
    await r.put({ entity: 'orders', id: 'ord_1', position: 4, row: order(1) });
    expect(await r.remove('orders', 'ord_1', 6)).toBe(true);
    expect(await r.put({ entity: 'orders', id: 'ord_1', position: 5, row: order(1) })).toBe(false);
    expect(await r.get('orders', 'ord_1')).toBeUndefined();
    expect(await r.scan('orders')).toEqual([]);
    expect(await r.put({ entity: 'orders', id: 'ord_1', position: 7, row: order(7) })).toBe(true);
  });

  it('versions arriving together leave the newer one', async () => {
    const { r } = await replica();
    await Promise.all([
      r.put({ entity: 'orders', id: 'ord_1', position: 9, row: order(9) }),
      r.put({ entity: 'orders', id: 'ord_1', position: 8, row: order(8) }),
    ]);
    expect((await r.get('orders', 'ord_1'))?.position).toBe(9);
  });

  it('scans one entity, by id', async () => {
    const { r } = await replica();
    for (const n of [3, 1, 2]) {
      await r.put({ entity: 'orders', id: `ord_${n}`, position: n, row: order(n) });
    }
    await r.put({ entity: 'customers', id: 'cus_1', position: 1, row: { id: 'cus_1' } });
    expect((await r.scan('orders')).map((x) => x.id)).toEqual(['ord_1', 'ord_2', 'ord_3']);
  });

  it('two principals, and two spaces, never see each other rows', async () => {
    const backend = new MemoryBackend();
    const a = await LocalReplica.open(backend, 'spc_a', 'usr_1');
    const b = await LocalReplica.open(backend, 'spc_a', 'usr_2');
    const c = await LocalReplica.open(backend, 'spc_b', 'usr_1');
    await a.put({ entity: 'orders', id: 'ord_1', position: 1, row: order(1) });
    expect(await b.get('orders', 'ord_1')).toBeUndefined();
    expect(await c.get('orders', 'ord_1')).toBeUndefined();
    expect(await b.scan('orders')).toEqual([]);
  });

  it('a copied disk holds no row, id, entity, space or principal in the clear', async () => {
    const { backend, r } = await replica();
    await r.put({ entity: 'orders', id: 'ord_1', position: 1, row: order(1) });
    await r.put({ entity: 'orders', id: 'ord_2', position: 2, row: order(2) });
    const disk = [...backend.dump()]
      .map(([k, v]) => k + Buffer.from(v).toString('latin1'))
      .join('\n');
    for (const secret of ['buyer1@example.com', 'ord_1', 'ord_2', 'orders', 'spc_a', 'usr_1', '2.00']) {
      expect(disk.includes(secret), secret).toBe(false);
    }
  });

  it('purge removes one replica and its keys, and leaves the others', async () => {
    const backend = new MemoryBackend();
    const a = await LocalReplica.open(backend, 'spc_a', 'usr_1');
    const b = await LocalReplica.open(backend, 'spc_a', 'usr_2');
    await a.put({ entity: 'orders', id: 'ord_1', position: 1, row: order(1) });
    await b.put({ entity: 'orders', id: 'ord_1', position: 1, row: order(1) });
    await a.purge();
    expect(backend.dump().size).toBe(1);
    expect((await b.get('orders', 'ord_1'))?.position).toBe(1);
    const reopened = await LocalReplica.open(backend, 'spc_a', 'usr_1');
    expect(await reopened.get('orders', 'ord_1')).toBeUndefined();
  });

  it('on IndexedDB, records and the replica keys survive a reopen', async () => {
    const factory = new IDBFactory();
    const first = await IndexedDbBackend.open('ordo-replica-test', factory);
    const r1 = await LocalReplica.open(first, 'spc_a', 'usr_1');
    await r1.put({ entity: 'orders', id: 'ord_1', position: 2, row: order(1) });
    first.close();

    const second = await IndexedDbBackend.open('ordo-replica-test', factory);
    const r2 = await LocalReplica.open(second, 'spc_a', 'usr_1');
    expect(await r2.get('orders', 'ord_1')).toEqual({
      entity: 'orders',
      id: 'ord_1',
      position: 2,
      row: order(1),
    });
    await r2.purge();
    expect(await second.keys('')).toEqual([]);
    second.close();
  });
});

describe('revocation poisons the replica (C7)', () => {
  it('purges, and then serves nothing — reads answer empty rather than miss', async () => {
    const { r } = await replica();
    await r.put({ entity: 'orders', id: 'ord_1', position: 3, row: { id: 'ord_1', total: 10 } });
    await r.hold('std.orders.list', 'orders', 'ord_1');
    expect(await r.scan('orders')).toHaveLength(1);

    await r.poison();

    expect(r.poisoned).toBe(true);
    // All three read paths, because `refineOp` reads through `holdings` and
    // `get`, and `syncOp` through `holdings`.
    expect(await r.scan('orders')).toEqual([]);
    expect(await r.get('orders', 'ord_1')).toBeUndefined();
    expect(await r.holdings('std.orders.list')).toEqual([]);
  });

  it('stays poisoned even though purge deleted the keys under it', async () => {
    // `purge` deletes the records AND the keys, so a purged replica is dead
    // rather than empty. Without the latch a later write would seal under
    // keys still held in memory and a read would answer from them, which is
    // the case this pins.
    const { backend, r } = await replica();
    await r.poison();
    // The write is refused, not merely invisible: this returned true and put
    // a sealed record on disk until 2026-09-16.
    expect(await r.put({ entity: 'orders', id: 'ord_2', position: 9, row: { id: 'ord_2', total: 1 } })).toBe(false);
    expect(await r.get('orders', 'ord_2')).toBeUndefined();
    expect(await r.scan('orders')).toEqual([]);
    // And nothing survives in the raw store. Asserting the store is EMPTY,
    // not that a particular id is absent: record names are HMACs, so an
    // id-substring check would pass even if purge had done nothing.
    expect(backend.dump().size).toBe(0);
  });

  it('a sign-out purge empties the store without latching it', async () => {
    // Sign-out and revocation are different events: one is the person
    // leaving, the other is access being taken away.
    const { r } = await replica();
    await r.put({ entity: 'orders', id: 'ord_3', position: 1, row: { id: 'ord_3', total: 2 } });
    await r.purge();
    expect(r.poisoned).toBe(false);
    expect(await r.scan('orders')).toEqual([]);
  });
});

describe('key ranges: a scan reads its own records only (H3, M2)', () => {
  const ns = (space: string, principal: string) => digestOf(['ordo-replica', space, principal]);

  async function twoOnOneDatabase() {
    const backend = await IndexedDbBackend.open('ordo-range-test', new IDBFactory(), IDBKeyRange);
    const a = await LocalReplica.open(backend, 'spc_a', 'usr_1');
    const b = await LocalReplica.open(backend, 'spc_a', 'usr_2');
    for (const r of [a, b]) {
      for (let n = 1; n <= 5; n++) {
        await r.put({ entity: 'orders', id: `ord_${n}`, position: n, row: order(n) });
        await r.hold('std.orders.list', 'orders', `ord_${n}`);
      }
    }
    return { backend, a, b };
  }

  it('on IndexedDB, the keys of one replica are its own, in order', async () => {
    const { backend, a, b } = await twoOnOneDatabase();
    const mine = await backend.keys(`${await ns('spc_a', 'usr_1')}/`);
    // Five rows, five holders lists, five holds, nothing of the other replica.
    expect(mine).toHaveLength(15);
    expect([...mine].sort()).toEqual(mine);
    const theirs = await backend.keys(`${await ns('spc_a', 'usr_2')}/`);
    expect(theirs.filter((k) => mine.includes(k))).toEqual([]);
    expect((await a.scan('orders')).map((r) => r.id)).toEqual(['ord_1', 'ord_2', 'ord_3', 'ord_4', 'ord_5']);
    expect(await b.holdings('std.orders.list')).toHaveLength(5);
    backend.close();
  });

  it('on IndexedDB, purging one replica leaves the other whole', async () => {
    const { backend, a, b } = await twoOnOneDatabase();
    await a.purge();
    expect(await backend.keys(`${await ns('spc_a', 'usr_1')}/`)).toEqual([]);
    expect(await backend.keys(`${await ns('spc_a', 'usr_2')}/`)).toHaveLength(15);
    expect(await b.scan('orders')).toHaveLength(5);
    expect(await b.holdings('std.orders.list')).toHaveLength(5);
    backend.close();
  });

  it('holdings of n rows is one read, and a purge is one write', async () => {
    const backend = new CountingBackend();
    const r = await LocalReplica.open(backend, 'spc_a', 'usr_1');
    for (let n = 1; n <= 30; n++) {
      await r.put({ entity: 'orders', id: `ord_${n}`, position: n, row: order(n) });
      await r.hold('std.orders.list', 'orders', `ord_${n}`);
    }
    backend.reset();
    expect(await r.holdings('std.orders.list')).toHaveLength(30);
    expect(backend.reads).toBe(1);
    backend.reset();
    expect(await r.scan('orders')).toHaveLength(30);
    expect(backend.reads).toBe(1);
    backend.reset();
    await r.purge();
    expect(backend.writes).toBe(1);
    expect(backend.dump().size).toBe(0);
  });

  it('every record key is lowercase hex and `/`, so a prefix range is exact', async () => {
    const { r, backend } = await replica();
    await r.put({ entity: 'orders', id: 'ord_\uffff', position: 1, row: order(1) });
    await r.hold('std.orders.list?x=\u00ff', 'orders', 'ord_\uffff');
    await r.rememberResult('std.orders.list?x=\u00ff', 'v1', 'hash');
    expect(backend.dump().size).toBe(4);
    for (const key of backend.dump().keys()) expect(key).toMatch(/^[0-9a-f/]+$/);
  });
});

describe('writes are whole and reads do not wait behind them (H4)', () => {
  /** A backend whose next write waits until it is let go. */
  class HeldBackend extends MemoryBackend {
    gate: Promise<void> | null = null;
    open: (() => void) | null = null;
    hold(): void {
      this.gate = new Promise((resolve) => {
        this.open = resolve;
      });
    }
    override async write(puts: readonly (readonly [string, Uint8Array])[], deletes: readonly string[]): Promise<void> {
      if (this.gate) await this.gate;
      return super.write(puts, deletes);
    }
  }

  it('a read issued mid-load answers before the load ends', async () => {
    const backend = new HeldBackend();
    const r = await LocalReplica.open(backend, 'spc_a', 'usr_1');
    await r.put({ entity: 'orders', id: 'ord_1', position: 1, row: order(1) });
    backend.hold();
    const frame = r.applyFrame('std.orders.list', {
      position: 2,
      upserts: [2, 3, 4].map((n) => ({ entity: 'orders', id: `ord_${n}`, row: order(n) })),
    });
    let landed = false;
    void frame.then(() => {
      landed = true;
    });
    // The frame's write is stuck; a read answers anyway, and sees none of it.
    expect((await r.get('orders', 'ord_1'))?.position).toBe(1);
    expect(await r.get('orders', 'ord_2')).toBeUndefined();
    expect(await r.scan('orders')).toHaveLength(1);
    expect(landed).toBe(false);
    backend.open!();
    await frame;
    expect(await r.scan('orders')).toHaveLength(4);
    expect(await r.holdings('std.orders.list')).toHaveLength(3);
  });

  it('a lower-position frame does not overwrite a row in one batch', async () => {
    const backend = new CountingBackend();
    const r = await LocalReplica.open(backend, 'spc_a', 'usr_1');
    await r.applyFrame('a', { position: 5, upserts: [{ entity: 'orders', id: 'ord_1', row: order(5) }] });
    backend.reset();
    const delta = await r.applyFrame('a', {
      position: 3,
      upserts: [
        { entity: 'orders', id: 'ord_1', row: order(3) },
        { entity: 'orders', id: 'ord_2', row: order(2) },
      ],
    });
    expect(backend.writes).toBe(1);
    expect(delta.upserts).toEqual([
      { entity: 'orders', id: 'ord_1', position: 5, row: order(5) },
      { entity: 'orders', id: 'ord_2', position: 3, row: order(2) },
    ]);
    expect((await r.get('orders', 'ord_1'))?.row).toEqual(order(5));
    expect((await r.get('orders', 'ord_2'))?.position).toBe(3);
  });

  it('a release in a frame removes a row only when no other holder keeps it', async () => {
    const { r } = await replica();
    await r.applyFrame('a', { position: 1, upserts: [1, 2].map((n) => ({ entity: 'orders', id: `ord_${n}`, row: order(n) })) });
    await r.applyFrame('b', { position: 1, upserts: [{ entity: 'orders', id: 'ord_1', row: order(1) }] });
    const delta = await r.applyFrame('a', {
      position: 2,
      releases: [
        { entity: 'orders', id: 'ord_1' },
        { entity: 'orders', id: 'ord_2' },
      ],
    });
    expect(delta.releases).toEqual([
      { entity: 'orders', id: 'ord_1', removed: false },
      { entity: 'orders', id: 'ord_2', removed: true },
    ]);
    expect((await r.scan('orders')).map((x) => x.id)).toEqual(['ord_1']);
    expect(await r.holdings('a')).toEqual([]);
    expect(await r.holdings('b')).toHaveLength(1);
    // The removal holds its position: an older version cannot bring it back.
    expect(await r.put({ entity: 'orders', id: 'ord_2', position: 1, row: order(2) })).toBe(false);
  });

  it('a poisoned replica applies no frame', async () => {
    const { r, backend } = await replica();
    await r.poison();
    const delta = await r.applyFrame('a', { position: 1, upserts: [{ entity: 'orders', id: 'ord_1', row: order(1) }] });
    expect(delta).toEqual({ upserts: [], releases: [] });
    expect(backend.dump().size).toBe(0);
  });

  it('a read in flight when the replica is poisoned answers nothing', async () => {
    const { r } = await replica();
    await r.put({ entity: 'orders', id: 'ord_1', position: 1, row: order(1) });
    const reading = r.get('orders', 'ord_1');
    const scanning = r.scan('orders');
    const poisoned = r.poison();
    expect(await reading).toBeUndefined();
    expect(await scanning).toEqual([]);
    await poisoned;
  });
});
