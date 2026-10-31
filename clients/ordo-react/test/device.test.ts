/**
 * The device copy, against the SDK's real `LocalReplica` and `SyncManager`.
 *
 * Nothing about the replica is faked: the rows go through the SDK's sealing
 * and its sync frames, on an in-memory backend. Only the door is scripted.
 */

import { afterEach, describe, expect, it } from 'vitest';
import { createElement, StrictMode, type ReactNode } from 'react';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { OrdoProvider } from '../src/hooks/context.js';
import {
  LocalReplica,
  MemoryBackend,
  OrdoClient as SdkClient,
  SyncManager,
  openDevice as sdkOpenDevice,
  refineMerge,
  refineRows,
  refineRowsAsync,
} from '../../ordo-sdk/src/index.js';
import { asDeviceCopy, deviceCopy, planDevice, toSyncSpec, type DeviceCopy, type DeviceQuery, type DeviceSource } from '../src/device.js';
import { useCommit } from '../src/hooks/useCommit.js';
import { useOpPages } from '../src/hooks/useOpPages.js';
import { useOpQuery } from '../src/hooks/useOpQuery.js';
import { opRef, type OpenField } from '../src/refine.js';
import { iface } from './harness.js';
import { LiveDoor, mount, settle, sse, until, type Mounted } from './live-door.js';

const task = (n: number, title = `task ${n}`) => ({ id: `task_${n}`, title, status: 'open' });
const load = (rows: unknown[], position: number, complete = true) =>
  sse('load', { entity: 'tasks', key: 'id', position, pin: null, rows, loaded: rows.length, complete });
const change = (position: number, upserts: unknown[], deletes: string[] = []) =>
  sse('change', { entity: 'tasks', key: 'id', position, pin: null, upserts, deletes });

const titles = (rows: readonly unknown[]) => rows.map((r) => (r as { title: string }).title);

interface Device {
  readonly copy: DeviceCopy;
  readonly manager: SyncManager;
  readonly replica: LocalReplica;
  readonly reloads: number[];
  readonly stop: () => Promise<void>;
}

async function openDevice(
  door: LiveDoor,
  backend = new MemoryBackend(),
  reload?: () => Promise<void> | void,
): Promise<Device> {
  const sdk = new SdkClient({
    baseUrl: 'http://door.test',
    space: 'spc_test',
    token: 'tok',
    fetch: door.fetch,
  });
  const replica = await LocalReplica.open(backend, 'spc_test', 'person_1');
  // A clean end reconnects at once; a failed connection waits forever, so it
  // stays failed for the test's length instead of reconnecting in a loop.
  const manager = SyncManager.start(sdk, replica, [], {
    backoff: (attempts) => (attempts === 0 ? 0 : 1e9),
    wait: (ms) => (ms === 0 ? Promise.resolve() : new Promise<void>(() => undefined)),
  });
  const reloads: number[] = [];
  const copy = deviceCopy({
    manager: manager as never,
    replica: replica as never,
    refineRows: refineRows as never,
    reload: async () => {
      reloads.push(reloads.length);
      await reload?.();
    },
  });
  return { copy, manager, replica, reloads, stop: () => manager.stop() };
}

let mounted: Mounted<unknown> | null = null;
let device: Device | null = null;
afterEach(async () => {
  mounted?.unmount();
  mounted = null;
  await device?.stop();
  device = null;
});

