/**
 * `useTenantStatus`: where the person's own tenant stands, kept up to date
 * until it is ready (owner, 2026-10-21).
 *
 * It reads `GET /auth/provisioning` with short pauses that grow to two
 * seconds, and every ten seconds once the setup is reported stuck. There is
 * no time limit, because the server never stops trying a setup. A read that
 * fails for a moment (busy, slow, unreachable) is read again after a growing
 * pause. Reading stops once the tenant is ready, or when it cannot be:
 * `failed`, `off`, `none`, or a refused session.
 *
 * What the app shows for each phase is the app developer's choice.
 * `TenantGate` is the same hook with a slot per phase.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { TokenSource } from '../client.js';
import {
  DEFAULT_STUCK_PAUSE_MS,
  readTenantStatus,
  retryTenantSetup,
  tenantPause,
  TenantStatusError,
  type TenantStatus,
} from '../tenant.js';

/** Where the setup stands, as a screen reads it. */
export type TenantPhase =
  /** No answer yet. */
  | 'checking'
  /** Being made. */
  | 'setting-up'
  /** Being made, past the deployment's `ORDO_PROVISION_STUCK_SECS`. Still tried. */
  | 'stuck'
  /** Made: `space` is its id. */
  | 'ready'
  /** It cannot be made now: see `status` and `problem`. `retry()` starts again. */
  | 'failed';

export interface UseTenantStatusOptions {
  /** The door's base URL. */
  readonly baseUrl: string;
  /** The person's token, or a function that gives the current one. */
  readonly token: TokenSource;
  /** Injectable for tests; defaults to the global. */
  readonly fetch?: typeof globalThis.fetch;
  /** `false` holds the hook still (no reads). Default `true`. */
  readonly enabled?: boolean;
  /** The pause between reads once stuck, in milliseconds. Default 10000. */
  readonly stuckPauseMs?: number;
  /** For tests: how to pause between reads. */
  readonly wait?: (ms: number) => Promise<void>;
}

export interface TenantStatusResult {
  readonly phase: TenantPhase;
  /** The last status the door gave, or `null` before the first. */
  readonly status: TenantStatus | null;
  /** The tenant's id once ready, else `null`. */
  readonly space: string | null;
  /**
   * The last read that failed. While reading goes on it is `unavailable`;
   * `refused`, `limit_reached` or `malformed` stop it and make the phase
   * `failed`. `null` after a good read.
   */
  readonly problem: TenantStatusError | null;
  /** Start the setup again (`POST /auth/provisioning`) and read on. */
  retry(): Promise<void>;
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

function phaseOf(status: TenantStatus | null, problem: TenantStatusError | null): TenantPhase {
  if (problem !== null && problem.problem !== 'unavailable') return 'failed';
  if (status === null) return 'checking';
  switch (status.state) {
    case 'ready':
      return status.space ? 'ready' : 'setting-up';
    case 'provisioning':
      return status.stuck ? 'stuck' : 'setting-up';
    default:
      return 'failed';
  }
}

/** Whether reading should stop after this status. */
function settled(status: TenantStatus): boolean {
  return status.state !== 'provisioning' && !(status.state === 'ready' && !status.space);
}

export function useTenantStatus(options: UseTenantStatusOptions): TenantStatusResult {
  const [status, setStatus] = useState<TenantStatus | null>(null);
  const [problem, setProblem] = useState<TenantStatusError | null>(null);
  // Bumped by `retry()`, which starts the reading again.
  const [round, setRound] = useState(0);
  // The token and fetch are read through a ref, so a new function each
  // render does not restart the reading.
  const latest = useRef(options);
  latest.current = options;
  const enabled = options.enabled ?? true;

  useEffect(() => {
    if (!enabled) return;
    let live = true;
    const run = async () => {
      let failedReads = 0;
      let stuck = false;
      for (let i = 0; live; i++) {
        const o = latest.current;
        try {
          const s = await readTenantStatus({ baseUrl: o.baseUrl, token: o.token, ...(o.fetch ? { fetch: o.fetch } : {}) });
          if (!live) return;
          failedReads = 0;
          stuck = s.stuck === true;
          setStatus(s);
          setProblem(null);
          if (settled(s)) return;
        } catch (e) {
          if (!live) return;
          const err = e instanceof TenantStatusError ? e : new TenantStatusError('unavailable', String(e));
          setProblem(err);
          if (err.problem !== 'unavailable') return;
          failedReads++;
        }
        const pause = tenantPause(i, stuck, failedReads, o.stuckPauseMs ?? DEFAULT_STUCK_PAUSE_MS);
        await (o.wait ?? sleep)(pause);
      }
    };
    void run();
    return () => {
      live = false;
    };
  }, [enabled, options.baseUrl, round]);

  const retry = useCallback(async () => {
    const o = latest.current;
    try {
      const s = await retryTenantSetup({ baseUrl: o.baseUrl, token: o.token, ...(o.fetch ? { fetch: o.fetch } : {}) });
      setStatus(s);
      setProblem(null);
    } catch (e) {
      setProblem(e instanceof TenantStatusError ? e : new TenantStatusError('unavailable', String(e)));
    }
    setRound((r) => r + 1);
  }, []);

  const phase = phaseOf(status, problem);
  return {
    phase,
    status,
    space: phase === 'ready' ? (status?.space ?? null) : null,
    problem,
    retry,
  };
}
