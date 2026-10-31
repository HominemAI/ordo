/**
 * `useOpPages(op)` — a keyset listing with cursor state.
 *
 * `hasMore` is `next !== undefined`. It is never `rows.length > 0`, and the
 * empty state is `empty && !hasMore` — a listing that returned an empty first
 * page while `next` was set is *not* empty, it is narrow (§7).
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { Params, Row } from '../client.js';
import { isRevoking, isSyncable, type DeviceFallback } from '../device.js';
import type { LiveStatus } from '../live.js';
import { collectAll, emptyPageState, nextPage, sameRows, type PageState } from '../pages.js';
import type { Refusal } from '../refusal.js';
import {
  refinementSurface,
  resolveRefine,
  type AnyFields,
  type OpRef,
  type Picked,
  type RefineInput,
  type RefinementSurface,
} from '../refine.js';
import { useOrdoRuntime } from './context.js';
import { useDevice, type UseDeviceOptions } from './useDevice.js';
import { useLive, type UseLiveOptions } from './useLive.js';

export interface UseOpPagesOptions<F = AnyFields, K extends string = string> extends UseLiveOptions, UseDeviceOptions {
  readonly params?: Params;
  /**
   * A filter over the op's own outputs: the wire shape, or a callback over
   * the field proxy. Every page, and every live reload, carries it.
   */
  readonly refine?: RefineInput<F>;
  /** Keep only these fields; the row type narrows to `Pick<R, K>`. */
  readonly pick?: readonly K[];
  readonly enabled?: boolean;
  /** Fetch the first page on mount. Default true. */
  readonly autoFirstPage?: boolean;
}

export interface ExhaustOptions {
  readonly signal?: AbortSignal;
  /** Pages between two updates of the rows on screen; `DEFAULT_PUBLISH_EVERY` (10) by default. */
  readonly publishEvery?: number;
}

export interface UseOpPages<R = Row> {
  readonly rows: readonly R[];
  /** Changes every time `rows` does, even when `rows` keeps its reference as `more()` appends. */
  readonly version: number;
  readonly state: PageState<R>;
  readonly hasMore: boolean;
  /** The *iteration* produced nothing and there is nothing left. */
  readonly empty: boolean;
  readonly loading: boolean;
  readonly refusal: Refusal | null;
  /**
   * True when {@link refusal} is a revocation (`access_denied`, `access_guard`
   * or `delegation_required`): this caller may no longer have these rows. The
   * hook shows none, and a synced op's rows are gone from the device with the
   * replica poisoned. The class is in `refusal.class`.
   */
  readonly revoked: boolean;
  /**
   * A stale cursor was dropped and the listing restarted. Distinct from a
   * CAS conflict: no data disagreed, the op's version moved under the token.
   */
  readonly restarted: Refusal | null;
  readonly stale: boolean;
  /** Fetch the next page. No-op once `hasMore` is false. */
  readonly more: () => void;
  /** Drop the cursor and start again from page one. */
  readonly restart: () => void;
  /**
   * Page to exhaustion: stops only on a missing `next`. The rows so far show
   * every `publishEvery` pages; a signal stops the walk with what it has. A
   * call while a page or a reload is in flight does nothing.
   */
  readonly exhaust: (maxPages?: number, options?: ExhaustOptions) => void;
  readonly refinement: RefinementSurface;
  /**
   * Where the rows came from. The device answers the whole result at once,
   * so `hasMore` is false and `more()` does nothing when it does.
   */
  readonly source: 'network' | 'device';
  /** Why a read went to the network although a device copy is configured. */
  readonly deviceFallback: DeviceFallback | null;
  readonly deviceFallbackReason: string | null;
  /** False while a device answer may still be partial. Always true from the network. */
  readonly complete: boolean;
  /** The live subscription's state, or `null` when the hook is not following. */
  readonly live: LiveStatus | null;
}