describe('a synced op is answered from the device', () => {
  it('answers from the replica with the network off', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: device.copy });
    mounted = m;

    await until(() => door.sync.length === 1, 'the sync feed to open');
    expect(door.sync[0]!.url).toContain('sync=1');
    expect(m.result.current.loading).toBe(true);
    door.sync[0]!.push(load([task(1), task(2)], 1));
    await until(() => m.result.current.rows.length === 2, 'rows from the device');
    expect(m.result.current.source).toBe('device');
    expect(m.result.current.deviceFallback).toBeNull();
    expect(m.result.current.complete).toBe(true);

    // The network goes away and the feed with it.
    door.offline = true;
    door.sync[0]!.close();
    await settle(20);
    // In the op's own order (27o): `std.tasks.list` states `-id`, newest first.
    expect(titles(m.result.current.rows)).toEqual(['task 2', 'task 1']);
    expect(m.result.current.loading).toBe(false);
    // No ordinary call was ever made: the answer was never the network's.
    expect(door.calls).toHaveLength(0);
    expect(door.live).toHaveLength(0);
  });

  it('keeps answering after a restart, offline, from what the device stored', async () => {
    const backend = new MemoryBackend();
    const first = new LiveDoor();
    device = await openDevice(first, backend);
    const a = await mount(() => useOpQuery('std.tasks.list'), { client: first.client(), device: device.copy });
    await until(() => first.sync.length === 1);
    first.sync[0]!.push(load([task(1), task(2), task(3)], 1));
    await until(() => a.result.current.rows.length === 3);
    a.unmount();
    await device.stop();

    // A new page load with the network off: a new manager over the same backend.
    const second = new LiveDoor();
    second.offline = true;
    device = await openDevice(second, backend);
    const b = await mount(() => useOpQuery('std.tasks.list'), { client: second.client(), device: device.copy });
    mounted = b;
    await until(() => b.result.current.rows.length === 3, 'the stored rows');
    expect(b.result.current.source).toBe('device');
    // Not complete: nothing has confirmed them this session. Honest, not empty.
    expect(b.result.current.complete).toBe(false);
    expect(b.result.current.loading).toBe(false);
    expect(second.calls).toHaveLength(0);
  });

  it('a change frame reaches the hook without any refetch', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: device.copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1), task(2)], 1));
    await until(() => m.result.current.rows.length === 2);
    door.sync[0]!.push(change(2, [task(3)], ['task_1']));
    await until(() => m.result.current.rows.length === 2 && titles(m.result.current.rows)[0] === 'task 3');
    expect(titles(m.result.current.rows)).toEqual(['task 3', 'task 2']);
    expect(door.calls).toHaveLength(0);
  });

  it('refines locally, and sorts only when asked', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(
      () =>
        useOpQuery('std.tasks.list', {
          refine: { where: [{ field: 'title', op: 'ne', value: 'task 2' }] },
          sort: [{ field: 'title', direction: 'desc' }],
        }),
      { client: door.client(), device: device.copy },
    );
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1), task(2), task(3)], 1));
    await until(() => m.result.current.rows.length === 2);
    // The device answers it because the rows are already here: the server
    // does no work and is not asked.
    expect(titles(m.result.current.rows)).toEqual(['task 3', 'task 1']);
    expect(door.calls).toHaveLength(0);
  });

  it('answers a typed refinement locally, and a pick narrows the rows', async () => {
    interface TaskRow {
      readonly id: string;
      readonly title: string;
      readonly status: string;
    }
    interface TaskFields {
      readonly title: OpenField<'title', string>;
      readonly status: OpenField<'status', string>;
    }
    const tasks = opRef<TaskRow, TaskFields>('std.tasks.list');
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(
      () => useOpQuery(tasks, { refine: (v) => [v.title.in(['task 1', 'task 3'])], pick: ['title'] }),
      { client: door.client(), device: device.copy },
    );
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1), task(2), task(3)], 1));
    await until(() => m.result.current.rows.length === 2);
    expect(m.result.current.source).toBe('device');
    // Only the picked column, in the device copy as on the wire. The order is
    // the op's stated order, which this test does not pin.
    expect([...m.result.current.rows].sort((a, b) => a.title.localeCompare(b.title))).toEqual([
      { title: 'task 1' },
      { title: 'task 3' },
    ]);
    expect(door.calls).toHaveLength(0);
  });

  it('compares and sorts money as numbers, as the server does, not by code point', async () => {
    // `std.orders.list` returns `total: money`, on the wire a decimal string
    // (27ak). The device must filter and order it as Postgres does; the
    // adapter used to leave the SDK's refiner without the list of decimal
    // outputs, so "100.00" sorted before "29.99" and `gt "30"` dropped it.
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(
      () =>
        useOpQuery('std.orders.list', {
          refine: { where: [{ field: 'total', op: 'gt', value: '30' }] },
          sort: [{ field: 'total', direction: 'desc' }],
        }),
      { client: door.client(), device: device.copy },
    );
    mounted = m;
    await until(() => door.sync.length === 1);
    const order = (n: number, total: string) => ({ id: `order_${n}`, customer: 'cust_1', total, status: 'open' });
    const rows = [order(1, '100.00'), order(2, '29.99'), order(3, '5.00'), order(4, '2000.00')];
    door.sync[0]!.push(
      sse('load', { entity: 'orders', key: 'id', position: 1, pin: null, rows, loaded: rows.length, complete: true }),
    );
    await until(() => m.result.current.rows.length === 2, 'the two rows over 30');
    const totals = m.result.current.rows.map((r) => (r as { total: string }).total);
    expect(totals).toEqual(['2000.00', '100.00']);
    expect(door.calls).toHaveLength(0);
  });

  it('useOpPages is answered whole, with nothing left to page', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(() => useOpPages('std.tasks.list'), { client: door.client(), device: device.copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    expect(m.result.current.empty).toBe(false);
    door.sync[0]!.push(load([task(1), task(2)], 1));
    await until(() => m.result.current.rows.length === 2);
    expect(m.result.current.source).toBe('device');
    expect(m.result.current.hasMore).toBe(false);
    m.result.current.more();
    expect(door.calls).toHaveLength(0);
  });

  it('an empty result is empty only once the load is complete', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(() => useOpPages('std.tasks.list'), { client: door.client(), device: device.copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([], 1, false));
    await settle(10);
    expect(m.result.current.empty).toBe(false);
    door.sync[0]!.push(load([], 1, true));
    await until(() => m.result.current.empty);
  });

  it('a hook can ask for the network even for a syncable op', async () => {
    const door = new LiveDoor().reply({ body: { rows: [task(9)] } });
    device = await openDevice(door);
    const m = await mount(() => useOpQuery('std.tasks.list', { device: false }), {
      client: door.client(),
      device: device.copy,
    });
    mounted = m;
    await until(() => m.result.current.rows.length === 1);
    expect(m.result.current.source).toBe('network');
    expect(m.result.current.deviceFallback).toBe('opted_out');
    expect(door.sync).toHaveLength(0);
  });

  it('a device on the provider replaces live for a syncable op: the feed is the subscription', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(() => useOpQuery('std.tasks.list'), {
      client: door.client(),
      live: true,
      device: device.copy,
    });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    await until(() => m.result.current.rows.length === 1);
    expect(door.live).toHaveLength(0);
  });
});

