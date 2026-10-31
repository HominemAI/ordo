/**
 * The Shares and links screen's calls and the Security log's
 * (docs/plan-console.md 4.7 and 4.8). The console server reaches the
 * deployment for each through its one client to the door.
 */

import { get, post } from './api';

export type LinkState = 'live' | 'expired' | 'used-up' | 'revoked';

/** One row of `std.space.links`, with what the console adds. */
export interface ShareLink {
  id: string;
  label: string;
  ops: string[];
  opens: number;
  /** Distinct devices that opened it: what a limit on opens counts. */
  devices: number;
  maxOpens: number | null;
  createdAt: number;
  expiresAt: number;
  state: LinkState;
  issuedBy: string;
  hasPassword: boolean;
  /** Locked after wrong passwords, until then. */
  lockedUntil: number | null;
  locked: boolean;
}

/** One row of `std.space.shares`. */
export interface Share {
  seat: string | null;
  displayName: string | null;
  guest: boolean;
  ops: string[];
  grantedAt: number;
  expiresAt: number;
}

export interface TenantShares {
  tenant: string;
  links: ShareLink[];
  shares: Share[];
}

export const listShares = (tenant: string) => get<TenantShares>(`shares?tenant=${encodeURIComponent(tenant)}`);
export const revokeLink = (tenant: string, link: string) => post('shares/revoke-link', { tenant, link });
export const revokeShare = (tenant: string, seat: string) => post('shares/revoke-share', { tenant, seat });

/** Every kind the deployment's record spells (ordo-host `security_record::KINDS`). */
export const KINDS = [
  'denied',
  'authenticated',
  'authentication-failed',
  'step-up-demanded',
  'step-up-satisfied',
  'key-released',
  'membership-suspended',
  'membership-reactivated',
  'custody-action',
  'authenticator-linked',
  'authenticator-unlinked',
  'principal-onboarded',
  'suppressed',
  'events-dropped',
  'schema-migration',
  'enrolment',
  'sessions-ended',
  'recovery',
  'second-factor',
  'session-handoff',
  'upstream-claim',
  'sealed-unreadable',
  'console-key',
  'deployment-booted',
  'deploy',
  'secret',
] as const;

export type Source = 'deployment' | 'tenant' | 'console';

/** One entry of a security record, deployment's or a tenant's. */
export interface SecurityEvent {
  index: number;
  kind: string;
  act?: string | null;
  detail?: string;
  decisions?: string[] | null;
  subject: string;
  op: string;
  tenant?: string | null;
  firstAt: number;
  lastAt: number;
  count: number;
}

/** One row of the console's own audit log. */
export interface AuditEvent {
  id: number;
  at: number;
  actor: string;
  action: string;
  outcome: string;
  detail: string;
}

export interface SecurityPage {
  source: Source;
  events: (SecurityEvent | AuditEvent)[];
  /** Newest first: the cursor for the next, older page. */
  before?: number | null;
  /** A tenant's record, oldest first: the cursor for the next page. */
  from?: number | null;
}

export interface SecurityQuery {
  source: Source;
  kind?: string | undefined;
  tenant?: string | undefined;
  action?: string | undefined;
  before?: number | undefined;
  from?: number | undefined;
  limit?: number;
}

export function readSecurity(q: SecurityQuery): Promise<SecurityPage> {
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(q)) {
    if (v !== undefined && v !== '') params.set(k, String(v));
  }
  return get<SecurityPage>(`security?${params.toString()}`);
}

/** A time as a short local date and time. */
export function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}
