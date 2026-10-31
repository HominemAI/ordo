/**
 * The device copy's gaps the notes page found (board task `react-gaps`),
 * against the SDK's real `LocalReplica` and `SyncManager`, structurally
 * assigned with no cast: a reload that replaces the manager, a spec that
 * keeps what the SDK's `OpSpec` carries, the revocation signal, and the
 * access list a device must not hold.
 */

import { afterEach, describe, expect, it } from 'vitest';
import {
  LocalReplica,
  MemoryBackend,
  OrdoClient as SdkClient,
  SyncManager,
  checkRefine,
  refineRows,
  type OpSpec,
} from '../../ordo-sdk/src/index.js';
import { deviceCopy, isAccessMap, planDevice, toSyncSpec, type DeviceCopy } from '../src/device.js';
import { useOpQuery } from '../src/hooks/useOpQuery.js';
import { iface } from './harness.js';
import { LiveDoor, mount, settle, sse, until, type Mounted } from './live-door.js';

const task = (n: number) => ({ id: `task_${n}`, title: `task ${n}`, status: 'open' });
const load = (rows: unknown[], position: number) =>
  sse('load', { entity: 'tasks', key: 'id', position, pin: null, rows, loaded: rows.length, complete: true });

function managerOn(door: LiveDoor, replica: LocalReplica): SyncManager {
  const sdk = new SdkClient({ baseUrl: 'http://door.test', space: 'spc_test', token: 'tok', fetch: door.fetch });
  return SyncManager.start(sdk, replica, [], {
    backoff: (attempts) => (attempts === 0 ? 0 : 1e9),
    wait: (ms) => (ms === 0 ? Promise.resolve() : new Promise<void>(() => undefined)),
  });
}

let mounted: Mounted<unknown> | null = null;
const stops: (() => Promise<void>)[] = [];
afterEach(async () => {
  mounted?.unmount();
  mounted = null;
  for (const stop of stops.splice(0)) await stop();
});

describe('a reload that replaces the manager', () => {
  it('is followed by the device copy, which declares the hooks’ ops on the new manager', async () => {
    const door = new LiveDoor();
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'person_1');
    let current = managerOn(door, replica);
    stops.push(async () => current.stop());
    // The function form: the SDK's own manager is assignable, with no cast.
    const copy: DeviceCopy = deviceCopy({
      manager: () => current,
      replica,
      refineRows,
      reload: async () => {
        const old = current;
        current = managerOn(door, replica);
        await old.stop();
      },
    });
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: copy });
    mounted = m;
    await until(() => door.sync.length === 1, 'the first feed');
    door.sync[0]!.push(load([task(1)], 1));
    await until(() => m.result.current.rows.length === 1);

    await copy.reload();
    // The new manager was given the declaration the hook made, so it opened a feed.
    await until(() => door.sync.length === 2, 'the second feed');
    expect(current.view.ops.map((o) => o.key)).toEqual(['std.tasks.list']);
    door.sync[1]!.push(load([task(1), task(2)], 2));
    // And the hook, which watched the first manager, hears the second.
    await until(() => m.result.current.rows.length === 2, 'the reloaded rows');
  });

  it('accepts a single manager as before', async () => {
    const door = new LiveDoor();
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'person_1');
    const manager = managerOn(door, replica);
    stops.push(async () => manager.stop());
    const copy = deviceCopy({ manager, replica });
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    await until(() => m.result.current.rows.length === 1);
  });
});