describe('an op that cannot be synced goes to the network, and says so', () => {
  it('an aggregate: not syncable, answered by the network', async () => {
    const door = new LiveDoor().reply({ body: { rows: [{ n: 4 }] } });
    device = await openDevice(door);
    const m = await mount(() => useOpQuery('std.tasks.count'), { client: door.client(), device: device.copy });
    mounted = m;
    await until(() => m.result.current.rows.length === 1);
    expect(m.result.current.source).toBe('network');
    expect(m.result.current.deviceFallback).toBe('not_syncable');
    expect(m.result.current.deviceFallbackReason).toContain('aggregate');
    expect(door.calls).toHaveLength(1);
    expect(door.sync).toHaveLength(0);
  });

  it('a grouped read names its rule', async () => {
    const door = new LiveDoor().reply({ body: { rows: [] } });
    device = await openDevice(door);
    const m = await mount(() => useOpQuery('productRevenue'), { client: door.client(), device: device.copy });
    mounted = m;
    await until(() => door.calls.length === 1);
    expect(m.result.current.deviceFallback).toBe('not_syncable');
    expect(m.result.current.deviceFallbackReason).toContain('grouped');
  });

  it('a syncable op still falls back when a refinement cannot run on the device', async () => {
    const door = new LiveDoor().reply({ body: { rows: [] } });
    device = await openDevice(door);
    const copy = deviceCopy({ manager: device.manager as never, replica: device.replica as never });
    const m = await mount(
      () => useOpPages('std.tasks.list', { refine: { where: [{ field: 'title', op: 'eq', value: 'x' }] } }),
      { client: door.client(), device: copy },
    );
    mounted = m;
    await until(() => m.result.current.deviceFallback === 'refine_unavailable');
    expect(m.result.current.source).toBe('network');
  });

  it('no device configured: no fallback is reported, because nothing was declined', async () => {
    const door = new LiveDoor().reply({ body: { rows: [] } });
    const m = await mount(() => useOpQuery('std.tasks.count'), { client: door.client() });
    mounted = m;
    await until(() => door.calls.length === 1);
    expect(m.result.current.source).toBe('network');
    expect(m.result.current.deviceFallback).toBeNull();
  });

  it('never std.space.*, however the descriptor reads', () => {
    const base = iface().requireOp('std.tasks.list');
    const space = { ...base, op: 'std.space.members' };
    const fake = { canRefine: true } as DeviceCopy;
    const plan = planDevice(space, fake);
    expect(plan).toMatchObject({ use: false, why: 'never_mirrored' });
    // And the same op under a name that is not the permission map is used.
    expect(planDevice(base, fake)).toEqual({ use: true });
  });

  it('a write is never a device read', () => {
    const write = iface().requireOp('std.tasks.create');
    const plan = planDevice(write, { canRefine: true } as DeviceCopy);
    expect(plan).toMatchObject({ use: false, why: 'not_syncable' });
  });

  it('the sync spec is the descriptor’s projection: entity, key, and nothing for an aggregate', () => {
    expect(toSyncSpec(iface().requireOp('std.tasks.list')).sync).toEqual({ entity: 'tasks', key: 'id' });
    expect(toSyncSpec(iface().requireOp('std.tasks.count')).sync).toBeNull();
    expect(toSyncSpec(iface().requireOp('std.tasks.create')).sync).toBeNull();
  });
});

