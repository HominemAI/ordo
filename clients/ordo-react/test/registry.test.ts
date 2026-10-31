/**
 * One device per space: opened on first use, shared by every screen that asks,
 * closed when the last lets go or the provider unmounts.
 */

import { describe, expect, it } from 'vitest';
import { createElement, type ReactNode } from 'react';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { DeviceRegistry, DeviceRegistryProvider, useSpaceDevice, type SpaceDevice } from '../src/registry.js';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

interface Fake {
  readonly space: string;
  closed: number;
}

/** A registry whose devices are recorded, and whose opens the test can hold. */
function setup(idleCloseMs = 0) {
  const opened: string[] = [];
  const failures = new Set<string>();
  const registry = new DeviceRegistry<Fake>({
    idleCloseMs,
    open: async (space) => {
      opened.push(space);
      if (failures.has(space)) throw new Error(`cannot open ${space}`);
      return { space, closed: 0 };
    },
    close: (d) => {
      d.closed += 1;
    },
  });
  return { registry, opened, failures };
}

const later = (ms = 5) => new Promise<void>((r) => setTimeout(r, ms));

describe('DeviceRegistry', () => {
  it('opens a space once, however many screens ask', async () => {
    const { registry, opened } = setup();
    const a = registry.acquire('spc_a', 'ada');
    const b = registry.acquire('spc_a', 'ada');
    expect(await a.device).toBe(await b.device);
    expect(opened).toEqual(['spc_a']);
    expect(registry.keys().map((k) => k.space)).toEqual(['spc_a']);
    expect(registry.opened()).toHaveLength(1);
  });

  it('keeps one device per space', async () => {
    const { registry } = setup();
    const a = await registry.acquire('spc_a', 'ada').device;
    const b = await registry.acquire('spc_b', 'ada').device;
    expect(a).not.toBe(b);
    expect(registry.opened().map((d) => d.space)).toEqual(['spc_a', 'spc_b']);
  });

  it('closes a device when the last lease is released, and not before', async () => {
    const { registry } = setup();
    const a = registry.acquire('spc_a', 'ada');
    const b = registry.acquire('spc_a', 'ada');
    const device = await a.device;
    a.release();
    await later();
    expect(device.closed).toBe(0);
    b.release();
    b.release(); // twice is the same as once
    await later();
    expect(device.closed).toBe(1);
    expect(registry.keys().map((k) => k.space)).toEqual([]);
  });

  it('a new lease before the close keeps the device', async () => {
    const { registry, opened } = setup(20);
    const first = registry.acquire('spc_a', 'ada');
    const device = await first.device;
    first.release();
    const second = registry.acquire('spc_a', 'ada');
    await later(40);
    expect(device.closed).toBe(0);
    expect(await second.device).toBe(device);
    expect(opened).toEqual(['spc_a']);
  });

  it('opens again after a close', async () => {
    const { registry, opened } = setup();
    const first = registry.acquire('spc_a', 'ada');
    await first.device;
    first.release();
    await later();
    await registry.acquire('spc_a', 'ada').device;
    expect(opened).toEqual(['spc_a', 'spc_a']);
  });

  it('closeAll closes every device, including one still opening', async () => {
    const { registry } = setup();
    const a = registry.acquire('spc_a', 'ada');
    const b = registry.acquire('spc_b', 'ada');
    await registry.closeAll();
    expect((await a.device).closed).toBe(1);
    expect((await b.device).closed).toBe(1);
    expect(registry.keys().map((k) => k.space)).toEqual([]);
  });

  it('a failed open is reported to the lease and not kept', async () => {
    const { registry, opened, failures } = setup();
    failures.add('spc_a');
    await expect(registry.acquire('spc_a', 'ada').device).rejects.toThrow('cannot open spc_a');
    failures.clear();
    expect((await registry.acquire('spc_a', 'ada').device).space).toBe('spc_a');
    expect(opened).toEqual(['spc_a', 'spc_a']);
  });

  it("closes with the device's own close() when the app names none", async () => {
    let closed = 0;
    const registry = new DeviceRegistry<{ close(): void }>({
      open: async () => ({
        close: () => {
          closed += 1;
        },
      }),
    });
    await registry.acquire('spc_a', 'ada').device;
    await registry.closeAll();
    expect(closed).toBe(1);
  });
});

