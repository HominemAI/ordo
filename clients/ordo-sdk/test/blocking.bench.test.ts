/**
 * Micro-benchmarks behind `docs/audit-client-blocking-2026-10-10.md`.
 *
 * They measure two things the audit claims:
 *
 * - how long one synchronous step holds the thread (a sort, a key scan), which
 *   is the time a page cannot respond to a tap;
 * - how long an async path takes end to end, and the longest stall it causes
 *   on the way (the "probe" below), which is how long a screen waits for rows.
 *
 * They are slow, so they run only with `ORDO_BENCH=1`:
 *
 *     ORDO_BENCH=1 npx vitest run test/blocking.bench.test.ts
 *
 * The default run executes one tiny case of each, so the code stays compiled
 * and correct without costing the suite any time.
 */

import { performance } from 'node:perf_hooks';
import { LocalReplica, MemoryBackend, OrdoClient, SyncManager, refineOp, refineRows, refineRowsAsync, syncOp } from '../src/index.js';
import type { OpSpec } from '../src/index.js';
import { IDBFactory, IDBKeyRange } from 'fake-indexeddb';
import { IndexedDbBackend } from '../src/index.js';
import { CountingBackend } from './counting.js';

const BENCH = process.env.ORDO_BENCH === '1';
const SIZES = BENCH ? [10_000, 100_000] : [200];
const ASYNC_SIZES = BENCH ? [1_000, 10_000] : [50];

const row = (n: number) => ({
  id: `task_${String(n).padStart(8, '0')}`,
  title: `task ${(n * 7919) % 100_003}`,
  total: `${(n * 37) % 10_000}.${String(n % 100).padStart(2, '0')}`,
  done: n % 3 === 0,
});

/** The longest gap between two turns of the event loop while `work` runs. */
async function probe<T>(work: () => Promise<T>): Promise<{ result: T; ms: number; stall: number }> {
  let stall = 0;
  let last = performance.now();
  let on = true;
  const tick = () => {
    const now = performance.now();
    stall = Math.max(stall, now - last);
    last = now;
    if (on) setImmediate(tick);
  };
  setImmediate(tick);
  const t0 = performance.now();
  const result = await work();
  const ms = performance.now() - t0;
  on = false;
  return { result, ms, stall };
}

function time(f: () => unknown): number {
  const t0 = performance.now();
  f();
  return performance.now() - t0;
}

const log = (line: string) => {
  if (BENCH) console.log(`[bench] ${line}`);
};

const tasks: OpSpec = {
  op: 'std.tasks.list',
  version: '0123456789abcdef',
  method: 'GET',
  kind: 'read',
  terminal: ['fetch', 'live', 'pages'],
  paged: true,
  singleRow: false,
  casParam: null,
  params: [],
  refusals: [],
  standingError: null,
  sync: { entity: 'tasks', key: 'id' },
  refine: { tier: 'none', fields: ['id', 'title', 'total', 'done'], decimals: ['total'] },
  order: { keys: [{ path: 'id', desc: false }], stated: true },
};

/** A door that answers a sync feed with `n` rows in load frames of `page`. */
function feedClient(n: number, page = 50): OrdoClient {
  const enc = new TextEncoder();
  const fetch: typeof globalThis.fetch = async () => {
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        for (let at = 0; at < n; at += page) {
          const rows = [];
          for (let i = at; i < Math.min(n, at + page); i++) rows.push(row(i));
          const loaded = Math.min(n, at + page);
          const data = { entity: 'tasks', key: 'id', position: 1, pin: null, rows, loaded, complete: loaded === n };
          c.enqueue(enc.encode(`event: load\ndata: ${JSON.stringify(data)}\n\n`));
        }
        if (n === 0) c.enqueue(enc.encode(`event: load\ndata: ${JSON.stringify({ entity: 'tasks', key: 'id', position: 1, pin: null, rows: [], loaded: 0, complete: true })}\n\n`));
        c.close();
      },
    });
    return new Response(body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
  };
  return new OrdoClient({ baseUrl: 'https://door.example/api', space: 'spc_test', token: 'tok', fetch });
}