describe('a commit is reflected in a synced hook', () => {
  it('tells the device to ask again, and the new row shows', async () => {
    const door = new LiveDoor().reply({ body: { rows: [{ entity: 'tasks', created: 'task_2' }] } });
    // The app's reload is what asks the server again; here the server's
    // answer to that is a change frame on the open feed.
    device = await openDevice(door, new MemoryBackend(), () => {
      door.sync[0]!.push(change(2, [task(2, 'from the server')]));
    });
    const m = await mount(
      () => ({ list: useOpQuery('std.tasks.list'), commit: useCommit('std.tasks.create') }),
      { client: door.client(), device: device.copy },
    );
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    await until(() => m.result.current.list.rows.length === 1);

    await settle(); // let the commit hook settle
    const before = device.reloads.length;
    await act(async () => {
      await m.result.current.commit.commit({ project: 'proj_1', title: 'new' });
    });
    expect(m.result.current.commit.state.status).toBe('committed');
    expect(device.reloads.length).toBe(before + 1);
    await until(() => m.result.current.list.rows.length === 2, 'the committed row');
    expect(titles(m.result.current.list.rows)).toEqual(['from the server', 'task 1']);
  });

  it('nudge() makes the feed connect again and the screen follows the new load', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: device.copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    await until(() => m.result.current.rows.length === 1);

    device.copy.nudge();
    await until(() => door.sync.length === 2, 'the feed to open again');
    door.sync[1]!.push(load([task(1), task(2)], 2));
    await until(() => m.result.current.rows.length === 2, 'the second load');
    // A nudge is not a reload: the app's own reload was never called.
    expect(device.reloads).toHaveLength(0);
  });

  it('a refused commit does not touch the device', async () => {
    const door = new LiveDoor().reply({ status: 403, body: { error: { code: 'access_denied', message: 'no' } } });
    device = await openDevice(door);
    const m = await mount(() => useCommit('std.tasks.create'), { client: door.client(), device: device.copy });
    mounted = m;
    await act(async () => {
      await m.result.current.commit({ project: 'proj_1', title: 'new' });
    });
    expect(m.result.current.state.status).toBe('refused');
    expect(device.reloads).toHaveLength(0);
  });

  it('a write with no network is refused, not queued', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(() => useCommit('std.tasks.create'), { client: door.client(), device: device.copy });
    mounted = m;
    door.offline = true;
    await act(async () => {
      await m.result.current.commit({ project: 'proj_1', title: 'typed offline' });
    });
    expect(m.result.current.state.status).toBe('refused');
    expect(m.result.current.refusal?.message).toContain('refused, not queued');
    // Coming back online sends nothing by itself.
    door.offline = false;
    await settle(20);
    expect(door.calls).toHaveLength(0);
    expect(device.reloads).toHaveLength(0);
  });
});

describe('a screen reads only when its own op moves (H1)', () => {
  const order = (n: number) => ({ id: `order_${n}`, customer: 'cust_1', total: `${n}.00`, status: 'open' });
  const orders = (rows: unknown[], position: number, complete = true) =>
    sse('load', { entity: 'orders', key: 'id', position, pin: null, rows, loaded: rows.length, complete });

  /** A device whose adapter counts the replica reads it makes, by entity, and its reads in flight. */
  /** With `observe: false` the copy reads per change, as with a manager that cannot observe. */
  async function counted(door: LiveDoor, options: { observe?: boolean } = {}) {
    device = await openDevice(door);
    const replica = device.replica;
    const gets = new Map<string, number>();
    const flight = { now: 0, max: 0 };
    const copy = deviceCopy({
      manager: device.manager as never,
      replica: {
        holdings: (op: string) => replica.holdings(op),
        get: (entity: string, id: string) => {
          gets.set(entity, (gets.get(entity) ?? 0) + 1);
          return replica.get(entity, id);
        },
      },
      refineRows: refineRows as never,
    });
    // The same copy, with its reads counted from start to answer.
    const watched: DeviceCopy = Object.create(copy, {
      ...(options.observe === false ? { canObserve: { value: false } } : {}),
      read: {
        value: async (handle: string, query?: DeviceQuery) => {
          flight.now += 1;
          flight.max = Math.max(flight.max, flight.now);
          try {
            return await copy.read(handle, query);
          } finally {
            flight.now -= 1;
          }
        },
      },
    }) as DeviceCopy;
    return { copy: watched, gets, flight };
  }

  it('a frame for op A runs no read for op B', async () => {
    const door = new LiveDoor();
    const { copy, gets } = await counted(door);
    const m = await mount(
      () => ({ tasks: useOpQuery('std.tasks.list'), orders: useOpQuery('std.orders.list') }),
      { client: door.client(), device: copy },
    );
    mounted = m;
    await until(() => door.sync.length === 2);
    const [taskFeed, orderFeed] = door.sync[0]!.url.includes('std.tasks.list')
      ? [door.sync[0]!, door.sync[1]!]
      : [door.sync[1]!, door.sync[0]!];
    taskFeed.push(load([task(1)], 1));
    orderFeed.push(orders([order(1), order(2)], 1));
    await until(() => m.result.current.tasks.rows.length === 1 && m.result.current.orders.rows.length === 2);
    await settle(10);
    const before = gets.get('orders') ?? 0;

    for (let p = 2; p < 12; p++) taskFeed.push(change(p, [task(p)]));
    await until(() => m.result.current.tasks.rows.length === 11, 'every task');
    await settle(10);
    // The orders screen was never woken: no replica read of an order.
    expect(gets.get('orders') ?? 0).toBe(before);
    expect(m.result.current.orders.rows).toHaveLength(2);
  });

  it('at most one read in flight per hook, and the last frame is shown', async () => {
    // The read path, which a manager without `observe` still takes.
    const door = new LiveDoor();
    const { copy, flight } = await counted(door, { observe: false });
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    const all = Array.from({ length: 40 }, (_, i) => task(i + 1));
    // Forty frames at once, faster than any read.
    for (let i = 0; i < all.length; i++) {
      door.sync[0]!.push(
        sse('load', {
          entity: 'tasks',
          key: 'id',
          position: 1,
          pin: null,
          rows: [all[i]],
          loaded: i + 1,
          complete: i === all.length - 1,
        }),
      );
    }
    await until(() => m.result.current.complete && m.result.current.rows.length === 40, 'the whole load');
    expect(flight.max).toBe(1);
  });

  it('a frame over the same entity still reaches a screen that holds the row', async () => {
    // Two ops over `tasks`: a change through one updates a row the other
    // shows, so a notice about one wakes the other.
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(
      () => ({ a: useOpQuery('std.tasks.list'), b: useOpQuery('openTasks') }),
      { client: door.client(), device: device.copy },
    );
    mounted = m;
    await until(() => door.sync.length === 2);
    const [list, open] = door.sync[0]!.url.includes('std.tasks.list')
      ? [door.sync[0]!, door.sync[1]!]
      : [door.sync[1]!, door.sync[0]!];
    list.push(load([task(1)], 1));
    open.push(
      sse('load', { entity: 'tasks', key: '$id', position: 1, pin: null, rows: [{ $id: 'task_1', title: 'task 1', project: 'p' }], loaded: 1, complete: true }),
    );
    await until(() => m.result.current.a.rows.length === 1 && m.result.current.b.rows.length === 1);
    list.push(change(2, [task(1, 'renamed')]));
    await until(() => titles(m.result.current.b.rows)[0] === 'renamed', 'the shared row on the other screen');
  });
});