describe('the sync spec the adapter builds', () => {
  const products = () => iface().requireOp('activeProducts');

  it('is the SDK’s OpSpec, so a refined syncable read keeps its types, sealed fields and limits', () => {
    const base = products();
    const op = {
      ...base,
      refine: { ...base.refine, limits: { conditions: 1, in_values: 2 } },
      result: {
        ...base.result,
        outputs: base.result.outputs.map((o) => (o.name === 'name' ? { ...o, sealed: true } : o)),
      },
    };
    // Assigned with no cast: the two shapes are one.
    const spec: OpSpec = toSyncSpec(op);
    expect(spec.refine.types).toMatchObject({ name: 'text', price: 'money' });
    expect(spec.refine.sealed).toEqual(['name']);
    expect(spec.refine.limits).toEqual({ conditions: 1, inValues: 2 });

    // The SDK's checks, which read exactly those three, now bite on the device path.
    const eq = (field: string, value: unknown) => ({ field, op: 'eq' as const, value });
    expect(() => checkRefine(spec, { where: [eq('price', 12.5)] }, 'door')).toThrow(/price/);
    expect(() => checkRefine(spec, { where: [eq('price', '12.50'), eq('vendor', 'vend_1')] }, 'door')).toThrow(
      /at most 1 conditions/,
    );
    expect(() => checkRefine(spec, { where: [{ field: 'name', op: 'gt', value: 'a' }] }, 'door')).toThrow(
      /equality only/,
    );
    expect(() => checkRefine(spec, { where: [eq('name', 'a')] }, 'door')).not.toThrow();
  });

  it('keeps a money parameter’s places, so the SDK refuses a finer value before the wire', async () => {
    const base = iface().requireOp('cheapActive');
    const op = { ...base, params: base.params.map((p) => (p.name === 'cap' ? { ...p, places: 2 } : p)) };
    const spec: OpSpec = toSyncSpec(op);
    expect(spec.params.find((p) => p.name === 'cap')?.places).toBe(2);
    const door = new LiveDoor();
    const sdk = new SdkClient({ baseUrl: 'http://door.test', space: 'spc_test', token: 'tok', fetch: door.fetch });
    await expect(sdk.request(spec, { cap: '1.005' })).rejects.toMatchObject({ detail: { class: 'invalid_param' } });
    expect(door.calls).toHaveLength(0);
  });
});

