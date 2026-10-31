/**
 * `useOpQuery(op)` — one shot over a read op, kept current when asked.
 *
 * Terminal-verb discipline is enforced by the client, so calling this on a
 * write refuses with `method_mismatch` rather than issuing a POST.
 *
 * Two things can be switched on at the provider (`OrdoProvider`):
 *
 * - **live** — the hook follows the client's shared live stream and
 *   refetches, paced, when the door says the data changed. Off unless the app or the hook asks.
 * - **device copy** — a syncable op is answered from the device replica,
 *   kept current by the SDK's sync manager, and keeps answering offline. An op
 *   the descriptor does not mark syncable goes to the network, and
 *   `source` and `deviceFallback` say so.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { Params, Row } from '../client.js';
import { sameRows } from '../pages.js';
import type { PlatformOpName, PlatformRows } from '../platform.js';
import { OrdoRefusal, type Refusal } from '../refusal.js';
import {
  refinementSurface,
  resolveRefine,
  type AnyFields,
  type OpRef,
  type Picked,
  type RefineInput,
  type RefinementSurface,
} from '../refine.js';
import { isRevoking, isSyncable, type DeviceFallback } from '../device.js';
import type { LiveStatus } from '../live.js';
import { useOrdoRuntime } from './context.js';
import { useDevice, type UseDeviceOptions } from './useDevice.js';
import { useLive, type UseLiveOptions } from './useLive.js';

export interface UseOpQueryOptions<F = AnyFields, K extends string = string> extends UseLiveOptions, UseDeviceOptions {
  readonly params?: Params;
  /**
   * A filter over the op's own outputs: the wire shape `{ where, pick }`, or
   * a callback over the field proxy, `(v) => [v.billed.gt('4000')]`. Each
   * hook's answer is its own: two hooks on one op with different
   * refinements fetch and hold different rows.
   */
  readonly refine?: RefineInput<F>;
  /** Keep only these fields; the row type narrows to `Pick<R, K>`. Replaces a pick in `refine`. */
  readonly pick?: readonly K[];
  /** Skip the call entirely — for a query whose params are not ready yet. */
  readonly enabled?: boolean;
}

export interface UseOpQuery<R = Row> {
  readonly rows: readonly R[];
  /** `rows[0]` — the shape a `single_row` op is for. */
  readonly row: R | undefined;
  /** Whether the descriptor says this op is statically one row (a metric). */
  readonly singleRow: boolean;
  readonly loading: boolean;
  readonly refusal: Refusal | null;
  /**
   * True when {@link refusal} is a revocation (`access_denied`, `access_guard`
   * or `delegation_required`): this caller may no longer have these rows. The
   * hook shows none, and a synced op's rows are gone from the device with the
   * replica poisoned. The class is in `refusal.class`.
   */
  readonly revoked: boolean;
  /** `X-Ordo-Version-Skew: stale` — the call still executed. */
  readonly stale: boolean;
  /**
   * True while the door answered `server_busy` and the fetch waits to try
   * again (L7), so the screen can say so. The retries are capped.
   */
  readonly retrying: boolean;
  readonly refetch: () => void;
  readonly refinement: RefinementSurface;
  /** Where the rows came from on the last answer. */
  readonly source: 'network' | 'device';
  /**
   * Why a read went to the network although a device copy is configured, or
   * `null` (answered from the device, or no device copy is configured).
   */
  readonly deviceFallback: DeviceFallback | null;
  /** The rule behind {@link deviceFallback}, in words. */
  readonly deviceFallbackReason: string | null;
  /** False while a device answer may still be partial. Always true from the network. */
  readonly complete: boolean;
  /** The live subscription's state, or `null` when the hook is not following. */
  readonly live: LiveStatus | null;
}

/**
 * Pass an {@link OpRef} (`opRef<VendorSpendRow, VendorSpendFields>('vendorSpend')`)
 * and the `refine` callback, `pick` and the rows are typed from it. A plain
 * name works as before, with rows typed `R`.
 */
export function useOpQuery<N extends PlatformOpName>(
  opName: N,
  options?: UseOpQueryOptions,
): UseOpQuery<PlatformRows[N]>;
export function useOpQuery<R = Row, F = AnyFields, K extends keyof R & string = keyof R & string>(
  opName: OpRef<R, F> | string,
  options?: UseOpQueryOptions<F, K>,
): UseOpQuery<Picked<R, K>>;
export function useOpQuery<R = Row, F = AnyFields, K extends keyof R & string = keyof R & string>(
  opName: OpRef<R, F> | string,
  options: UseOpQueryOptions<F, K> = {},
): UseOpQuery<Picked<R, K>> {
  return useOpQueryRows<Picked<R, K>>(opName, options as UseOpQueryOptions<unknown, string>);
}