describe('revocation', () => {
  it('a revoking refusal on the feed empties the hook and the device rows the op held', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: device.copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1), task(2)], 1));
    await until(() => m.result.current.rows.length === 2);

    // Access is withdrawn: the feed ends and the next one is refused.
    door.refuseNextStream(403, 'access_denied');
    door.sync[0]!.close();
    await until(() => m.result.current.refusal !== null, 'the refusal');
    expect(m.result.current.refusal?.class).toBe('access_denied');
    expect(m.result.current.rows).toEqual([]);
    // The device itself released them, not just the hook. The hook hides
    // them the moment the refusal is known, and the release follows.
    for (let i = 0; i < 100 && (await device.replica.scan('tasks')).length > 0; i++) await settle(0);
    expect(await device.replica.scan('tasks')).toEqual([]);
  });

  it('a row another synced op still holds survives the revocation of this one', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const m = await mount(
      () => ({ a: useOpQuery('std.tasks.list'), b: useOpQuery('openTasks') }),
      { client: door.client(), device: device.copy },
    );
    mounted = m;
    await until(() => door.sync.length === 2);
    const [list, open] = door.sync[0]!.url.includes('std.tasks.list')
      ? [door.sync[0]!, door.sync[1]!]
      : [door.sync[1]!, door.sync[0]!];
    list.push(load([task(1)], 1));
    open.push(
      sse('load', {
        entity: 'tasks',
        key: '$id',
        position: 1,
        pin: null,
        rows: [{ $id: 'task_1', title: 'task 1', project: 'p' }],
        loaded: 1,
        complete: true,
      }),
    );
    await until(() => m.result.current.a.rows.length === 1 && m.result.current.b.rows.length === 1);

    door.refuseNextStream(403, 'access_denied');
    list.close();
    await until(() => m.result.current.a.refusal !== null);
    expect(m.result.current.a.rows).toEqual([]);
    expect(m.result.current.b.rows).toHaveLength(1);
  });
});

