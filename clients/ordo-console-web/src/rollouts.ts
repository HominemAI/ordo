/**
 * The Rollouts and Moves screens' calls: the console's `rollouts` and
 * `moves` routes, which reach the deployment's `std.deployment.rollout.*`,
 * `rollbackCheck`, `closeSoak` and `revertSoak` (docs/plan-versions.md 3.7,
 * increments 7, 10, 10c and 11a). The moves themselves come from
 * `std.deployment.tenants` (`move`), on the Tenants route.
 */

import { get, post } from './api';

export type Strategy = 'rolling' | 'all' | 'canary' | 'group' | 'query';
export const STRATEGIES: Strategy[] = ['rolling', 'all', 'canary', 'group', 'query'];

export type RolloutState = 'running' | 'paused' | 'finished' | 'aborted';
export type TenantState = 'queued' | 'waiting' | 'moving' | 'moved' | 'failed' | 'skipped';

/** One tenant's row in a rollout. */
export interface RolloutTenant {
  tenant: string;
  state: TenantState;
  /** Why it waits, failed or was skipped. */
  reason: string;
  at: number;
  /** In a staged rollout's first stage (canary, group, query). */
  firstStage: boolean;
}

/** `std.deployment.rollout.status`: the open rollout, or the newest. `id` is null when there is none. */
export interface Rollout {
  id: number | null;
  target?: string;
  from?: string | null;
  strategy?: Strategy;
  /** Seconds. */
  batchInterval?: number;
  window?: number;
  batchSize?: number;
  failureThreshold?: number;
  deploymentSpace?: 'first' | 'last';
  state: RolloutState | null;
  reason?: string;
  actor?: string;
  startedAt?: number;
  endedAt?: number | null;
  lastBatchAt?: number | null;
  batches?: number;
  counts?: Partial<Record<TenantState, number>>;
  /** The deployment's own space's step. */
  deploymentStep?: { state: TenantState; reason: string; at: number } | null;
  tenants: RolloutTenant[];
  canaryCount?: number;
  canaryWait?: number;
  healthErrorRate?: string;
  alertChannel?: string;
  selection?: string;
  /** '' with no first stage, else `pending` or `passed`. */
  gate?: '' | 'pending' | 'passed';
  firstDoneAt?: number | null;
  health?: { calls: number; faults: number; healthy: boolean };
}

/** What the start form sends. Blank fields take the deployment's rollout settings. */
export interface StartInput {
  version: string;
  strategy?: Strategy;
  batchInterval?: string;
  batchSize?: number;
  window?: string;
  failureThreshold?: number;
  canaryCount?: number;
  canaryWait?: string;
  healthErrorRate?: string;
  group?: string;
  query?: string;
  order?: string;
  limit?: number;
}

export interface Held {
  tenant: string;
  held: boolean;
  reason: string | null;
  actor: string | null;
  at: number | null;
}

/** `std.deployment.rollbackCheck`: a rollback's dry run. Changes nothing. */
export interface RollbackCheck {
  version: string;
  active: string | null;
  /** One line per external or peer table the version can no longer read. */
  refusals: string[];
  notes: string[];
  ok: boolean;
  /** Each tenant not on the version, and what a rollback does to it. */
  tenants: { tenant: string; on: string | null; kept: boolean; note: string }[];
}

/** What a soak op answers. */
export interface SoakDone {
  tenant: string;
  from: string;
  to: string;
  phase?: string;
}

export const rolloutStatus = (): Promise<Rollout> => get<Rollout>('rollouts');
export const startRollout = (s: StartInput): Promise<Rollout> => post<Rollout>('rollouts/start', s);
export const resumeRollout = (): Promise<Rollout> => post<Rollout>('rollouts/resume');
export const abortRollout = (reason: string): Promise<Rollout> => post<Rollout>('rollouts/abort', { reason });
export const finishRollout = (reason: string): Promise<Rollout> =>
  post<Rollout>('rollouts/finish', reason ? { reason } : {});
export const skipTenant = (tenant: string, reason: string) =>
  post<{ rollout: number; tenant: string; state: TenantState; reason: string }>('rollouts/skip', {
    tenant,
    ...(reason ? { reason } : {}),
  });
export const retryTenant = (tenant: string) =>
  post<{ tenant: string; rollout: number | null; move: { to: string } | null }>('rollouts/retry', { tenant });
export const holdTenant = (tenant: string, reason: string): Promise<Held> =>
  post<Held>('rollouts/hold', { tenant, reason });
export const unholdTenant = (tenant: string): Promise<Held> => post<Held>('rollouts/hold', { tenant, clear: true });

export const rollbackCheck = (version: string): Promise<RollbackCheck> =>
  get<RollbackCheck>(`rollouts/check?version=${encodeURIComponent(version)}`);

export const closeSoak = (tenant: string): Promise<SoakDone> => post<SoakDone>('moves/close-soak', { tenant });
export const revertSoak = (tenant: string): Promise<SoakDone> => post<SoakDone>('moves/revert-soak', { tenant });

/** Seconds as words: `1 h 30 min`. */
export function secsWords(secs: number | undefined): string {
  if (secs === undefined) return '';
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  const parts = [h ? `${h} h` : '', m ? `${m} min` : '', s ? `${s} s` : ''].filter(Boolean);
  return parts.length ? parts.join(' ') : '0 s';
}
