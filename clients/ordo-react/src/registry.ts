/**
 * One device per space and person, opened when it is first used and closed
 * when it is not.
 *
 * An app that shows several spaces (a person's own, and each one shared with
 * them) holds one device for each. Writing the opening and the closing by hand
 * is easy to get wrong: a device opened twice, one never closed, one closed
 * while a screen still draws from it. {@link DeviceRegistry} does it once.
 *
 * Devices are keyed by space **and person** (ruled 2026-10-21, device plan
 * decision 7). Two people signed in on one origin, one after the other or in
 * two tabs, never share a device, and a sign-out purges only the devices of
 * the person leaving.
 *
 * It knows nothing of the SDK. The app says how to open a device for a space
 * and person (`open`) and the registry keeps what that returns. A device is
 * anything with a `close()`, or the app names how to close it (`close`). The
 * SDK's `openDevice` answers one with `close()` and `purge()`.
 *
 * - **Opened on first use.** The first {@link DeviceRegistry.acquire} of a
 *   space and person calls `open` once. Every later one, from any screen,
 *   shares it.
 * - **Closed when no screen uses it.** When the last lease is released the
 *   device is closed after `idleCloseMs` (default 0: the next turn of the
 *   event loop, so React's double effect in development does not open it
 *   twice). A new lease before then keeps it.
 * - **Closed when the registry is.** {@link DeviceRegistry.closeAll} closes
 *   every device, and `<DeviceRegistryProvider>` calls it when it unmounts.
 * - **Purged at sign-out.** {@link DeviceRegistry.purgeAll} erases every
 *   device the registry opened for a person, open or closed since.
 * - **A failed open is not kept.** The next lease tries again.
 */

import { createContext, createElement, useContext, useEffect, useState, type ReactNode } from 'react';

export interface DeviceRegistryOptions<D> {
  /** Open the device for `person` in `space`. Called once per pair until it is closed. */
  readonly open: (space: string, person: string) => Promise<D>;
  /** Close one device. Defaults to the device's own `close()`, when it has one. */
  readonly close?: (device: D, space: string, person: string) => Promise<void> | void;
  /**
   * Erase what one device holds. Defaults to the device's own `purge()`,
   * when it has one. {@link DeviceRegistry.purgeAll} calls it.
   */
  readonly purge?: (device: D, space: string, person: string) => Promise<void> | void;
  /**
   * How long a device with no lease stays open, in milliseconds, before it
   * is closed. Default 0, which is the next turn of the event loop.
   */
  readonly idleCloseMs?: number;
}

/** One screen's hold on a device. */
export interface DeviceLease<D> {
  /** The device, once it is open. Rejects when `open` does. */
  readonly device: Promise<D>;
  /** Let go. The device closes when no other lease holds it. Safe to call twice. */
  release(): void;
}

/** Which device: a space and the person it is held for. */
export interface DeviceKey {
  readonly space: string;
  readonly person: string;
}

interface Entry<D> {
  readonly key: string;
  readonly space: string;
  readonly person: string;
  readonly device: Promise<D>;
  /** The device once `open` has answered. */
  ready: D | undefined;
  leases: number;
  timer: ReturnType<typeof setTimeout> | null;
}

const keyOf = (space: string, person: string): string => JSON.stringify([space, person]);

export class DeviceRegistry<D> {
  readonly #options: DeviceRegistryOptions<D>;
  readonly #entries = new Map<string, Entry<D>>();
  /** Every device opened since the last purge of its person, open or closed since. */
  readonly #touched = new Set<Entry<D>>();

  constructor(options: DeviceRegistryOptions<D>) {
    this.#options = options;
  }

