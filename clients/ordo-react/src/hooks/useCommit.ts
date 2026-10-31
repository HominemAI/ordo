/**
 * `useCommit(op)` — the CAS-by-default write path as a state machine.
 *
 * The state a consumer must handle is `conflict`, and the API makes that
 * unavoidable: there is no code path from `conflict` back to `committed` that
 * does not go through the consumer supplying a fresh `rev`. `resolve()`
 * refetches; `retry()` needs the new revision; neither happens on its own.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { OrdoNetworkError } from '@hominemai/ordo-sdk/core/network.js';
import type { OpResponse, Params, Row } from '../client.js';
import {
  commitOnce,
  retryAfterConflict,
  type CasConflict,
  type CommitInput,
  type ConflictLookup,
  type CommitOutcome,
} from '../commit.js';
import { deriveForm, type FormModel } from '../form.js';
import type { PlatformOpName, PlatformRows } from '../platform.js';
import { OrdoLocalRefusal, type Refusal } from '../refusal.js';
import { useOrdoRuntime } from './context.js';

/**
 * What a conflict knows of the row it lost against. `not_fetched` is before
 * {@link UseCommit.resolve} ran (or `autoResolve` did), `fetching` while it
 * runs. Then exactly one of `found` (see `current`), `deleted` (the row is
 * gone, so this is a delete conflict and `current` stays `null`) and
 * `unavailable` (the catalog grants no read for it, or the read failed, so
 * what is stored cannot be known). `current: null` alone says none of these.
 */
export type ConflictCurrent = 'not_fetched' | 'fetching' | 'found' | 'deleted' | 'unavailable';

export type CommitState<R = Row> =
  | { readonly status: 'idle' }
  | { readonly status: 'committing' }
  | { readonly status: 'committed'; readonly response: OpResponse<R> }
  | {
      readonly status: 'conflict';
      readonly conflict: CasConflict;
      readonly current: Row | null;
      readonly currentIs: ConflictCurrent;
    }
  | { readonly status: 'refused'; readonly refusal: Refusal };

export interface UseCommit<R = Row> {
  readonly model: FormModel;
  readonly state: CommitState<R>;
  readonly committing: boolean;
  /** The op's refusal set, verbatim from the descriptor. */
  readonly refusals: readonly string[];
  readonly refusal: Refusal | null;
  readonly conflict: CasConflict | null;
  /** True when this op declares `rev`, so a commit needs one. */
  readonly cas: boolean;
  readonly commit: (values: Params, rev?: number) => Promise<CommitState<R>>;
  /** Refetch the row the write lost against; parks the fresh row in state. */
  readonly resolve: () => Promise<Row | null>;
  /** Retry the merged values against the refetched revision. */
  readonly retry: (values: Params, rev: number) => Promise<CommitState<R>>;
  readonly reset: () => void;
}

export interface UseCommitOptions {
  /**
   * Read the current row back as soon as a conflict arrives, so
   * `state.currentIs` moves from `not_fetched` to an answer without the
   * caller asking. Off by default: `resolve()` is the explicit verb.
   */
  readonly autoResolve?: boolean;
}