describe('a screen on rows in memory (I6)', () => {
  /** A copy over the device's manager with every SDK refiner, as an app passes them. */
  const full = (d: Device, refineChunk?: number): DeviceCopy =>
    deviceCopy({
      manager: d.manager as never,
      replica: d.replica as never,
      refineRows: refineRows as never,
      refineMerge: refineMerge as never,
      refineRowsAsync: refineRowsAsync as never,
      ...(refineChunk !== undefined ? { refineChunk } : {}),
    });

  it('complete is never true with a partial row set', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const copy = full(device);
    const seen: [boolean, number][] = [];
    const m = await mount(
      () => {
        const r = useOpQuery('std.tasks.list');
        seen.push([r.complete, r.rows.length]);
        return r;
      },
      { client: door.client(), device: copy },
    );
    mounted = m;
    await until(() => door.sync.length === 1);
    const all = Array.from({ length: 30 }, (_, i) => task(i + 1));
    for (let at = 0; at < 30; at += 5) {
      door.sync[0]!.push(
        sse('load', {
          entity: 'tasks',
          key: 'id',
          position: 1,
          pin: null,
          rows: all.slice(at, at + 5),
          loaded: at + 5,
          complete: at + 5 === 30,
        }),
      );
    }
    await until(() => m.result.current.complete && m.result.current.rows.length === 30, 'the whole load');
    expect(seen.filter(([complete, n]) => complete && n < 30)).toEqual([]);
  });

  it('a change to one row keeps every other row object, picked or not', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const copy = full(device);
    const m = await mount(
      () => ({ whole: useOpQuery('std.tasks.list'), picked: useOpQuery('std.tasks.list', { pick: ['title'] }) }),
      { client: door.client(), device: copy },
    );
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1), task(2), task(3)], 1));
    await until(() => m.result.current.whole.rows.length === 3 && m.result.current.picked.rows.length === 3);
    const before = { whole: [...m.result.current.whole.rows], picked: [...m.result.current.picked.rows] };
    door.sync[0]!.push(change(2, [task(2, 'renamed')]));
    await until(
      () =>
        titles(m.result.current.whole.rows).includes('renamed') && titles(m.result.current.picked.rows).includes('renamed'),
    );
    for (const which of ['whole', 'picked'] as const) {
      const after = m.result.current[which].rows;
      // `std.tasks.list` states `-id`: task 3, task 2, task 1.
      expect(titles(after)).toEqual(['task 3', 'renamed', 'task 1']);
      expect(after[0]).toBe(before[which][0]);
      expect(after[2]).toBe(before[which][2]);
      expect(after[1]).not.toBe(before[which][1]);
    }
  });

  it('a load of 2,000 rows with a screen mounted reads no row from the store', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const replica = device.replica;
    let gets = 0;
    const copy = deviceCopy({
      manager: device.manager as never,
      replica: {
        holdings: (op: string) => replica.holdings(op),
        get: (entity: string, id: string) => {
          gets += 1;
          return replica.get(entity, id);
        },
      },
      refineRows: refineRows as never,
      refineMerge: refineMerge as never,
    });
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    const n = 2_000;
    for (let at = 0; at < n; at += 50) {
      const rows = Array.from({ length: 50 }, (_, i) => task(at + i + 1));
      door.sync[0]!.push(
        sse('load', { entity: 'tasks', key: 'id', position: 1, pin: null, rows, loaded: at + 50, complete: at + 50 === n }),
      );
    }
    await until(() => m.result.current.complete && m.result.current.rows.length === n, 'the whole load');
    // At most once per row; here never, because the manager's one bulk read
    // and the frames themselves fill memory.
    expect(gets).toBe(0);
  });

  it('a large answer is refined in slices, then kept current by merging', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const copy = full(device, 10);
    const sort = [{ field: 'title', direction: 'desc' as const }];
    const m = await mount(() => useOpQuery('std.tasks.list', { sort }), { client: door.client(), device: copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    const all = Array.from({ length: 60 }, (_, i) => task(i + 1));
    door.sync[0]!.push(load(all, 1));
    await until(() => m.result.current.rows.length === 60, 'the rows, refined in slices');
    expect(m.result.current.rows).toEqual(refineRows(all, { sort }, null));
    door.sync[0]!.push(change(2, [task(61, 'task 99')], ['task_5']));
    await until(() => titles(m.result.current.rows)[0] === 'task 99', 'the change, merged');
    const now = all.filter((t) => t.id !== 'task_5').concat([task(61, 'task 99')]);
    expect(m.result.current.rows).toEqual(refineRows(now, { sort }, null));
  });

  it('without refineRowsAsync a large refinement runs whole and warns once', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const copy = deviceCopy({
      manager: device.manager as never,
      replica: device.replica as never,
      refineRows: refineRows as never,
      refineChunk: 10,
    });
    const warned: unknown[][] = [];
    const before = console.warn;
    console.warn = (...args: unknown[]) => {
      warned.push(args);
    };
    try {
      const sort = [{ field: 'title', direction: 'desc' as const }];
      const m = await mount(() => useOpQuery('std.tasks.list', { sort }), { client: door.client(), device: copy });
      mounted = m;
      await until(() => door.sync.length === 1);
      const all = Array.from({ length: 60 }, (_, i) => task(i + 1));
      door.sync[0]!.push(load(all, 1));
      await until(() => m.result.current.rows.length === 60, 'the rows, refined whole');
      expect(m.result.current.rows).toEqual(refineRows(all, { sort }, null));
      door.sync[0]!.push(change(2, [task(61, 'task 99')], ['task_5']));
      await until(() => titles(m.result.current.rows)[0] === 'task 99', 'the change');
    } finally {
      console.warn = before;
    }
    const ours = warned.filter((a) => String(a[0]).includes('refineRowsAsync'));
    expect(ours).toHaveLength(1);
  });

  it('a new sign-in drops every row in memory and starts from the new store', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    let current: SyncManager = device.manager;
    const sdk = new SdkClient({ baseUrl: 'http://door.test', space: 'spc_test', token: 'tok', fetch: door.fetch });
    const copy = deviceCopy({
      manager: () => current as never,
      replica: device.replica as never,
      refineRows: refineRows as never,
      refineMerge: refineMerge as never,
    });
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1), task(2)], 1));
    await until(() => m.result.current.rows.length === 2);

    // Another person signs in: a new replica, a new manager.
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'person_2');
    const second = SyncManager.start(sdk, replica, [], {
      backoff: (attempts) => (attempts === 0 ? 0 : 1e9),
      wait: (ms) => (ms === 0 ? Promise.resolve() : new Promise<void>(() => undefined)),
    });
    current = second;
    await act(async () => {
      await copy.reload();
    });
    await until(() => door.sync.length === 2, 'the new manager to open its feed');
    expect(m.result.current.rows).toHaveLength(0);
    door.sync[1]!.push(load([task(7)], 1));
    await until(() => m.result.current.rows.length === 1);
    expect(titles(m.result.current.rows)).toEqual(['task 7']);
    await second.stop();
  });
});