describe('a revoked synced op', () => {
  it('says so with revoked and the refusal class, and the replica is poisoned', async () => {
    const door = new LiveDoor();
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'person_1');
    const manager = managerOn(door, replica);
    stops.push(async () => manager.stop());
    const copy = deviceCopy({ manager, replica, refineRows });
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1), task(2)], 1));
    await until(() => m.result.current.rows.length === 2);
    expect(m.result.current.revoked).toBe(false);
    expect(copy.revoked).toBe(false);

    door.refuseNextStream(403, 'access_denied');
    door.sync[0]!.close();
    await until(() => m.result.current.revoked, 'the revoked flag');
    expect(m.result.current.refusal?.class).toBe('access_denied');
    expect(m.result.current.rows).toEqual([]);
    // As the page did by hand: nothing is held and nothing is served.
    await until(() => replica.poisoned, 'the replica to be poisoned');
    expect(copy.revoked).toBe(true);
    expect(await replica.scan('tasks')).toEqual([]);
  });

  it('poisons the replica even when no hook is mounted to hear of the revocation', async () => {
    const door = new LiveDoor();
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'person_1');
    const manager = managerOn(door, replica);
    stops.push(async () => manager.stop());
    const copy = deviceCopy({ manager, replica });
    // Declared the way a hook does, with nothing watching the copy.
    copy.declare(iface().requireOp('std.tasks.list'), {});
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    for (let i = 0; i < 100 && (await replica.scan('tasks')).length === 0; i++) await settle(0);
    expect(await replica.scan('tasks')).toHaveLength(1);
    door.refuseNextStream(403, 'access_denied');
    door.sync[0]!.close();
    await until(() => replica.poisoned, 'the replica to be poisoned');
    expect(copy.revoked).toBe(true);
  });

  it('stops being revoked once the manager is replaced, so reload works again', async () => {
    const door = new LiveDoor();
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'person_1');
    let current = managerOn(door, replica);
    stops.push(async () => current.stop());
    let reloads = 0;
    const copy = deviceCopy({
      manager: () => current,
      replica,
      reload: async () => {
        reloads++;
      },
    });
    copy.declare(iface().requireOp('std.tasks.list'), {});
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    for (let i = 0; i < 100 && (await replica.scan('tasks')).length === 0; i++) await settle(0);
    door.refuseNextStream(403, 'access_denied');
    door.sync[0]!.close();
    await until(() => copy.revoked, 'the revocation');
    await copy.reload();
    expect(reloads, 'a revoked copy reloads nothing').toBe(0);

    // A new sign-in: the app replaces the manager.
    const old = current;
    current = managerOn(door, replica);
    await old.stop();
    await copy.reload();
    expect(copy.revoked, 'the latch went with the manager it was set on').toBe(false);
    expect(reloads, 'reload is live again').toBe(1);
  });

  it('leaves the replica alone when the app turned poisoning off', async () => {
    const door = new LiveDoor();
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'person_1');
    const manager = managerOn(door, replica);
    stops.push(async () => manager.stop());
    const copy = deviceCopy({ manager, replica, poisonOnRevoke: false });
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client(), device: copy });
    mounted = m;
    await until(() => door.sync.length === 1);
    door.sync[0]!.push(load([task(1)], 1));
    await until(() => m.result.current.rows.length === 1);
    door.refuseNextStream(403, 'access_denied');
    door.sync[0]!.close();
    await until(() => m.result.current.revoked);
    await settle(5);
    expect(replica.poisoned).toBe(false);
  });

  it('says so from the network too, for a read that is not on the device', async () => {
    const door = new LiveDoor().reply({ status: 403, body: { error: { code: 'access_denied', message: 'no' } } });
    const m = await mount(() => useOpQuery('std.tasks.count'), { client: door.client() });
    mounted = m;
    await until(() => m.result.current.refusal !== null);
    expect(m.result.current.revoked).toBe(true);
    expect(m.result.current.refusal?.class).toBe('access_denied');
  });

  it('is not claimed for a refusal that is not a revocation', async () => {
    const door = new LiveDoor().reply({ status: 400, body: { error: { code: 'invalid_param', message: 'bad' } } });
    const m = await mount(() => useOpQuery('std.tasks.count'), { client: door.client() });
    mounted = m;
    await until(() => m.result.current.refusal !== null);
    expect(m.result.current.revoked).toBe(false);
  });
});

describe('the access rows of an acl entity', () => {
  it('are never mirrored on the device by default, because they are the permission map', () => {
    const i = iface();
    const access = i.requireOp('std.boardsAccess.list');
    const device = { canRefine: true } as DeviceCopy;
    expect(isAccessMap(access, i)).toBe(true);
    expect(planDevice(access, device, { iface: i })).toMatchObject({ use: false, why: 'never_mirrored' });
    // The entity they guard is an ordinary synced read.
    const boards = i.requireOp('std.boards.list');
    expect(isAccessMap(boards, i)).toBe(false);
    expect(planDevice(boards, device, { iface: i })).toEqual({ use: true });
  });

  it('go to the network from a hook, and say why', async () => {
    const door = new LiveDoor();
    const replica = await LocalReplica.open(new MemoryBackend(), 'spc_test', 'person_1');
    const manager = managerOn(door, replica);
    stops.push(async () => manager.stop());
    const copy = deviceCopy({ manager, replica });
    const m = await mount(() => useOpQuery('std.boardsAccess.list'), { client: door.client(), device: copy });
    mounted = m;
    await until(() => door.calls.length === 1, 'a network read');
    expect(m.result.current.source).toBe('network');
    expect(m.result.current.deviceFallback).toBe('never_mirrored');
    expect(m.result.current.deviceFallbackReason).toContain('permission map');
    expect(door.sync).toHaveLength(0);
  });
});