describe('what holds the main thread (F-sort, F-keys)', () => {
  for (const n of SIZES) {
    const rows = Array.from({ length: n }, (_, i) => row(i));

    it(`refineRows over ${n} rows: one synchronous task each`, () => {
      const byText = time(() => refineRows(rows, { sort: [{ field: 'title' }] }, tasks.order));
      const byDecimal = time(() => refineRows(rows, { sort: [{ field: 'total' }] }, tasks.order, ['total']));
      const filter = time(() =>
        refineRows(rows, { where: [{ field: 'total', op: 'gt', value: '5000.00' }] }, tasks.order, ['total']),
      );
      log(`refineRows n=${n}: sort text ${byText.toFixed(0)} ms, sort decimal ${byDecimal.toFixed(0)} ms, filter decimal ${filter.toFixed(0)} ms`);
      expect(refineRows(rows.slice(0, 3), { sort: [{ field: 'total', direction: 'desc' }] }, tasks.order, ['total']).length).toBe(3);
    });

  }
});

describe('what makes a screen wait (F-apply, F-scan, F-refineOp, F-purge)', () => {
  for (const n of ASYNC_SIZES) {
    it(`a sync load of ${n} rows, then reading it back`, async () => {
      const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'usr_1');
      const load = await probe(() => syncOp(feedClient(n), replica, tasks));
      expect(load.result.complete).toBe(true);
      const scan = await probe(() => replica.scan('tasks'));
      expect(scan.result.length).toBe(n);
      const refine = await probe(() => refineOp(replica, tasks, { sort: [{ field: 'title' }] }));
      expect(refine.result.length).toBe(n);
      const purge = await probe(() => replica.purge());
      log(
        `n=${n}: load ${load.ms.toFixed(0)} ms (longest stall ${load.stall.toFixed(1)} ms), ` +
          `scan ${scan.ms.toFixed(0)} ms (stall ${scan.stall.toFixed(1)}), ` +
          `refineOp ${refine.ms.toFixed(0)} ms (stall ${refine.stall.toFixed(1)}), ` +
          `purge ${purge.ms.toFixed(0)} ms (stall ${purge.stall.toFixed(1)})`,
      );
    }, 600_000);
  }
});

describe('a mounted screen during a load (F-reread)', () => {
  // What `useDevice` does through ordo-react's `deviceCopy.read`: on every
  // frame the manager publishes, read every held row back and order it.
  for (const n of BENCH ? [1_000, 3_000] : [100]) {
    it(`a load of ${n} rows with one screen re-reading on every frame`, async () => {
      const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'usr_1');
      const reads: Promise<unknown>[] = [];
      const read = async () => {
        const held = [...(await replica.holdings(tasks.op))].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
        const rows = [];
        for (const { entity, id } of held) {
          const r = await replica.get(entity, id);
          if (r) rows.push(r.row);
        }
        return refineRows(rows, {}, tasks.order, ['total']);
      };
      const run = await probe(async () => {
        await syncOp(feedClient(n), replica, tasks, {}, { onFrame: () => void reads.push(read()) });
        await Promise.all(reads);
      });
      log(
        `n=${n}: load with a screen re-reading per frame ${run.ms.toFixed(0)} ms ` +
          `over ${reads.length} reads (stall ${run.stall.toFixed(1)} ms)`,
      );
      expect(reads.length).toBe(Math.ceil(n / 50));
    }, 600_000);
  }
});

/**
 * The budgets of `docs/plan-client-loading-fixes.md`, as assertions. Each
 * failed on the code the plan was written from, and names its target. They
 * are asserted only with `ORDO_BENCH=1`; the default run measures the tiny
 * case and checks it is correct.
 */
/** As {@link feedClient}, with the feed left open until the manager aborts it. */
function openFeedClient(n: number, page = 50): OrdoClient {
  const enc = new TextEncoder();
  const fetch: typeof globalThis.fetch = async (_input, init) => {
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        for (let at = 0; at < n; at += page) {
          const rows = [];
          for (let i = at; i < Math.min(n, at + page); i++) rows.push(row(i));
          const loaded = Math.min(n, at + page);
          const data = { entity: 'tasks', key: 'id', position: 1, pin: null, rows, loaded, complete: loaded === n };
          c.enqueue(enc.encode(`event: load\ndata: ${JSON.stringify(data)}\n\n`));
        }
        init?.signal?.addEventListener('abort', () => c.error(init.signal!.reason));
      },
    });
    return new Response(body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
  };
  return new OrdoClient({ baseUrl: 'https://door.example/api', space: 'spc_test', token: 'tok', fetch });
}

