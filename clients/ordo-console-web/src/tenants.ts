/**
 * The Tenants screen's calls (docs/plan-console.md 4.4). The console server
 * reaches the deployment for each through its one client to the door.
 */

import { get, post } from './api';

/** The last rolled-up usage period of a tenant. */
export interface Usage {
  periodStart: number;
  periodEnd: number;
  writes: number;
  rowsWritten: number;
  bytesSealed: number;
  bytesChain: number;
  bytesBlob: number;
  bytesRowsEstimated: number;
}

export type TenantStatus = 'provisioning' | 'active' | 'paused' | 'failed' | 'reaped';

/** One row of `std.deployment.tenants`. */
export interface Tenant {
  id: string;
  name: string;
  status: TenantStatus;
  phase: string;
  createdAt: number;
  stateAt: number;
  own: boolean;
  note: string | null;
  pausedAt: number | null;
  pausedBy: 'deployment' | 'tenant' | null;
  pausedNote: string | null;
  target: string | null;
  custody: string | null;
  members: number | null;
  usage: Usage | null;
  /** Why the deployment holds it closed, on the node that answered. */
  quarantined: string | null;
  /** The role the deployment owner holds in it, if any. */
  yourRole: string | null;
}

export interface TenantList {
  tenants: Tenant[];
  /** Set when the deployment's readiness list could not be read. */
  readyError: string | null;
}

/** The closed list a pause is recorded with (ordo-host `PAUSE_NOTES`). */
export const PAUSE_NOTES = ['owner', 'billing', 'abuse', 'legal', 'maintenance', 'other'] as const;

export const listTenants = (): Promise<TenantList> => get<TenantList>('tenants');

export const createTenant = (name: string, owner: string) =>
  post<{ created: { space: string; name: string; owner: string } }>('tenants/create', {
    name,
    ...(owner ? { owner } : {}),
  });

export const pauseTenant = (tenant: string, note: string) => post('tenants/pause', { tenant, note });
export const resumeTenant = (tenant: string) => post('tenants/resume', { tenant });
export const reapTenant = (tenant: string) => post('tenants/reap', { tenant });
export const exportTenant = (tenant: string) =>
  post<{ exported: { bundle: string; rows: number } }>('tenants/export', { tenant });
export const deleteTenant = (tenant: string, confirm: string) => post('tenants/delete', { tenant, confirm });

export interface BucketInput {
  endpoint: string;
  region: string;
  bucket: string;
  credential: string;
  platformSealed: boolean;
}

export const attachBucket = (tenant: string, b: BucketInput) => post('tenants/attach-bucket', { tenant, ...b });
export const detachBucket = (tenant: string) => post('tenants/detach-bucket', { tenant });

/** One objection a tenant's manager made to a case, and its resolution. */
export interface Objection {
  objector: string;
  tenant: string;
  at: number;
  resolvedAt: number | null;
  resolution: 'resume' | 'cancel' | null;
  reason: string | null;
}

/** One open case of `std.identity.openCases`: a succession, or a recovery through the door. */
export interface OwnerCase {
  case: string;
  path: 'succession' | 'operator';
  /** The tenant of a succession; empty for a recovery. */
  tenant: string;
  principal: string;
  evidence: string;
  reference: string;
  openedAt: number;
  maturesAt: number;
  state: 'pending' | 'matured';
  /** Past its delay and not paused: it may be completed. */
  mature: boolean;
  /** A manager objected and nobody has resolved it. */
  paused: boolean;
  approvals: number;
  approvalsRequired: number;
  departed: string[];
  objections: Objection[];
}

/** The closed list of evidence a case names (ordo-host `identity_ops::EVIDENCE`). */
export const EVIDENCE = ['vouched-by-members', 'verified-out-of-band', 'contract-holder', 'host-break-glass'] as const;

/** The reasons each decision on an objection may give (ordo-host `recovery`). */
export const RESOLVE_REASONS = {
  resume: ['evidence-rechecked', 'objector-withdrew', 'objector-answered'],
  cancel: ['objection-upheld', 'opened-in-error', 'not-verified'],
} as const;

export interface Succeeded {
  case: string;
  tenant: string;
  principal: string;
  endsGrants: boolean;
  grantsEnded: string[];
  delegationsEnded: string[];
  notEnded: string[];
}

export const listCases = () => get<{ cases: OwnerCase[] }>('tenants/successions');
export const openSuccession = (
  tenant: string,
  principal: string,
  departed: string[],
  evidence: string,
  reference: string,
) =>
  post<{ opened: { case: string; maturesAt: number } }>('tenants/succession/open', {
    tenant,
    principal,
    departed,
    evidence,
    ...(reference ? { reference } : {}),
  });
export const completeSuccession = (tenant: string, c: string, endGrants: boolean) =>
  post<{ succeeded: Succeeded }>('tenants/succession/complete', { tenant, case: c, endGrants });
export const resolveObjection = (c: string, decision: 'resume' | 'cancel', reason: string) =>
  post('tenants/succession/resolve', { case: c, decision, reason });

/** Bytes as a short size. */
export function size(bytes: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let n = bytes;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i++;
  }
  return `${i === 0 ? n : n.toFixed(1)} ${units[i]}`;
}

/** Everything a tenant stores, from its last usage period. */
export function stored(u: Usage): number {
  return u.bytesSealed + u.bytesChain + u.bytesBlob + u.bytesRowsEstimated;
}