describe('keyed by space and person (device plan decision 7)', () => {
  interface Held {
    readonly space: string;
    readonly person: string;
    closed: number;
    purged: number;
    close(): void;
    purge(): void;
  }
  const make = () => {
    const registry = new DeviceRegistry<Held>({
      open: async (space, person) => {
        const d: Held = {
          space,
          person,
          closed: 0,
          purged: 0,
          close: () => void (d.closed += 1),
          purge: () => void (d.purged += 1),
        };
        return d;
      },
    });
    return registry;
  };

  it('two people in one space get two devices', async () => {
    const registry = make();
    const ada = await registry.acquire('spc_a', 'ada').device;
    const bo = await registry.acquire('spc_a', 'bo').device;
    expect(ada).not.toBe(bo);
    expect(registry.keys()).toEqual([
      { space: 'spc_a', person: 'ada' },
      { space: 'spc_a', person: 'bo' },
    ]);
  });

  it('purgeAll(person) erases only that person’s devices, open or closed since', async () => {
    const registry = make();
    const shared = registry.acquire('spc_a', 'ada');
    const adaA = await shared.device;
    shared.release();
    await later();
    expect(adaA.closed).toBe(1);
    const adaB = await registry.acquire('spc_b', 'ada').device;
    const bo = await registry.acquire('spc_b', 'bo').device;

    await registry.purgeAll('ada');
    expect([adaA.purged, adaB.purged, bo.purged]).toEqual([1, 1, 0]);
    expect(adaB.closed).toBe(1);
    expect(registry.keys()).toEqual([{ space: 'spc_b', person: 'bo' }]);
    // Purged once: a second call has nothing left of Ada's.
    await registry.purgeAll('ada');
    expect(adaA.purged).toBe(1);
    await registry.purgeAll();
    expect(bo.purged).toBe(1);
  });
});

describe('useSpaceDevice', () => {
  function Probe({ space, out }: { space: string | null; out: { current: SpaceDevice<Fake> } }): ReactNode {
    out.current = useSpaceDevice<Fake>(space, 'ada');
    return null;
  }
  const tree = (registry: DeviceRegistry<Fake>, space: string | null, out: { current: SpaceDevice<Fake> }) =>
    createElement(DeviceRegistryProvider<Fake>, { registry }, createElement(Probe, { space, out }));

  it('opens on first use, follows a change of space, and closes the old one', async () => {
    const { registry, opened } = setup();
    const out = { current: { device: null, error: null } as SpaceDevice<Fake> };
    let r!: ReactTestRenderer;
    await act(async () => {
      r = create(tree(registry, 'spc_a', out));
    });
    expect(out.current.device?.space).toBe('spc_a');
    const first = out.current.device!;

    await act(async () => {
      r.update(tree(registry, 'spc_b', out));
      await later();
    });
    expect(out.current.device?.space).toBe('spc_b');
    expect(first.closed).toBe(1);
    expect(opened).toEqual(['spc_a', 'spc_b']);
    await act(async () => r.unmount());
  });

  it('closes every device when the provider unmounts', async () => {
    const { registry } = setup();
    const out = { current: { device: null, error: null } as SpaceDevice<Fake> };
    let r!: ReactTestRenderer;
    await act(async () => {
      r = create(tree(registry, 'spc_a', out));
    });
    const device = out.current.device!;
    await act(async () => r.unmount());
    await later();
    expect(device.closed).toBe(1);
  });

  it('reports a failure to open, and no device', async () => {
    const { registry, failures } = setup();
    failures.add('spc_a');
    const out = { current: { device: null, error: null } as SpaceDevice<Fake> };
    let r!: ReactTestRenderer;
    await act(async () => {
      r = create(tree(registry, 'spc_a', out));
    });
    expect(out.current.device).toBeNull();
    expect(String(out.current.error)).toContain('cannot open spc_a');
    await act(async () => r.unmount());
  });

  it('a null space holds nothing', async () => {
    const { registry, opened } = setup();
    const out = { current: { device: null, error: null } as SpaceDevice<Fake> };
    let r!: ReactTestRenderer;
    await act(async () => {
      r = create(tree(registry, null, out));
    });
    expect(opened).toEqual([]);
    await act(async () => r.unmount());
  });
});
