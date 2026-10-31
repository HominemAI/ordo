/**
 * The H1 budget of `docs/plan-client-loading-fixes.md`: a screen mounted on
 * the device copy must not slow a load down.
 *
 * Each case times a sync load with no screen, then the same load with a
 * screen mounted, until the screen shows every row with `complete: true`.
 * The budget is 1.5 times the load alone. The wait for the screen is cut off
 * at that budget, so a case that misses fails at its deadline instead of
 * running for the hours the quadratic re-read would take at 100k rows.
 *
 * The budgets run only with `ORDO_BENCH=1`:
 *
 *     ORDO_BENCH=1 npx vitest run test/device-load.bench.test.ts
 *
 * The default run loads 200 rows, so CI keeps the file compiled and the
 * counting correct, and asserts only what holds today.
 */

import { afterEach, describe, expect, it } from 'vitest';
import {
  LocalReplica,
  MemoryBackend,
  OrdoClient as SdkClient,
  SyncManager,
  refineMerge,
  refineRows,
  refineRowsAsync,
} from '../../ordo-sdk/src/index.js';
import { deviceCopy, type DeviceCopy } from '../src/device.js';
import { useOpQuery } from '../src/hooks/useOpQuery.js';
import { iface } from './harness.js';
import { LiveDoor, mount, settle, sse, type Mounted, type Stream } from './live-door.js';

// `process` without the Node types: this package's tests compile against the DOM.
const BENCH = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env.ORDO_BENCH === '1';
const SIZES = BENCH ? [3_000, 10_000, 100_000] : [200];
const BUDGET = 1.5;
const PAGE = 50;

const log = (line: string) => {
  if (BENCH) console.log(`[bench] ${line}`);
};

const task = (n: number) => ({ id: `task_${String(n).padStart(8, '0')}`, title: `task ${n}`, status: 'open' });
const order = (n: number) => ({ id: `order_${String(n).padStart(8, '0')}`, customer: 'cust_1', total: `${n % 997}.00`, status: 'open' });

/** Every frame of a load of `n` rows, pushed at once: the door answers as fast as it can. */
function pushLoad(stream: Stream, entity: string, make: (n: number) => unknown, n: number): void {
  for (let at = 0; at < n; at += PAGE) {
    const rows = [];
    for (let i = at; i < Math.min(n, at + PAGE); i++) rows.push(make(i));
    const loaded = Math.min(n, at + PAGE);
    stream.push(sse('load', { entity, key: 'id', position: 1, pin: null, rows, loaded, complete: loaded === n }));
  }
}

interface Rig {
  readonly door: LiveDoor;
  readonly copy: DeviceCopy;
  readonly manager: SyncManager;
  /** `replica.get` calls the adapter made: the store row reads behind the screens. */
  readonly gets: { n: number };
}

async function rig(): Promise<Rig> {
  const door = new LiveDoor();
  const sdk = new SdkClient({ baseUrl: 'http://door.test', space: 'spc_test', token: 'tok', fetch: door.fetch });
  const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'person_1');
  const manager = SyncManager.start(sdk, replica, [], {
    backoff: (attempts) => (attempts === 0 ? 0 : 1e9),
    wait: (ms) => (ms === 0 ? Promise.resolve() : new Promise<void>(() => undefined)),
  });
  const gets = { n: 0 };
  const counted = {
    holdings: (op: string) => replica.holdings(op),
    get: (entity: string, id: string) => {
      gets.n += 1;
      return replica.get(entity, id);
    },
    poison: () => replica.poison(),
  };
  const copy = deviceCopy({
    manager: manager as never,
    replica: counted,
    refineRows: refineRows as never,
    refineMerge: refineMerge as never,
    refineRowsAsync: refineRowsAsync as never,
  });
  return { door, copy, manager, gets };
}

/** Poll until `check` holds or `deadline` (ms since `t0`) passes; the time it took, or null. */
async function waitFor(check: () => boolean, t0: number, deadline: number): Promise<number | null> {
  for (;;) {
    if (check()) return performance.now() - t0;
    if (performance.now() - t0 > deadline) return null;
    await settle(5);
  }
}

async function waitSync(sync: Stream[], count: number): Promise<void> {
  for (let i = 0; i < 1000 && sync.length < count; i++) await settle(0);
  expect(sync.length).toBe(count);
}

/** The time to load `n` rows of each op with nothing mounted. */
async function loadAlone(ops: readonly ('std.tasks.list' | 'std.orders.list')[], n: number): Promise<number> {
  const r = await rig();
  try {
    const t0 = performance.now();
    const handles = ops.map((op) => r.copy.declare(iface().requireOp(op), {}));
    await waitSync(r.door.sync, ops.length);
    feed(r.door.sync, n);
    const ms = await waitFor(() => handles.every((h) => r.copy.state(h)?.complete === true), t0, 3_600_000);
    return ms!;
  } finally {
    await r.manager.stop();
  }
}

