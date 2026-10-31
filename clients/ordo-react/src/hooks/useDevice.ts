/**
 * `useDevice` — the device half of a read hook, shared by them.
 *
 * It decides, from the descriptor, whether the op is answered from the
 * device copy, and if so keeps the op in sync and reads its rows back
 * whenever the manager says something changed. The plan is a value, so a
 * hook can tell its caller *why* a read went to the network.
 */

import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import type { Params, Row } from '../client.js';
import type { OpDescriptor } from '../descriptor.js';
import {
  planDevice,
  type DeviceCopy,
  type DeviceObservation,
  type DeviceOpState,
  type DevicePlan,
  type DeviceQuery,
  type DeviceSnapshot,
} from '../device.js';
import type { Refine } from '../refine.js';
import type { Refusal } from '../refusal.js';
import { useOrdoRuntime } from './context.js';

export interface UseDeviceOptions {
  /** `false` asks the network even for a syncable op. */
  readonly device?: boolean | undefined;
  /** Device-only ordering; the wire envelope has no sort. */
  readonly sort?: DeviceQuery['sort'];
  /**
   * Keep only the rows that pass this filter on the device (M6). The server
   * sends just those, so a screen on a large op loads a slice of it. The
   * same conditions as a refinement's `where`; a different filter is a
   * different sync. Device only: the network path ignores it.
   */
  readonly syncWhere?: Refine['where'];
}

export interface DeviceAnswer<R> {
  readonly plan: DevicePlan;
  readonly rows: readonly R[];
  /** True until the device has something to say about this op. */
  readonly loading: boolean;
  /** False while the current load is still arriving; the rows may be partial. */
  readonly complete: boolean;
  /** Connections that failed in a row with nothing to show usually means offline. */
  readonly attempts: number;
  /** The refusal that ended this op's sync for good. */
  readonly refusal: Refusal | null;
  /** True when that refusal revoked the op: the device holds none of its rows. */
  readonly revoked: boolean;
}

const IDLE: DeviceAnswer<never> = {
  plan: { use: false, why: 'no_device', detail: '' },
  rows: [],
  loading: false,
  complete: false,
  attempts: 0,
  refusal: null,
  revoked: false,
};

const UNOBSERVED: DeviceSnapshot = { rows: [], state: null, filled: false };
const unobserved = (): DeviceSnapshot => UNOBSERVED;
const noSubscribe = (): (() => void) => () => undefined;

/**
 * A string that names the hook's inputs by content, recomputed only when
 * one of them is a new object (L6): a render with the same objects costs no
 * `JSON.stringify`, and new objects with the same content give the same key.
 */
function useContentKey(...inputs: unknown[]): string {
  const last = useRef<{ readonly inputs: readonly unknown[]; readonly key: string } | null>(null);
  const prev = last.current;
  if (prev !== null && prev.inputs.length === inputs.length && prev.inputs.every((v, i) => v === inputs[i])) return prev.key;
  const key = JSON.stringify(inputs.map((v) => v ?? null));
  last.current = { inputs, key };
  return key;
}

/**
 * One use of an op on the device, given back when the screen goes, so a sync
 * nobody uses closes after the device's `idleMs`. A copy without `use` keeps
 * every sync it was asked for.
 */
function useOf(
  device: DeviceCopy,
  op: OpDescriptor,
  params: Params,
  syncWhere: Refine['where'] | undefined,
): { readonly handle: string; release(): void } {
  if (device.use) return device.use(op, params, syncWhere);
  return { handle: device.declare(op, params, syncWhere), release: () => undefined };
}

export function useDevice<R = Row>(
  op: OpDescriptor,
  params: Params | undefined,
  refine: Refine | undefined,
  enabled: boolean,
  options: UseDeviceOptions,
): DeviceAnswer<R> {
  const { device, client } = useOrdoRuntime();
  const sort = options.sort;
  const syncWhere = options.syncWhere;
  const plan = useMemo(
    () => planDevice(op, device, { optOut: options.device === false, refine: refine ?? (sort ? {} : undefined), iface: client.iface }),
    [op, device, client, options.device, refine, sort],
  );
  const using = plan.use && enabled;
  const key = useContentKey(op.op, params, refine, sort, syncWhere);
  // Rows in memory when the manager keeps them (I6); a read per change otherwise.
  const observing = using && device !== null && device.canObserve === true && typeof device.observe === 'function';

  // The store a screen on rows in memory reads, one per op, params and query.
  const store = useMemo(() => {
    if (!observing || device === null) return null;
    const query: DeviceQuery = {
      ...(refine?.where ? { where: refine.where } : {}),
      ...(refine?.pick ? { pick: refine.pick } : {}),
      ...(sort ? { sort } : {}),
    };
    let observation: DeviceObservation | null = null;
    return {
      subscribe(onChange: () => void): () => void {
        const { handle, release } = useOf(device, op, params ?? {}, syncWhere);
        observation = device.observe?.(handle, query, onChange) ?? null;
        return () => {
          observation?.stop();
          observation = null;
          release();
        };
      },
      snapshot(): DeviceSnapshot {
        return observation?.snapshot() ?? UNOBSERVED;
      },
    };
    // `params`, `refine` and `sort` are keyed by content in `key`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [observing, device, op, key]);
  const observed = useSyncExternalStore(store?.subscribe ?? noSubscribe, store?.snapshot ?? unobserved, store?.snapshot ?? unobserved);

  const [answer, setAnswer] = useState<{ readonly key: string; readonly rows: readonly Row[]; readonly state: DeviceOpState | null } | null>(
    null,
  );

  useEffect(() => {
    if (!using || device === null || observing) {
      setAnswer(null);
      return;
    }
    const { handle, release } = useOf(device, op, params ?? {}, syncWhere);
    const query: DeviceQuery = {
      ...(refine?.where ? { where: refine.where } : {}),
      ...(refine?.pick ? { pick: refine.pick } : {}),
      ...(sort ? { sort } : {}),
    };
    let live = true;
    // One read in flight, and at most one waiting (H1). A notice during a
    // read marks it dirty, and one more read runs when it ends, so a load of
    // many frames costs a few reads rather than one per frame.
    let busy = false;
    let dirty = false;
    const refresh = async (): Promise<void> => {
      if (busy) {
        dirty = true;
        return;
      }
      busy = true;
      try {
        do {
          dirty = false;
          const rows = await device.read(handle, query);
          if (!live) return;
          setAnswer({ key, rows, state: device.state(handle) });
        } while (dirty && live);
      } finally {
        busy = false;
      }
    };
    void refresh();
    const stop = device.watch(() => void refresh(), handle);
    return () => {
      live = false;
      stop();
      release();
    };
    // `params`, `refine` and `sort` are keyed by content in `key`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [using, device, op, key, observing]);

  if (!using) return { ...IDLE, plan };
  // One snapshot carries the rows and the state of the same frames.
  const mine = observing
    ? observed.filled
      ? observed
      : null
    : answer !== null && answer.key === key
      ? answer
      : null;
  const state = mine?.state ?? null;
  const rows = (mine?.rows ?? []) as readonly R[];
  const refusal = state?.stopped ?? null;
  const attempts = state?.attempts ?? 0;
  return {
    plan,
    rows,
    // Nothing held and nothing said yet is loading. Nothing held after a
    // failed connection is an answer (an empty device, offline), not a wait.
    loading: mine === null || (!(state?.complete ?? false) && rows.length === 0 && refusal === null && attempts === 0),
    complete: state?.complete ?? false,
    attempts,
    refusal,
    revoked: state?.revoked ?? false,
  };
}