describe('the budgets (plan-client-loading-fixes.md)', () => {
  const LOAD = BENCH ? 10_000 : 200;

  it(`a sync load of ${LOAD} rows, in memory, within 900 ms per 10k (H4)`, async () => {
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'usr_1');
    const load = await probe(() => syncOp(feedClient(LOAD), replica, tasks));
    expect(load.result.complete).toBe(true);
    const budget = (900 * LOAD) / 10_000;
    log(`budget: load of ${LOAD} rows ${load.ms.toFixed(0)} ms (target ${budget.toFixed(0)} ms), stall ${load.stall.toFixed(1)} ms`);
    if (BENCH) expect(load.ms, `target: a ${LOAD}-row load in at most ${budget} ms`).toBeLessThanOrEqual(budget);
  }, 600_000);

  it('one write transaction per 50-row frame (H4)', async () => {
    const n = BENCH ? 2_000 : 200;
    const backend = new CountingBackend();
    const replica = await LocalReplica.open(backend, 'spc_test', 'usr_1');
    backend.reset();
    const frames = Math.ceil(n / 50);
    await syncOp(feedClient(n), replica, tasks);
    const perFrame = backend.writes / frames;
    log(`budget: ${backend.writes} write transactions over ${frames} frames, ${perFrame.toFixed(1)} per frame (target 1)`);
    // A counter, not a timing: asserted on every run since I3 met it.
    expect(perFrame, 'target: one write transaction per frame').toBeLessThanOrEqual(1);
  }, 600_000);

  it('a read issued in the middle of a load answers within 100 ms (H4)', async () => {
    // A screen's read of what the op holds so far (1,000 rows at frame 20),
    // issued while the rest of a 10k load is still being written.
    const n = BENCH ? 10_000 : 200;
    const at = BENCH ? 20 : 2;
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'usr_1');
    let asked: Promise<[number, number]> | null = null;
    let frames = 0;
    // The batched read the store offers (`held`: the holds in one read, the
    // rows in one more), and the row-at-a-time read a screen made before it.
    const batched = async () => (await replica.held(tasks.op)).length;
    const perRow = async () => {
      const rows = [];
      for (const { entity, id } of await replica.holdings(tasks.op)) rows.push(await replica.get(entity, id));
      return rows.length;
    };
    const timed = async (read: () => Promise<number>) => {
      const t0 = performance.now();
      expect(await read()).toBeGreaterThanOrEqual(at * 50);
      return performance.now() - t0;
    };
    await syncOp(feedClient(n), replica, tasks, {}, {
      onFrame: () => {
        frames += 1;
        if (frames !== at) return;
        asked = Promise.all([timed(batched), timed(perRow)]);
      },
    });
    const [ms, rowByRow] = await asked!;
    log(
      `budget: a read of ${at * 50} rows issued mid-load of ${n} answered in ${ms.toFixed(0)} ms ` +
        `(target 100 ms); row by row ${rowByRow.toFixed(0)} ms`,
    );
    if (BENCH) expect(ms, 'target: a read issued mid-load answers within 100 ms').toBeLessThanOrEqual(100);
  }, 600_000);

  it('a key scan does not grow with the other records on the device (H3)', async () => {
    // One replica's 3,000 keys (300 by default), timed alone and then beside 300k keys of other
    // replicas. The scan asks IndexedDB for its prefix's range, so no filter
    // and no sort over the others runs on the page.
    const others = BENCH ? 300_000 : 600;
    const count = BENCH ? 3_000 : 300;
    const factory = new IDBFactory();
    const backend = await IndexedDbBackend.open('ordo-bench-keys', factory, IDBKeyRange);
    const mine = 'a'.repeat(64);
    // Filled straight through IndexedDB, in one transaction per batch.
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const req = factory.open('ordo-bench-keys', 1);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
    const fill = async (ns: string, n: number) => {
      for (let at = 0; at < n; at += 20_000) {
        await new Promise<void>((resolve, reject) => {
          const tx = db.transaction('records', 'readwrite');
          const store = tx.objectStore('records');
          for (let i = at; i < Math.min(n, at + 20_000); i++) {
            store.put(new Uint8Array(1), `${ns}/${i.toString(16).padStart(64, '0')}`);
          }
          tx.oncomplete = () => resolve();
          tx.onerror = () => reject(tx.error);
        });
      }
    };
    await fill(mine, count);
    const scan = async () => {
      let best = Infinity;
      for (let i = 0; i < 5; i++) {
        const t0 = performance.now();
        expect(await backend.keys(`${mine}/`)).toHaveLength(count);
        best = Math.min(best, performance.now() - t0);
      }
      return best;
    };
    const alone = await scan();
    await fill('b'.repeat(64), others / 2);
    await fill('0'.repeat(64), others / 2);
    const beside = await scan();
    log(`budget: a ${count}-key scan ${alone.toFixed(1)} ms alone, ${beside.toFixed(1)} ms beside ${others} other keys (target: no growth)`);
    if (BENCH) expect(beside, 'target: a key scan does not grow with other replicas’ keys').toBeLessThanOrEqual(alone * 2 + 5);
    db.close();
    backend.close();
  }, 600_000);

  for (const n of BENCH ? [10_000, 100_000] : [200]) {
    it(`a load of ${n} rows with an observer settles within 1.5x of the load alone (H1, SDK half)`, async () => {
      // What a screen on SyncManager.observe costs (I4). The React hooks move
      // onto it in I6; until then the screen's own budget is device-load.bench.
      const run = async (observing: boolean) => {
        const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'usr_1');
        const t0 = performance.now();
        const manager = SyncManager.start(openFeedClient(n), replica, [{ spec: tasks }]);
        let snapshots = 0;
        const settled = new Promise<void>((resolve) => {
          if (!observing) {
            void manager.settled().then(() => resolve());
            return;
          }
          manager.observe(tasks.op, (s) => {
            snapshots += 1;
            if (s.state?.complete && s.rows.length === n) resolve();
          });
        });
        const result = await probe(() => settled);
        const ms = performance.now() - t0;
        await manager.stop();
        return { ms, stall: result.stall, snapshots };
      };
      const alone = await run(false);
      const observed = await run(true);
      log(
        `budget: load of ${n} rows alone ${alone.ms.toFixed(0)} ms (stall ${alone.stall.toFixed(1)} ms); with an observer ${observed.ms.toFixed(0)} ms ` +
          `(${(observed.ms / alone.ms).toFixed(2)}x, ${observed.snapshots} snapshots, stall ${observed.stall.toFixed(1)} ms)`,
      );
      if (BENCH) expect(observed.ms, 'target: within 1.5x of the load').toBeLessThanOrEqual(alone.ms * 1.5);
    }, 600_000);
  }

  it('local sort of 100k rows: money within 150 ms, text within 100 ms (H2)', () => {
    const n = BENCH ? 100_000 : 200;
    const rows = Array.from({ length: n }, (_, i) => row(i));
    const byText = time(() => refineRows(rows, { sort: [{ field: 'title' }] }, tasks.order));
    const byMoney = time(() => refineRows(rows, { sort: [{ field: 'total' }] }, tasks.order, ['total']));
    log(`budget: sort ${n} rows by money ${byMoney.toFixed(0)} ms (target 150), by text ${byText.toFixed(0)} ms (target 100)`);
    if (BENCH) {
      expect(byMoney, 'target: sort 100k rows by money in at most 150 ms').toBeLessThanOrEqual(150);
      expect(byText, 'target: sort 100k rows by text in at most 100 ms').toBeLessThanOrEqual(100);
    }
  }, 600_000);

  it('a sliced refine of 100k rows never holds the page over 50 ms (H2)', async () => {
    const n = BENCH ? 100_000 : 200;
    // Shuffled, so the sort has real work; and a filter, so every phase runs.
    const rows = Array.from({ length: n }, (_, i) => row((i * 7_919) % n));
    const query = { where: [{ field: 'done', op: 'eq' as const, value: false }], sort: [{ field: 'total' }] };
    const run = await probe(() => refineRowsAsync(rows, query, tasks.order, ['total'], BENCH ? {} : { chunk: 20 }));
    expect(run.result.map((r) => r.id)).toEqual(refineRows(rows, query, tasks.order, ['total']).map((r) => r.id));
    log(`budget: sliced refine of ${n} rows ${run.ms.toFixed(0)} ms, longest stall ${run.stall.toFixed(1)} ms (target 50)`);
    if (BENCH) expect(run.stall, 'target: no stall over 50 ms during a device refine of 100k rows').toBeLessThanOrEqual(50);
  }, 600_000);
});