  /** Hold the device for `person` in `space`, opening it if no screen has yet. */
  acquire(space: string, person: string): DeviceLease<D> {
    const key = keyOf(space, person);
    let entry = this.#entries.get(key);
    if (entry?.timer) {
      clearTimeout(entry.timer);
      entry.timer = null;
    }
    if (!entry) {
      const made: Entry<D> = {
        key,
        space,
        person,
        device: this.#options.open(space, person).then(
          (d) => {
            made.ready = d;
            return d;
          },
          (e: unknown) => {
            // A failed open is not kept: the next lease tries again.
            if (this.#entries.get(key) === made) this.#entries.delete(key);
            this.#touched.delete(made);
            throw e;
          },
        ),
        ready: undefined,
        leases: 0,
        timer: null,
      };
      // Nobody may be listening yet; the lease's own promise reports it.
      made.device.catch(() => undefined);
      this.#entries.set(key, made);
      this.#touched.add(made);
      entry = made;
    }
    const held = entry;
    held.leases += 1;
    let released = false;
    return {
      device: held.device,
      release: () => {
        if (released) return;
        released = true;
        held.leases -= 1;
        if (held.leases > 0 || this.#entries.get(key) !== held) return;
        held.timer = setTimeout(() => {
          held.timer = null;
          if (held.leases === 0 && this.#entries.get(key) === held) void this.#closeEntry(held);
        }, this.#options.idleCloseMs ?? 0);
      },
    };
  }

  /** The devices that are open now, in the order they were first used. */
  opened(): D[] {
    const out: D[] = [];
    for (const e of this.#entries.values()) if (e.ready !== undefined) out.push(e.ready);
    return out;
  }

  /** The space and person of each device open or opening. */
  keys(): DeviceKey[] {
    return [...this.#entries.values()].map((e) => ({ space: e.space, person: e.person }));
  }

  /** Close every device, waiting for any still opening. The registry can be used again. */
  async closeAll(): Promise<void> {
    const all = [...this.#entries.values()];
    await Promise.allSettled(all.map((e) => this.#closeEntry(e)));
  }

  /**
   * Close and erase every device opened for `person`, or for everyone when
   * no person is named: those open now and those closed since they were
   * opened. Sign-out calls this. The registry can be used again afterwards.
   */
  async purgeAll(person?: string): Promise<void> {
    const all = [...this.#touched].filter((e) => person === undefined || e.person === person);
    for (const e of all) this.#touched.delete(e);
    await Promise.allSettled(
      all.map(async (e) => {
        await this.#closeEntry(e);
        let device: D;
        try {
          device = await e.device;
        } catch {
          return;
        }
        const purge = this.#options.purge;
        if (purge) await purge(device, e.space, e.person);
        else await (device as { purge?: () => Promise<void> | void }).purge?.();
      }),
    );
  }

  async #closeEntry(entry: Entry<D>): Promise<void> {
    if (this.#entries.get(entry.key) === entry) this.#entries.delete(entry.key);
    if (entry.timer) clearTimeout(entry.timer);
    entry.timer = null;
    let device: D;
    try {
      device = await entry.device;
    } catch {
      return;
    }
    const close = this.#options.close;
    if (close) await close(device, entry.space, entry.person);
    else await (device as { close?: () => Promise<void> | void }).close?.();
  }
}

const Ctx = createContext<DeviceRegistry<never> | null>(null);

export interface DeviceRegistryProviderProps<D> {
  readonly registry: DeviceRegistry<D>;
  readonly children?: ReactNode;
}

/**
 * Make `registry` available to {@link useSpaceDevice}, and close every device
 * in it when this provider unmounts.
 */
export function DeviceRegistryProvider<D>({ registry, children }: DeviceRegistryProviderProps<D>): ReactNode {
  useEffect(() => () => void registry.closeAll(), [registry]);
  return createElement(Ctx.Provider, { value: registry as unknown as DeviceRegistry<never> }, children);
}

/** What {@link useSpaceDevice} reports. */
export interface SpaceDevice<D> {
  /** The device, once open; `null` while it opens, and after a failure. */
  readonly device: D | null;
  /** Why it could not be opened. */
  readonly error: unknown;
}

/**
 * The device for `person` in `space`, from the nearest
 * {@link DeviceRegistryProvider}. The first screen to ask opens it; the last
 * to stop asking lets it close. `null` for a space is no device, and holds
 * nothing.
 */
export function useSpaceDevice<D>(space: string | null, person: string): SpaceDevice<D> {
  const registry = useContext(Ctx) as DeviceRegistry<D> | null;
  if (registry === null) {
    throw new Error('useSpaceDevice() outside <DeviceRegistryProvider>');
  }
  const [found, setFound] = useState<{ key: string; device: D | null; error: unknown } | null>(null);
  const key = space === null ? null : keyOf(space, person);
  useEffect(() => {
    if (space === null || key === null) return undefined;
    let alive = true;
    const lease = registry.acquire(space, person);
    lease.device.then(
      (device) => alive && setFound({ key, device, error: null }),
      (error: unknown) => alive && setFound({ key, device: null, error }),
    );
    return () => {
      alive = false;
      lease.release();
    };
  }, [registry, space, person, key]);
  // What was found for another space or person is not this one's device.
  return found !== null && found.key === key ? found : { device: null, error: null };
}