/** A platform op (`std.space.share` and the rest) is typed by its name. */
export function useCommit<N extends PlatformOpName>(opName: N, options?: UseCommitOptions): UseCommit<PlatformRows[N]>;
export function useCommit<R = Row>(opName: string, options?: UseCommitOptions): UseCommit<R>;
export function useCommit<R = Row>(opName: string, options: UseCommitOptions = {}): UseCommit<R> {
  const { client, device, stepUp } = useOrdoRuntime();
  const op = useMemo(() => client.iface.requireOp(opName), [client, opName]);
  const model = useMemo(() => deriveForm(client.iface, opName), [client, opName]);
  const [state, setState] = useState<CommitState<R>>({ status: 'idle' });

  /**
   * Turn the runtime's outcome into state, and tell the device copy when a
   * write landed so its rows are read again, the way the SDK's notes example
   * reloads after a write. A write is never applied to the device by this
   * package: the server's answer is the only one.
   */
  const settle = useCallback(
    async (attempt: () => Promise<CommitOutcome<R>>): Promise<CommitState<R>> => {
      setState({ status: 'committing' });
      let outcome: CommitOutcome<R>;
      try {
        outcome = await attempt();
      } catch (e) {
        // No network: the client names it `unreachable` (or `blocked`), and a
        // bare `fetch` rejects with a TypeError. Nothing was sent. Writes are
        // not queued (`plan-client-cache.md` §6), so the write is refused and
        // the caller keeps what it typed. A `timeout` may have landed, so it
        // is thrown as it came; anything else is a bug.
        const unsent = e instanceof OrdoNetworkError ? e.kind !== 'timeout' : e instanceof TypeError;
        if (!unsent) throw e;
        const refusal = new OrdoLocalRefusal({
          class: 'internal',
          message:
            `${op.op} was not sent: the network is unavailable. Writes made offline are refused, ` +
            `not queued, so nothing was saved and the values are still the caller's`,
          op: op.op,
          param: null,
        }).refusal;
        const refused: CommitState<R> = { status: 'refused', refusal };
        setState(refused);
        return refused;
      }
      const next: CommitState<R> =
        outcome.status === 'committed'
          ? { status: 'committed', response: outcome.response }
          : outcome.status === 'conflict'
            ? { status: 'conflict', conflict: outcome.conflict, current: null, currentIs: 'not_fetched' }
            : { status: 'refused', refusal: outcome.refusal };
      setState(next);
      if (next.status === 'committed' && device) void Promise.resolve(device.afterCommit()).catch(() => undefined);
      return next;
    },
    [op, device],
  );

  /**
   * A platform write wants a fresh touch, and the door's `access_denied`
   * cannot say "too old" from "not allowed". Ask once, try once more, and
   * believe the second answer.
   */
  const withPresence = useCallback(
    async (attempt: () => Promise<CommitOutcome<R>>): Promise<CommitOutcome<R>> => {
      const first = await attempt();
      if (!stepUp || !op.platform || first.status !== 'refused' || first.refusal.class !== 'access_denied') {
        return first;
      }
      await stepUp();
      return attempt();
    },
    [stepUp, op],
  );

  const run = useCallback(
    (input: CommitInput): Promise<CommitState<R>> =>
      settle(() => withPresence(() => commitOnce<R>(client, opName, input, model))),
    [client, opName, settle, withPresence, model],
  );

  const commit = useCallback(
    (values: Params, rev?: number) => run(rev === undefined ? { values } : { values, rev }),
    [run],
  );

  const resolve = useCallback(async (): Promise<Row | null> => {
    if (state.status !== 'conflict') return null;
    const { conflict } = state;
    setState({ status: 'conflict', conflict, current: null, currentIs: 'fetching' });
    let found: ConflictLookup;
    try {
      found = await conflict.lookup();
    } catch (e) {
      setState({ status: 'conflict', conflict, current: null, currentIs: 'unavailable' });
      throw e;
    }
    const current = found.status === 'found' ? found.row : null;
    setState({ status: 'conflict', conflict, current, currentIs: found.status });
    return current;
  }, [state]);

  const auto = options.autoResolve === true;
  const asked = useRef<CasConflict | null>(null);
  useEffect(() => {
    if (!auto || state.status !== 'conflict' || state.currentIs !== 'not_fetched') return;
    if (asked.current === state.conflict) return;
    asked.current = state.conflict;
    // A failed read leaves `currentIs: 'unavailable'`; there is nothing more to do with the error.
    void resolve().catch(() => undefined);
  }, [auto, state, resolve]);

  const retry = useCallback(
    async (values: Params, rev: number): Promise<CommitState<R>> => {
      if (state.status !== 'conflict') return commit(values, rev);
      return settle(() => retryAfterConflict<R>(client, opName, state.conflict, { values, rev }));
    },
    [client, opName, state, commit, settle],
  );

  return {
    model,
    state,
    committing: state.status === 'committing',
    refusals: op.refusals,
    refusal: state.status === 'refused' ? state.refusal : null,
    conflict: state.status === 'conflict' ? state.conflict : null,
    cas: model.revField !== null,
    commit,
    resolve,
    retry,
    reset: useCallback(() => setState({ status: 'idle' }), []),
  };
}
