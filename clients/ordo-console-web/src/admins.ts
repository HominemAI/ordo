/**
 * The people who manage the deployment, its machine keys, and the
 * identity service's audit log (docs/plan-identity-service.md 5.6, 5.7, 7).
 *
 * The console relays each call to the identity service with the signed-in
 * person's own token (`crates/ordo-console/src/server/admins.rs`). A write
 * needs a fresh touch: run each through `withTouch`, one call each.
 */

import { get, post } from './api';

export interface Admin {
  id: string;
  displayName: string;
  role: string;
  createdAt: number;
  endedAt: number | null;
}

export interface AdminInvite {
  id: string;
  label: string;
  role: string;
  issuedBy: string;
  issuedAt: number | null;
  expiresAt: number | null;
  usedAt: number | null;
  usedBy: string | null;
  revokedAt: number | null;
  state: 'open' | 'used' | 'revoked' | 'expired';
}

export interface MachineKey {
  id: string;
  name: string;
  publicKey: string;
  createdBy: string;
  createdAt: number | null;
  lastUsedAt: number | null;
  endedAt: number | null;
}

export interface Admins {
  people: Admin[];
  invites: AdminInvite[];
  machineKeys: MachineKey[];
}

export interface AuditEntry {
  seq: number;
  at: number;
  actor: string;
  action: string;
  subject: string;
  outcome: string;
  client: string;
  detail: string;
}

export interface AuditPage {
  entries: AuditEntry[];
  verified: { ok: true; rows: number; checkpoints: number; at: number } | { ok: false; problem: string; at: number };
}

export const loadAdmins = () => get<Admins>('admins');
export const auditPage = (before?: number) =>
  get<AuditPage>(before === undefined ? 'admins/audit?limit=50' : `admins/audit?limit=50&before=${before}`);
export const inviteAdmin = (label: string) =>
  post<{ id: string; link: string; expiresAt: number }>('admins/invite', { label });
export const revokeInvite = (id: string) => post('admins/invite/revoke', { id });
export const endAdmin = (id: string) => post('admins/end', { id });
export const addMachineKey = (name: string) =>
  post<{ id: string; secret?: string; name?: string }>('admins/machine-keys/add', { name });
export const endMachineKey = (id: string) => post('admins/machine-keys/end', { id });