/** As `useOpQuery`: an {@link OpRef} types the `refine` callback, `pick` and the rows. */
export function useOpPages<R = Row, F = AnyFields, K extends keyof R & string = keyof R & string>(
  opName: OpRef<R, F> | string,
  options: UseOpPagesOptions<F, K> = {},
): UseOpPages<Picked<R, K>> {
  return useOpPagesRows<Picked<R, K>>(opName, options as UseOpPagesOptions<unknown, string>);
}

function useOpPagesRows<R>(opName: string, options: UseOpPagesOptions<unknown, string>): UseOpPages<R> {
  const { client, device: deviceCopy } = useOrdoRuntime();
  const op = useMemo(() => client.iface.requireOp(opName), [client, opName]);
  const { params, enabled = true, autoFirstPage = true } = options;
  // The typed form becomes the wire shape, keyed by content (see useOpQuery).
  const resolved = resolveRefine(options.refine, options.pick);
  const refineKey = JSON.stringify(resolved ?? null);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const refine = useMemo(() => resolved, [refineKey]);
  const answer = useDevice<R>(op, params, refine, enabled, options);
  const onDevice = answer.plan.use;
  const signal = useLive(op, params, enabled && !onDevice, options, refine);

  const [state, setState] = useState<PageState<R>>(() => emptyPageState<R>());
  const [loading, setLoading] = useState(false);
  const [refusal, setRefusal] = useState<Refusal | null>(null);
  const [restarted, setRestarted] = useState<Refusal | null>(null);

  const key = useMemo(() => JSON.stringify([params ?? null, refine ?? null]), [params, refine]);
  const stateRef = useRef(state);
  stateRef.current = state;
  const busy = useRef(false);

  const call = useMemo(() => ({ params: params ?? {}, refine }), [params, refine]);

  const dirty = useRef(false);
  const reloadRef = useRef<() => void>(() => undefined);

  /** A revoking refusal empties the listing and, on a synced op, the device. */
  const revoked = useCallback(
    (r: Refusal) => {
      if (!isRevoking(r)) return;
      setState(emptyPageState<R>());
      if (deviceCopy && isSyncable(op)) void deviceCopy.revokeOp(op.op);
    },
    [deviceCopy, op],
  );

  const more = useCallback(() => {
    if (onDevice || busy.current || !stateRef.current.hasMore) return;
    busy.current = true;
    setLoading(true);
    void (async () => {
      try {
        const step = await nextPage<R>(client, opName, stateRef.current, call);
        setState(step.state);
        if (step.status === 'restart') {
          // The cursor is stale, the rows were not. Restarting is the whole
          // repair (§8: the token is not migrated).
          setRestarted(step.refusal);
          setRefusal(null);
        } else if (step.status === 'refused') {
          setRefusal(step.refusal);
          revoked(step.refusal);
        } else {
          setRefusal(null);
        }
      } finally {
        busy.current = false;
        setLoading(false);
        if (dirty.current) {
          dirty.current = false;
          reloadRef.current();
        }
      }
    })();
  }, [client, opName, call, onDevice, revoked]);

  /**
   * A live invalidation: read again what is already on screen. The same
   * number of pages are fetched from the top and swapped in whole, so the
   * rows do not vanish while the new ones load and a user scrolled ten pages
   * down is not returned to page one.
   */
  const reload = useCallback(() => {
    if (onDevice) return;
    if (busy.current) {
      // A fetch is in flight. One more pass after it covers every signal
      // that arrived meanwhile, however many there were.
      dirty.current = true;
      return;
    }
    if (stateRef.current.pagesFetched === 0) return;
    busy.current = true;
    void (async () => {
      try {
        const pages = Math.max(1, stateRef.current.pagesFetched);
        const step = await collectAll<R>(client, opName, { ...call, maxPages: pages });
        if (step.status === 'page') {
          // The same rows keep the array on screen (L4).
          setState((old) => (sameRows(old.rows, step.state.rows) ? { ...step.state, rows: old.rows } : step.state));
          setRefusal(null);
        } else if (step.status === 'restart') {
          setState(step.state);
          setRestarted(step.refusal);
        } else {
          setRefusal(step.refusal);
          revoked(step.refusal);
        }
      } finally {
        busy.current = false;
        if (dirty.current) {
          dirty.current = false;
          reloadRef.current();
        }
      }
    })();
  }, [client, opName, call, onDevice, revoked]);
  reloadRef.current = reload;

  const restart = useCallback(() => {
    setState(emptyPageState<R>());
    setRefusal(null);
    setRestarted(null);
  }, []);

  const exhaust = useCallback(
    (maxPages?: number, options: ExhaustOptions = {}) => {
      if (onDevice) return;
      // One walk at a time: a page or a reload in flight would interleave
      // with it and append rows twice (M4).
      if (busy.current) return;
      busy.current = true;
      setLoading(true);
      void (async () => {
        try {
          const step = await collectAll<R>(client, opName, {
            ...call,
            ...(maxPages ? { maxPages } : {}),
            ...(options.signal ? { signal: options.signal } : {}),
            ...(options.publishEvery ? { publishEvery: options.publishEvery } : {}),
            // The rows so far show while the walk goes on.
            onProgress: (so) => setState(so),
          });
          setState(step.state);
          if (step.status === 'restart') setRestarted(step.refusal);
          else if (step.status === 'refused') {
            setRefusal(step.refusal);
            revoked(step.refusal);
          } else setRefusal(null);
        } finally {
          busy.current = false;
          setLoading(false);
        }
      })();
    },
    [client, opName, call, onDevice, revoked],
  );

  // A parameter change is a different listing, so the cursor from the old one
  // is meaningless — carrying it over would seek into another result set.
  useEffect(() => {
    setState(emptyPageState<R>());
    setRefusal(null);
    setRestarted(null);
  }, [key, opName]);

  const first = useRef('');
  const beforeOpen = useRef(false);
  const openedRef = useRef(signal.opened);
  openedRef.current = signal.opened;
  useEffect(() => {
    // The first page starts at once, beside the subscribe, like `useOpQuery`.
    if (!enabled || !autoFirstPage || onDevice) return;
    const id = `${opName}:${key}`;
    if (first.current === id) return;
    first.current = id;
    if (!openedRef.current) beforeOpen.current = true;
    more();
  }, [enabled, autoFirstPage, opName, key, more, onDevice]);

  // The read opened on the stream after the first page began: read the pages
  // on screen once more, so a change in between is not missed.
  useEffect(() => {
    if (!signal.opened || !beforeOpen.current) return;
    beforeOpen.current = false;
    reloadRef.current();
  }, [signal.opened]);

  useEffect(() => {
    if (signal.tick > 0) reloadRef.current();
  }, [signal.tick]);

  if (onDevice) {
    const shown = (isRevoking(answer.refusal) ? [] : answer.rows) as readonly R[];
    const deviceState: PageState<R> = {
      rows: shown,
      cursor: null,
      hasMore: false,
      empty: shown.length === 0,
      pagesFetched: 1,
      stale: false,
      version: 1,
    };
    return {
      rows: shown,
      version: deviceState.version,
      state: deviceState,
      hasMore: false,
      empty: shown.length === 0 && answer.complete,
      loading: answer.loading,
      refusal: answer.refusal,
      revoked: isRevoking(answer.refusal),
      restarted: null,
      stale: false,
      more: () => undefined,
      restart: () => undefined,
      exhaust: () => undefined,
      refinement: refinementSurface(op),
      source: 'device',
      deviceFallback: null,
      deviceFallbackReason: null,
      complete: answer.complete,
      live: null,
    };
  }

  return {
    rows: state.rows,
    version: state.version,
    state,
    hasMore: state.hasMore,
    empty: state.empty && !state.hasMore,
    loading,
    refusal,
    revoked: isRevoking(refusal),
    restarted,
    stale: state.stale,
    more,
    restart,
    exhaust,
    refinement: refinementSurface(op),
    source: 'network',
    deviceFallback: answer.plan.use || answer.plan.why === 'no_device' ? null : answer.plan.why,
    deviceFallbackReason: answer.plan.use || answer.plan.why === 'no_device' ? null : answer.plan.detail,
    complete: true,
    live: signal.status,
  };
}
