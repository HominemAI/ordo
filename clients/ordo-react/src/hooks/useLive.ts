/**
 * `useLive` — the live half of a read hook, shared by all of them.
 *
 * It answers three things: is this hook following the feed, has the stream
 * opened yet (so the first fetch is safe to make), and how many paced
 * invalidations has it seen (the number a hook puts in its effect deps to
 * refetch).
 *
 * Whether a hook follows is decided here and nowhere else:
 *
 * - the hook's own `live` option, when given;
 * - else the provider's `live`, which is off unless the app turned it on;
 * - and never for an op whose descriptor says `call.live` is false, because
 *   the door would refuse it with `not_live`.
 */

import { useEffect, useMemo, useRef, useState } from 'react';
import type { Params } from '../client.js';
import type { OpDescriptor } from '../descriptor.js';
import type { Refine } from '../refine.js';
import { createPacer, effectivePaceMs, type LiveStatus } from '../live.js';
import { useOrdoRuntime } from './context.js';

export interface UseLiveOptions {
  /** `true` to follow, `false` to not, omitted for the provider's setting. */
  readonly live?: boolean | undefined;
  /**
   * Minimum milliseconds between this hook's refetches. The provider's
   * `live.paceMs` is a floor: a hook may wait longer than it, never less.
   */
  readonly livePaceMs?: number | undefined;
}

export interface LiveSignal {
  /** Whether this hook is following the feed at all. */
  readonly following: boolean;
  /**
   * False while the stream is still opening. A hook does not wait for it: it
   * fetches at once. Always true for a hook that is not following.
   */
  readonly ready: boolean;
  /**
   * True once this read has been accepted on a stream, and for good after.
   * A hook whose first fetch started while this was false fetches once more
   * when it turns true, so a change between the two is not missed. Always
   * true for a hook that is not following.
   */
  readonly opened: boolean;
  /** Bumped once per paced invalidation. Put it in an effect's deps. */
  readonly tick: number;
  readonly status: LiveStatus | null;
}

const OFF: LiveSignal = { following: false, ready: true, opened: true, tick: 0, status: null };

/**
 * `refine` is the hook's refinement in its wire form. It rides with the read
 * on the shared stream (R6), so the door checks it, and a hook with another
 * refinement of the same op is another read. Signals are per op either way.
 */
export function useLive(
  op: OpDescriptor,
  params: Params | undefined,
  active: boolean,
  options: UseLiveOptions,
  refine?: Refine,
): LiveSignal {
  const runtime = useOrdoRuntime();
  const wants = options.live ?? runtime.live;
  const capable = op.call.live && op.kind === 'read';
  const following = wants && capable && active;
  const paceMs = effectivePaceMs(runtime.paceMs, options.livePaceMs);

  const [tick, setTick] = useState(0);
  const key = useMemo(
    () => `${op.op}?${JSON.stringify(params ?? null)}#${JSON.stringify(refine ?? null)}`,
    [op.op, params, refine],
  );
  // The status is stored with the subscription it belongs to, so a change of
  // params cannot be read as "open" on the strength of the previous stream.
  const [seen, setSeen] = useState<{ readonly key: string; readonly status: LiveStatus } | null>(null);
  const [openedFor, setOpenedFor] = useState<string | null>(null);
  const status = seen !== null && seen.key === key ? seen.status : null;
  const paramsRef = useRef(params);
  paramsRef.current = params;
  const refineRef = useRef(refine);
  refineRef.current = refine;

  useEffect(() => {
    if (!following) {
      setSeen(null);
      setOpenedFor(null);
      return;
    }
    const pacer = createPacer(() => setTick((t) => t + 1), paceMs);
    const stop = runtime.hub.subscribe(
      op.op,
      paramsRef.current ?? {},
      {
        invalidate: () => pacer.request(),
        status: (s) => {
          setSeen({ key, status: s });
          if (s.state === 'open') setOpenedFor(key);
        },
      },
      refineRef.current,
    );
    return () => {
      pacer.cancel();
      stop();
    };
  }, [runtime.hub, op.op, key, following, paceMs]);

  if (!following) return OFF;
  // `ready` only informs; no hook holds a fetch for it. A stream that is slow,
  // unsupported or failed must not stall the read it was meant to keep fresh.
  return {
    following,
    ready: status !== null && status.state !== 'connecting',
    opened: openedFor === key,
    tick,
    status,
  };
}
