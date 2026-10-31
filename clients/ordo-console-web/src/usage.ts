/**
 * The Usage screen's calls (docs/plan-console.md 4.9 and 4.11). The console
 * server reaches the deployment for each through its one client to the door.
 */

import { get, post } from './api';

/** One period's numbers, or a total. */
export interface Numbers {
  requests: number;
  rowsReturned: number;
  writes: number;
  rowsWritten: number;
  /** What was stored at the end of the period, in bytes. */
  bytes: number;
  end: number;
}

export interface Period extends Numbers {
  start: number;
}

export interface TenantShare extends Numbers {
  id: string;
  name: string;
}

export interface Gap {
  space: string;
  start: number;
  end: number;
  reason: string;
}

export interface UsageReading {
  scope: 'tenant' | 'deployment';
  tenant: string | null;
  from: number;
  to: number;
  periods: Period[];
  total: Numbers;
  byTenant: TenantShare[];
  gaps: Gap[];
  /** Tenants the door would not answer for, and why. */
  missing: { id: string; name: string; error: string; message: string }[];
  /** Tenants past the console's limit, left out of the reading. */
  leftOut: number;
  /** The deployment's own space, when the reading covers it. */
  home: string | null;
}

export const readUsage = (from: number, to: number, tenant: string) =>
  get<UsageReading>(`usage?from=${from}&to=${to}${tenant ? `&tenant=${encodeURIComponent(tenant)}` : ''}`);

/** The quantities a quota counts (ordo-host `quotas::QUANTITIES`). */
export const QUANTITIES = ['requests', 'rows_returned'] as const;
export type Quantity = (typeof QUANTITIES)[number];

export interface Quota {
  space: string;
  quantity: Quantity;
  limit: number;
  grace: number;
  periodSecs: number;
  periodStart: number;
  /** Used this period, as flushed by every node. */
  used: number;
  /** False until the deployment counts usage (ORDO_USAGE_LEDGER). */
  enforced: boolean;
  note?: string;
}

export const listQuotas = () => get<{ quotas: Quota[] }>('usage/quotas');

export interface QuotaInput {
  space: string;
  quantity: Quantity;
  limit: number;
  grace: number;
  periodSecs: number;
}

export const setQuota = (q: QuotaInput) => post<{ quota: Quota }>('usage/quotas/set', q);

/** One op's observed cost on one node. */
export interface SlowOp {
  op: string;
  calls: number;
  errors: number;
  totalMicros: number;
  maxMicros: number;
  meanMicros: number;
  rowsReturned: number;
  registeredClass: string;
  effectiveClasses: Record<string, number>;
  /** The cost class most of its calls ran at. */
  tier: string | null;
}

export interface SlowOps {
  ops: SlowOp[];
  node: string | null;
  since: number | null;
}

export const slowOps = () => get<SlowOps>('usage/slow-ops');

/** A duration in microseconds, in the unit that reads best. */
export function duration(micros: number): string {
  if (micros < 1000) return `${micros} µs`;
  if (micros < 1_000_000) return `${(micros / 1000).toFixed(micros < 10_000 ? 1 : 0)} ms`;
  return `${(micros / 1_000_000).toFixed(2)} s`;
}

/** A period length in seconds, in words. */
export function period(secs: number): string {
  if (secs % 86_400 === 0) return secs === 86_400 ? 'a day' : `${secs / 86_400} days`;
  if (secs % 3600 === 0) return secs === 3600 ? 'an hour' : `${secs / 3600} hours`;
  if (secs % 60 === 0) return secs === 60 ? 'a minute' : `${secs / 60} minutes`;
  return `${secs} seconds`;
}

export const count = (n: number) => n.toLocaleString();