describe('a screen can hold a slice of a large op (M6)', () => {
  it('syncWhere narrows the sync itself, and the rows that pass it show', async () => {
    const door = new LiveDoor();
    device = await openDevice(door);
    const syncWhere = [{ field: 'status', op: 'eq' as const, value: 'open' }];
    const m = await mount(() => useOpQuery('std.tasks.list', { syncWhere }), { client: door.client(), device: device.copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    const url = new URL(door.sync[0]!.url);
    expect(JSON.parse(url.searchParams.get('$refine') ?? 'null')).toEqual({ where: syncWhere });
    door.sync[0]!.push(load([task(1), task(2)], 1));
    await until(() => m.result.current.rows.length === 2);
    expect(m.result.current.source).toBe('device');
  });
});

describe('reload() reconnects in place (plan-sdk-device.md 5)', () => {
  /** A device copy with no app reload, over the SDK's manager. */
  async function bare(door: LiveDoor) {
    const sdk = new SdkClient({ baseUrl: 'http://door.test', space: 'spc_test', token: 'tok', fetch: door.fetch });
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'person_1');
    const manager = SyncManager.start(sdk, replica, [], { online: null, publishMs: 0 });
    let reconnects = 0;
    const real = manager.reconnect.bind(manager);
    manager.reconnect = () => {
      reconnects += 1;
      return real();
    };
    const copy = deviceCopy({ manager: manager as never, replica: replica as never, refineRows: refineRows as never });
    return { copy, manager, replica, reconnects: () => reconnects, stop: () => manager.stop() };
  }

  it('calls the manager’s reconnect, and the screen keeps its rows until the new load', async () => {
    const door = new LiveDoor();
    const d = await bare(door);
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: d.copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    await until(() => m.result.current.rows.length === 1);
    let done = false;
    const reloading = d.copy.reload().then(() => {
      done = true;
    });
    await until(() => door.sync.length === 2, 'the feed to reopen');
    // The old feed ended before the new one opened, and the rows stayed.
    expect(door.sync[0]!.done).toBe(true);
    expect(m.result.current.rows).toHaveLength(1);
    expect(done).toBe(false);
    door.sync[1]!.push(load([task(1), task(2)], 2));
    await act(async () => {
      await reloading;
    });
    await until(() => m.result.current.rows.length === 2, 'the second load');
    expect(d.reconnects()).toBe(1);
    m.unmount();
    mounted = null;
    await d.stop();
  });

  it('a commit with no app reload reopens nothing: the feed brings the change', async () => {
    const door = new LiveDoor().reply({ body: { rows: [{ entity: 'tasks', created: 'task_2' }] } });
    const d = await bare(door);
    const m = await mount(
      () => ({ list: useOpQuery('std.tasks.list'), commit: useCommit('std.tasks.create') }),
      { client: door.client(), device: d.copy },
    );
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    await until(() => m.result.current.list.rows.length === 1);
    await act(async () => {
      await m.result.current.commit.commit({ project: 'proj_1', title: 'new' });
    });
    await settle(20);
    expect(d.reconnects()).toBe(0);
    expect(door.sync).toHaveLength(1);
    m.unmount();
    mounted = null;
    await d.stop();
  });
});

describe('the provider takes what openDevice answers (plan-sdk-device.md 5)', () => {
  it('wraps it once, answers hooks from it, and reload reconnects its manager', async () => {
    const door = new LiveDoor();
    const sdk = new SdkClient({ baseUrl: 'http://door.test', space: 'spc_test', token: 'tok', fetch: door.fetch });
    const opened = await sdkOpenDevice({
      client: sdk,
      space: 'spc_test',
      principal: 'person_1',
      backend: new MemoryBackend(),
      manager: { online: null },
    });
    // No cast: the SDK's device is a device source as it is.
    const source: DeviceSource = opened;
    expect(asDeviceCopy(source)).toBe(asDeviceCopy(source));
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: opened });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    await until(() => m.result.current.rows.length === 1);
    expect(m.result.current.source).toBe('device');

    const reloading = asDeviceCopy(opened).reload();
    await until(() => door.sync.length === 2, 'the feed to reopen');
    expect(door.sync[0]!.done).toBe(true);
    door.sync[1]!.push(load([task(1), task(2)], 2));
    await act(async () => {
      await reloading;
    });
    await until(() => m.result.current.rows.length === 2);
    m.unmount();
    mounted = null;
    await opened.close();
  });
});