function useOpQueryRows<R>(opName: string, options: UseOpQueryOptions<unknown, string>): UseOpQuery<R> {
  const { client, device: deviceCopy } = useOrdoRuntime();
  const op = useMemo(() => client.iface.requireOp(opName), [client, opName]);
  const { params, enabled = true } = options;
  // The typed form becomes the wire shape here, once per render, and is then
  // keyed by content: a callback is a new function every render, and its
  // identity must not refetch.
  const resolved = resolveRefine(options.refine, options.pick);
  const refineKey = JSON.stringify(resolved ?? null);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const refine = useMemo(() => resolved, [refineKey]);
  const answer = useDevice<R>(op, params, refine, enabled, options);
  const onDevice = answer.plan.use;

  const [rows, setRows] = useState<readonly R[]>([]);
  const [loading, setLoading] = useState(enabled);
  const [refusal, setRefusal] = useState<Refusal | null>(null);
  const [stale, setStale] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const [tick, setTick] = useState(0);

  // Params and refinement are plain objects that change identity every
  // render. Keying the effect on their content, not their identity, is what
  // keeps a caller from writing `useOpQuery(op, { params: { id } })` and
  // getting an infinite fetch loop.
  const key = useMemo(() => JSON.stringify([params ?? null, refine ?? null]), [params, refine]);
  const latest = useRef(0);
  const fetchedKey = useRef<string | null>(null);
  const signal = useLive(op, params, enabled && !onDevice, options, refine);
  // Set when a fetch starts before the read is on the stream; the open then
  // owes one more fetch, for a change made in between.
  const beforeOpen = useRef(false);
  const openedRef = useRef(signal.opened);
  openedRef.current = signal.opened;

  useEffect(() => {
    if (!enabled || onDevice) {
      setLoading(false);
      return;
    }
    // The fetch starts at once, beside the subscribe: a live stream that is
    // slow or never opens must not hide the rows. If it starts before the
    // read is on the stream, the open fetches once more (below).
    if (!openedRef.current) beforeOpen.current = true;
    const seq = ++latest.current;
    const controller = new AbortController();
    // A refetch of what is already on screen is not a loading state: the
    // rows stay, and swap when the new ones arrive.
    if (fetchedKey.current !== key) setLoading(true);
    void (async () => {
      try {
        const res = await client.fetchOp<R>(opName, {
          params: params ?? {},
          refine,
          signal: controller.signal,
          onRetry: () => {
            if (seq === latest.current) setRetrying(true);
          },
        });
        if (seq === latest.current) setRetrying(false);
        if (seq !== latest.current) return;
        // A refetch that changed nothing keeps the array on screen (L4).
        setRows((old) => (sameRows(old, res.rows) ? old : res.rows));
        setStale(res.stale);
        setRefusal(null);
        fetchedKey.current = key;
      } catch (e) {
        if (seq !== latest.current) return;
        setRetrying(false);
        if (e instanceof OrdoRefusal) {
          const r = e.refusal as Refusal;
          setRefusal(r);
          if (isRevoking(r)) {
            // The door says this caller may not have these rows. Showing the
            // last ones would be showing what was just withdrawn, and the
            // device must not keep what the op held either.
            setRows([]);
            if (deviceCopy && isSyncable(op)) void deviceCopy.revokeOp(op.op);
          }
        } else throw e;
      } finally {
        if (seq === latest.current) setLoading(false);
      }
    })();
    return () => controller.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, opName, key, enabled, tick, onDevice, signal.tick]);

  // The read just opened on the stream, and the last fetch began before that:
  // a change between the two would be missed. One more fetch, once.
  useEffect(() => {
    if (!signal.opened || !beforeOpen.current) return;
    beforeOpen.current = false;
    setTick((t) => t + 1);
  }, [signal.opened]);

  const refetch = useCallback(() => setTick((t) => t + 1), []);

  const shown = onDevice ? (isRevoking(answer.refusal) ? [] : (answer.rows as readonly R[])) : rows;
  return {
    rows: shown,
    row: shown[0],
    singleRow: op.result.single_row,
    loading: onDevice ? answer.loading : loading,
    refusal: onDevice ? answer.refusal : refusal,
    revoked: isRevoking(onDevice ? answer.refusal : refusal),
    stale: onDevice ? false : stale,
    retrying: onDevice ? false : retrying,
    refetch,
    refinement: refinementSurface(op),
    source: onDevice ? 'device' : 'network',
    deviceFallback: answer.plan.use || answer.plan.why === 'no_device' ? null : answer.plan.why,
    deviceFallbackReason: answer.plan.use || answer.plan.why === 'no_device' ? null : answer.plan.detail,
    complete: onDevice ? answer.complete : true,
    live: signal.status,
  };
}
