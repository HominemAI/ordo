/**
 * The People screen's calls (docs/plan-console.md 4.5). The console server
 * reaches the tenant through its one client to the door, with the deployment
 * owner's relayed touch, and only where the owner holds a role there.
 */

import { get, post } from './api';

export type Role = 'owner' | 'manager' | 'editor' | 'viewer';
export const ROLES: Role[] = ['owner', 'manager', 'editor', 'viewer'];

/** One row of `std.space.members` or `std.space.groupMembers`. */
export interface Member {
  member: string;
  principal: string;
  name: string | null;
  /** Null for a guest's seat. */
  role: Role | null;
  status: string;
  joined: string | null;
  groups: string[];
}

/** One row of `std.space.groups`. */
export interface Group {
  group: string;
  name: string;
  members: string[];
}

/** One row of `std.space.grants`. */
export interface Grant {
  grant: string;
  kind: 'token' | 'share';
  principal: string;
  member: string | null;
  client: string | null;
  ops: string[];
  grantedAt: number;
  expires: number;
}

/** One way in: a row of `callableBy` (with `op`) or `callersOf` (with `principal`). */
export interface Via {
  op?: string;
  principal?: string;
  via: string;
  detail: string;
}

export interface PeopleOf {
  tenant: string;
  members: Member[];
  groups: Group[];
  grants: Grant[];
}

const q = encodeURIComponent;

export const listPeople = (tenant: string) => get<PeopleOf>(`people?tenant=${q(tenant)}`);
export const groupMembers = (tenant: string, group: string) =>
  get<{ group: string; members: Member[] }>(`people/group?tenant=${q(tenant)}&group=${q(group)}`);
export const callableBy = (tenant: string, member: string) =>
  get<{ member: string; ops: Via[] }>(`people/callable?tenant=${q(tenant)}&member=${q(member)}`);
export const callersOf = (tenant: string, op: string) =>
  get<{ op: string; callers: Via[] }>(`people/callers?tenant=${q(tenant)}&op=${q(op)}`);

export const invite = (tenant: string, role: string, expires: string, label: string) =>
  post<{ invited: { ticket: string; expiresAt: number; role: string | null } }>('people/invite', {
    tenant,
    ...(role ? { role } : {}),
    ...(expires ? { expires } : {}),
    ...(label ? { label } : {}),
  });
export const setRole = (tenant: string, member: string, role: string) =>
  post('people/set-role', { tenant, member, role });
export const removeMember = (tenant: string, member: string) => post('people/remove', { tenant, member });
export const addToGroup = (tenant: string, group: string, member: string) =>
  post<{ added: { outcome: string } }>('people/add-to-group', { tenant, group, member });

// Step 8b: the rest of the People screen.

/** One row of `std.space.invitations`: by id, never the ticket. */
export interface Invitation {
  id: string;
  role: string | null;
  for: string | null;
  label: string | null;
  issuedBy: string;
  issuedAt: number;
  expiresAt: number;
  status: 'open' | 'declined';
  declinedAt: number | null;
}

/** One row of `std.space.profile`: who may call one op in this tenant. */
export interface OpAccess {
  op: string;
  roles: string[];
  groups: string[];
  allow: string[];
  deny: string[];
  /** Whether the tenant set this line itself. */
  changed: boolean;
}

/** `std.space.admission`: the ways a person who is not a member gets in. */
export interface Admission {
  open: boolean;
  role: string;
  providers: string;
  firstComer: boolean;
  secondFactor: string;
  presenceFrom: string;
  ceiling: string;
}

/** The four ways in the console changes; unset leaves one alone. */
export interface AdmissionChange {
  open?: boolean;
  role?: string;
  providers?: string;
  firstComer?: boolean;
}

/** What `std.space.setRecoveryPolicy` takes; unset leaves one alone. */
export interface RecoveryChange {
  hold?: boolean | undefined;
  delaySecs?: number | undefined;
  approvals?: number | undefined;
  pathEnabled?: boolean | undefined;
  codesEnabled?: boolean | undefined;
  managersMayRecover?: boolean | undefined;
}

export interface RecoverySettings {
  hold: boolean;
  delaySecs: number;
  approvals: number;
  pathEnabled: boolean;
  codesEnabled: boolean;
  managersMayRecover: boolean;
}

/** A member's recovery: a case, and a ticket once it matured. */
export interface MemberRecovery {
  case: string;
  member: string;
  state?: 'matured' | 'pending';
  maturesAt?: number;
  approvalsRequired?: number;
  ticket: string | null;
  ticketExpiresAt: number | null;
  outcome: string;
}

export const suspend = (tenant: string, member: string) => post('people/suspend', { tenant, member });
export const reinstate = (tenant: string, member: string) => post('people/reinstate', { tenant, member });

export const createGroup = (tenant: string, name: string) =>
  post<{ created: { group: string; name: string } }>('people/create-group', { tenant, name });
export const deleteGroup = (tenant: string, group: string) => post('people/delete-group', { tenant, group });
export const removeFromGroup = (tenant: string, group: string, member: string) =>
  post('people/remove-from-group', { tenant, group, member });

export const profile = (tenant: string, op = '') =>
  get<{ ops: OpAccess[] }>(`people/profile?tenant=${q(tenant)}${op ? `&op=${q(op)}` : ''}`);
export const setOpAccess = (tenant: string, op: string, roles: string[], groups: string[]) =>
  post('people/set-op-access', { tenant, op, roles: roles.join(','), groups: groups.join(',') });
export const resetOpAccess = (tenant: string, op: string) => post('people/reset-op-access', { tenant, op });
export const setMemberAccess = (tenant: string, member: string, op: string, effect: 'allow' | 'deny' | 'default') =>
  post('people/set-member-access', { tenant, member, op, effect });

export const invitations = (tenant: string) =>
  get<{ invitations: Invitation[] }>(`people/invitations?tenant=${q(tenant)}`);
export const revokeInvitation = (tenant: string, id: string) => post('people/revoke-invitation', { tenant, id });

export const admission = (tenant: string) => get<{ admission: Admission }>(`people/admission?tenant=${q(tenant)}`);
export const setAdmission = (tenant: string, change: AdmissionChange) =>
  post<{ admission: Admission }>('people/set-admission', { tenant, ...change });

export const recoverMember = (tenant: string, member: string, recoveryCase = '') =>
  post<{ recovery: MemberRecovery }>('people/recover-member', {
    tenant,
    member,
    ...(recoveryCase ? { case: recoveryCase } : {}),
  });
export const approveRecovery = (tenant: string, member: string, recoveryCase: string) =>
  post<{ approval: { approvals: number; approvalsRequired: number; outcome: string } }>('people/approve-recovery', {
    tenant,
    member,
    case: recoveryCase,
  });
export const setRecoveryPolicy = (tenant: string, change: RecoveryChange) =>
  post<{ policy: RecoverySettings }>('people/recovery-policy', { tenant, ...change });

/** What a via means, in words. */
export function viaWords(v: Via): string {
  switch (v.via) {
    case 'role':
      return `as ${v.detail}`;
    case 'group':
      return `through the group ${v.detail}`;
    case 'override':
      return 'allowed for this person alone';
    case 'token':
      return `by an app's grant, ${v.detail}`;
    case 'delegation':
      return v.detail;
    default:
      return `${v.via} ${v.detail}`;
  }
}