describe('a sync nobody uses closes after idleMs (plan-sdk-device.md 6)', () => {
  const IDLE = 40;
  async function idleDevice(door: LiveDoor, idleMs = IDLE) {
    const sdk = new SdkClient({ baseUrl: 'http://door.test', space: 'spc_test', token: 'tok', fetch: door.fetch });
    const opened = await sdkOpenDevice({
      client: sdk,
      space: 'spc_test',
      principal: 'person_1',
      backend: new MemoryBackend(),
      manager: { online: null },
      idleMs,
    });
    return opened;
  }
  function List(): ReactNode {
    useOpQuery('std.tasks.list');
    return null;
  }
  /** `lists` screens on one op, under one provider, in StrictMode when asked. */
  const tree = (door: LiveDoor, device: DeviceSource, lists: number, strict = false) => {
    const body = createElement(
      OrdoProvider,
      { client: door.client(), device },
      ...Array.from({ length: lists }, (_, i) => createElement(List, { key: i })),
    );
    return strict ? createElement(StrictMode, null, body) : body;
  };
  const open = (door: LiveDoor) => door.sync.filter((s) => !s.done).length;

  it('two screens on one handle: one unmounts and the feed stays', async () => {
    const door = new LiveDoor();
    const device = await idleDevice(door);
    let r!: ReactTestRenderer;
    await act(async () => {
      r = create(tree(door, device, 2));
    });
    await until(() => door.sync.length === 1);
    await act(async () => r.update(tree(door, device, 1)));
    await settle(IDLE * 3);
    expect(open(door)).toBe(1);
    expect(door.sync).toHaveLength(1);
    await act(async () => r.unmount());
    await device.close();
  });

  it('the last screen unmounts and one mounts within idleMs: one feed throughout', async () => {
    const door = new LiveDoor();
    const device = await idleDevice(door);
    let r!: ReactTestRenderer;
    await act(async () => {
      r = create(tree(door, device, 1));
    });
    await until(() => door.sync.length === 1);
    await act(async () => r.update(tree(door, device, 0)));
    await settle(IDLE / 4);
    await act(async () => r.update(tree(door, device, 1)));
    await settle(IDLE * 3);
    expect(door.sync).toHaveLength(1);
    expect(open(door)).toBe(1);
    await act(async () => r.unmount());
    await device.close();
  });

  it('after idleMs the feed is closed, reload does not reopen it, and the next screen resumes', async () => {
    const door = new LiveDoor();
    const device = await idleDevice(door);
    let r!: ReactTestRenderer;
    await act(async () => {
      r = create(tree(door, device, 1));
    });
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    await until(() => device.manager.view.complete);
    await act(async () => r.update(tree(door, device, 0)));
    await settle(IDLE * 3);
    expect(open(door)).toBe(0);
    expect(device.manager.view.ops).toHaveLength(0);
    // The rows stay on the device.
    expect(await device.replica.holdings('std.tasks.list')).toHaveLength(1);
    await act(async () => {
      await asDeviceCopy(device).reload();
    });
    await settle(10);
    expect(door.sync).toHaveLength(1);

    // A screen mounts again: one new feed.
    await act(async () => r.update(tree(door, device, 1)));
    await until(() => door.sync.length === 2);
    expect(open(door)).toBe(1);
    await act(async () => r.unmount());
    await device.close();
  });

  it('StrictMode’s mount, unmount, mount opens one feed', async () => {
    const door = new LiveDoor();
    const device = await idleDevice(door);
    let r!: ReactTestRenderer;
    await act(async () => {
      r = create(tree(door, device, 1, true));
    });
    await until(() => door.sync.length === 1);
    await settle(IDLE * 3);
    expect(door.sync).toHaveLength(1);
    expect(open(door)).toBe(1);
    await act(async () => r.unmount());
    await device.close();
  });

  it('idleMs Infinity keeps the sync open after the last screen goes', async () => {
    const door = new LiveDoor();
    const device = await idleDevice(door, Infinity);
    let r!: ReactTestRenderer;
    await act(async () => {
      r = create(tree(door, device, 1));
    });
    await until(() => door.sync.length === 1);
    await act(async () => r.update(tree(door, device, 0)));
    await settle(IDLE * 3);
    expect(open(door)).toBe(1);
    await act(async () => r.unmount());
    await device.close();
  });

  it('a declared handle is never idled', async () => {
    const door = new LiveDoor();
    const device = await idleDevice(door);
    const copy = asDeviceCopy(device);
    const op = iface().requireOp('std.tasks.list');
    copy.declare(op, {});
    const use = copy.use!(op, {});
    use.release();
    await settle(IDLE * 3);
    expect(open(door)).toBe(1);
    await device.close();
  });
});