function feed(streams: Stream[], n: number): void {
  for (const s of streams) {
    if (s.url.includes('std.orders.list')) pushLoad(s, 'orders', order, n);
    else pushLoad(s, 'tasks', task, n);
  }
}

/**
 * How long to wait for the screens. At 3k rows and below, room to see how far
 * over the budget a miss is; above, a miss stops at the budget.
 */
function deadlineFor(n: number, budget: number): number {
  if (!BENCH) return 60_000;
  return n <= 3_000 ? Math.max(budget, 2_000) * 15 : Math.max(budget, 2_000);
}

let mounted: Mounted<unknown> | null = null;
let running: Rig | null = null;
afterEach(async () => {
  mounted?.unmount();
  mounted = null;
  await running?.manager.stop();
  running = null;
});

/**
 * Mount `screens` on a fresh device, load `n` rows into each op they show,
 * and time it until every screen shows all `n` rows, complete.
 */
async function loadWithScreens(
  screens: readonly ('std.tasks.list' | 'std.orders.list')[],
  n: number,
  deadline: number,
): Promise<{ ms: number | null; gets: number }> {
  const r = await rig();
  running = r;
  const t0 = performance.now();
  const m = await mount(() => screens.map((op) => useOpQuery(op)), { client: r.door.client(), device: r.copy });
  mounted = m;
  const ops = new Set(screens).size;
  await waitSync(r.door.sync, ops);
  feed(r.door.sync, n);
  const ms = await waitFor(
    () => m.result.current.every((s) => s.complete && s.rows.length === n),
    t0,
    deadline,
  );
  return { ms, gets: r.gets.n };
}

describe('a screen mounted during a load (H1)', () => {
  for (const n of SIZES) {
    it(`${n} rows: one screen settles within ${BUDGET}x of the load alone`, async () => {
      const alone = await loadAlone(['std.tasks.list'], n);
      const budget = alone * BUDGET;
      const run = await loadWithScreens(['std.tasks.list'], n, deadlineFor(n, budget));
      const ratio = run.ms === null ? null : run.ms / alone;
      log(
        `n=${n}: load alone ${alone.toFixed(0)} ms; with one screen ` +
          `${run.ms === null ? `not settled by the deadline` : `${run.ms.toFixed(0)} ms (${ratio!.toFixed(1)}x)`}, ` +
          `${run.gets} store row reads`,
      );
      if (BENCH) {
        expect(run.ms, `target: settle with one screen within ${BUDGET}x of the load (${budget.toFixed(0)} ms)`).not.toBeNull();
        expect(run.ms!, `target: settle with one screen within ${BUDGET}x of the load (${budget.toFixed(0)} ms)`).toBeLessThanOrEqual(budget);
        expect(run.gets, `target: store row reads per load at most n plus the first read (${n + 1})`).toBeLessThanOrEqual(n + 1);
      } else {
        expect(run.ms).not.toBeNull();
      }
    }, 3_600_000);
  }

  for (const n of BENCH ? [3_000] : [100]) {
    it(`${n} rows: two screens on one op settle within ${BUDGET}x`, async () => {
      const alone = await loadAlone(['std.tasks.list'], n);
      const budget = alone * BUDGET;
      const run = await loadWithScreens(['std.tasks.list', 'std.tasks.list'], n, deadlineFor(n, budget));
      log(`n=${n}: two screens on one op ${run.ms === null ? 'not settled' : `${run.ms.toFixed(0)} ms (${(run.ms / alone).toFixed(1)}x)`}, ${run.gets} reads`);
      if (BENCH) expect(run.ms ?? Infinity, `target: within ${BUDGET}x of the load (${budget.toFixed(0)} ms)`).toBeLessThanOrEqual(budget);
      else expect(run.ms).not.toBeNull();
    }, 3_600_000);

    it(`${n} rows each: one screen on each of two ops settles within ${BUDGET}x`, async () => {
      const alone = await loadAlone(['std.tasks.list', 'std.orders.list'], n);
      const budget = alone * BUDGET;
      const run = await loadWithScreens(['std.tasks.list', 'std.orders.list'], n, deadlineFor(n, budget));
      log(`n=${n}: one screen on each of two ops ${run.ms === null ? 'not settled' : `${run.ms.toFixed(0)} ms (${(run.ms / alone).toFixed(1)}x)`}, ${run.gets} reads`);
      if (BENCH) expect(run.ms ?? Infinity, `target: within ${BUDGET}x of the load (${budget.toFixed(0)} ms)`).toBeLessThanOrEqual(budget);
      else expect(run.ms).not.toBeNull();
    }, 3_600_000);
  }
});
