/**
 * Platform ops: the sharing ops the door serves beside the program's own.
 *
 * The delegation ops (`std.delegation.review`, `approve`, `decline`, `mine`
 * and `revoke`) are here too: the person's side of the consent ceremony.
 * `std.space.share`, `shareLink`, `setLinkPassword`, `setLinkMaxOpens`, `revokeShare`, `revokeLink`, `links`,
 * `shares`, `invite`, `invitations`, `revokeInvitation`, `setRole`, the
 * membership ops (`admit`, `suspend`, `reinstate`, `remove`, `leave`,
 * `members`) and the access and group ops (`profile`, `setOpAccess`, `groups` and the rest) are not
 * ops of the program, so `interface.json` does not carry them
 * and `GET /interface` does not list them (it lists the program's manifests
 * the token may call). A hook needs a descriptor to call an op, so this module
 * holds one for each, small and typed, mirroring `share_specs` and `member_specs` in
 * `crates/ordo-host/src/platform_ops.rs`, `specs` in `people_ops.rs` and the link specs in `share_links.rs`.
 *
 * ## Why a table in the package, and why it is opt-in
 *
 * Three ways were open: fetch the door's descriptor (it does not carry them,
 * and carrying them would be a server change), mint a platform descriptor on
 * the server (the same), or keep a table here. The table is the one that needs
 * no server change. It keeps the property the program descriptor has, that a
 * hook reaches only what the token may call, by two rules:
 *
 * 1. The table is a catalog of what exists, never evidence of what may be
 *    called. The door authorises every call, and a token that may not share
 *    (a guest's window, a share link, a member without the role) is refused
 *    `access_denied`, which the hook shows as a typed `refusal`.
 * 2. An app names the platform ops it uses: `withPlatformOps(iface, 'std.space.share')`.
 *    An op not named is absent from the interface, so `useCommit` on it throws
 *    the same "not in this catalog" error as any op the token cannot call,
 *    and a screen cannot reach `std.space.revokeShare` by accident.
 *
 * If a later door does carry a platform op in its descriptor, that descriptor
 * wins and the table entry is ignored.
 */

import type { OpDescriptor, OpKind, OrdoType, ParamDescriptor } from './descriptor.js';
import { OrdoInterface } from './load.js';

/** The refusals every platform write names, from the door. */
const WRITE_REFUSALS = [
  'access_denied',
  'idempotency_in_flight',
  'idempotency_mismatch',
  'invalid_param',
  'method_mismatch',
  'missing_param',
  'unknown_param',
] as const;

const READ_REFUSALS = ['access_denied', 'invalid_param', 'method_mismatch', 'unknown_param'] as const;

type P = readonly [name: string, type: OrdoType, required: boolean];

function param([name, type, required]: P): ParamDescriptor {
  return { name, type, required, max: null, min: null, places: null, id_entity: null, field_ref: null, enum_values: null };
}

function platformOp(op: string, kind: OpKind, params: readonly P[], outputs: readonly string[]): OpDescriptor {
  const read = kind === 'read';
  return {
    op,
    version: 'platform',
    kind,
    mode: null,
    platform: true,
    params: params.map(param),
    result: {
      // A platform op's columns carry no declared type (`OrdoType | null`).
      outputs: outputs.map((name) => ({
        name,
        type: null,
        sealed: false,
        refinable: false,
        id_entity: null,
        align: 'left' as const,
      })),
      row_identity: null,
      single_row: false,
      synthetic_rows: true,
      shape: 'rows',
      sync: { syncable: false, reason: read ? 'no_anchor' : 'not_a_read' },
      order: null,
    },
    call: {
      method: read ? 'GET' : 'POST',
      terminal: read ? ['fetch'] : ['commit'],
      cas: false,
      refinable: false,
      live: false,
      paged: false,
    },
    refine: { tier: 'none', fields: [], limits: null },
    cost: { class: null, rows: null },
    disclosures: { guards: [], bypassed_guards: [], key_release: null, unbounded: false, unguarded: false },
    statements: [],
    refusals: read ? READ_REFUSALS : WRITE_REFUSALS,
    error: null,
  };
}

/** What the membership writes answer: the seat after the change. */
export interface MemberChange {
  readonly member: string;
  readonly principal: string;
  readonly role: string;
  readonly status: string;
  readonly outcome: string;
  readonly door: unknown;
}

/** The row each platform op answers, as the door names the columns. */
export interface PlatformRows {
  /** Share some rows with a person: the ticket to send. */
  readonly 'std.space.share': {
    readonly ticket: string;
    readonly expiresAt: number;
    readonly windowEndsAt: number;
    readonly ops: unknown;
    readonly outcome: string;
  };
  /** Close a guest's window into the tenant. */
  readonly 'std.space.revokeShare': {
    readonly seat: string;
    readonly principal: string;
    readonly grants: unknown;
    readonly aclRows: unknown;
    readonly suspended: unknown;
    readonly outcome: string;
    readonly door: unknown;
  };
  /** The guests who hold a window. */
  readonly 'std.space.shares': {
    readonly seat: string;
    readonly displayName: string;
    readonly guest: unknown;
    readonly ops: unknown;
    readonly grantedAt: number;
    readonly expiresAt: number;
  };
  /** Make a share link: the secret comes back once. */
  readonly 'std.space.shareLink': {
    readonly link: string;
    readonly id: string;
    readonly seat: string;
    readonly expiresAt: number;
    readonly ops: unknown;
    readonly outcome: string;
    /** Whether the link was given a password. */
    readonly hasPassword: boolean;
    /** The most opens the link allows, or `null` for no limit. */
    readonly maxOpens: number | null;
  };
  /** Change a link's maximum number of opens, or remove it with `none`. */
  readonly 'std.space.setLinkMaxOpens': {
    readonly id: string;
    readonly maxOpens: number | null;
    readonly opensUsed: number;
    readonly state: 'live' | 'used-up';
    readonly outcome: string;
  };
  /** Set a link's password, or clear it when `password` is left out. */
  readonly 'std.space.setLinkPassword': {
    readonly id: string;
    readonly hasPassword: boolean;
    readonly outcome: string;
  };
  /** Stop one link by its id. */
  readonly 'std.space.revokeLink': {
    readonly id: string;
    readonly seat: string;
    readonly grants: unknown;
    readonly aclRows: unknown;
    readonly outcome: string;
    readonly door: unknown;
  };
  /** The tenant's links with their state. Never the secret. */
  readonly 'std.space.links': {
    readonly id: string;
    readonly label: string;
    readonly ops: unknown;
    /** The rows the link opens, as `{ entity, id, level }` objects. */
    readonly rows: readonly { readonly entity?: string; readonly id?: string; readonly level?: string }[];
    readonly opens: number;
    readonly createdAt: number;
    readonly expiresAt: number;
    readonly state: 'live' | 'expired' | 'revoked' | 'used-up';
    readonly issuedBy: string;
    /** Whether opening it wants a password. Never the password or its hash. */
    readonly hasPassword: boolean;
    /** When a lock after too many wrong passwords ends, or `null`. */
    readonly lockedUntil: number | null;
    /** Opens counted against `maxOpens`: the devices that have opened it. */
    readonly opensUsed: number;
    /** The most opens the link allows, or `null` for no limit. */
    readonly maxOpens: number | null;
  };
  /** Invite a person: the ticket to send, once. */
  readonly 'std.space.invite': {
    readonly ticket: string;
    readonly expiresAt: number;
    readonly role: string | null;
    readonly outcome: string;
  };
  /** The tenant's invitations with their state. Never the ticket. */
  readonly 'std.space.invitations': {
    readonly id: string;
    readonly role: string | null;
    readonly for: string | null;
    readonly label: string;
    readonly issuedBy: string;
    readonly issuedAt: number;
    readonly expiresAt: number;
    readonly status: 'open' | 'declined';
    readonly declinedAt: number | null;
  };
  /** Withdraw one invitation by its id. */
  readonly 'std.space.revokeInvitation': {
    readonly id: string;
    readonly outcome: string;
  };
  /** Change a member's role. */
  readonly 'std.space.setRole': {
    readonly member: string;
    readonly principal: string;
    readonly role: string;
    readonly status: string;
    readonly outcome: string;
    readonly door: unknown;
  };
  /** Read the access profile: one row per op, or the op named. */
  readonly 'std.space.profile': {
    readonly op: string;
    readonly roles: readonly string[];
    readonly groups: readonly string[];
    readonly allow: readonly string[];
    readonly deny: readonly string[];
    readonly changed: boolean;
  };
  /** Set the roles and groups that may call an op. Both params are comma-separated. */
  readonly 'std.space.setOpAccess': {
    readonly op: string;
    readonly roles: readonly string[];
    readonly groups: readonly string[];
    readonly outcome: string;
    readonly door: unknown;
  };
  /** Put an op back at its default access. */
  readonly 'std.space.resetOpAccess': {
    readonly op: string;
    readonly outcome: string;
    readonly door: unknown;
  };
  /** Allow, deny or default one member for one op. */
  readonly 'std.space.setMemberAccess': {
    readonly member: string;
    readonly op: string;
    readonly effect: 'allow' | 'deny' | 'default';
    readonly door: unknown;
  };
  /** Make a group. */
  readonly 'std.space.createGroup': {
    readonly group: string;
    readonly name: string;
    readonly outcome: string;
    readonly door: unknown;
  };
  /** Add a member to a group. */
  readonly 'std.space.addToGroup': {
    readonly group: string;
    readonly member: string;
    readonly outcome: string;
    readonly door: unknown;
  };
  /** Take a member out of a group. */
  readonly 'std.space.removeFromGroup': {
    readonly group: string;
    readonly member: string;
    readonly outcome: string;
    readonly door: unknown;
  };
  /** Delete a group and its links. Refused while a profile line names it. */
  readonly 'std.space.deleteGroup': {
    readonly group: string;
    readonly outcome: string;
    readonly links: number;
    readonly door: unknown;
  };
  /** Every group of the space with its members. */
  readonly 'std.space.groups': {
    readonly group: string;
    readonly name: string;
    readonly members: readonly string[];
  };
  /** Seat a person (by principal) in the tenant with a role. */
  readonly 'std.space.admit': MemberChange;
  /** Stop a member's access, keeping their seat. */
  readonly 'std.space.suspend': MemberChange;
  /** Give a suspended member their access back. */
  readonly 'std.space.reinstate': MemberChange;
  /** Take a member out of the tenant. */
  readonly 'std.space.remove': MemberChange;
  /** Leave the tenant: the caller's own seat, so it takes no params. */
  readonly 'std.space.leave': {
    readonly member: string;
    readonly status: string;
    readonly rule: string;
    readonly effects: unknown;
    readonly door: unknown;
  };
  /** The tenant's members, at most 500 a page in member id order; pass the last `member` as `after` for more. */
  readonly 'std.space.members': {
    readonly member: string;
    readonly principal: string;
    readonly name: string | null;
    /** `null` for a guest's seat. */
    readonly role: string | null;
    readonly status: string;
    readonly joined: string | null;
    readonly groups: readonly string[];
  };
  /** Read what an agent's delegation request would let it do: the disclosure to show. */
  readonly 'std.delegation.review': {
    readonly request: string;
    readonly tenant: string;
    readonly agent: { readonly id: string; readonly name: string; readonly namedBy: string };
    readonly person: string;
    readonly term: { readonly hours: number; readonly expiresIfApprovedNow: number; readonly expiresText: string };
    readonly requestExpiresAt: number;
    readonly ops: readonly {
      readonly op: string;
      readonly kind: string;
      readonly reads: readonly { readonly entity: string; readonly fields: readonly string[] }[];
      readonly writes: readonly { readonly entity: string; readonly fields: readonly string[] }[];
      readonly examines: readonly { readonly entity: string; readonly fields: readonly string[] }[];
      readonly deletes: readonly string[];
      readonly words: readonly string[];
      readonly yours: boolean;
    }[];
    readonly notGranted: readonly { readonly op: string; readonly reason: string }[];
    readonly unknownNames: number;
    readonly cannot: readonly string[];
    readonly ceiling: string;
    readonly revoke: string;
    readonly ownerSees: string | null;
  };
  /** Grant a delegation request. Needs a fresh passkey touch unless the app developer exempted it. */
  readonly 'std.delegation.approve': {
    readonly request: string;
    readonly delegation: string;
    readonly agent: string;
    readonly ops: readonly string[];
    readonly expiresAt: number;
    readonly replaced: readonly string[];
    readonly outcome: 'approved';
  };
  /** Refuse a delegation request. */
  readonly 'std.delegation.decline': {
    readonly request: string;
    readonly outcome: 'declined';
  };
  /** The person's own delegations, waiting and live and ended. */
  readonly 'std.delegation.mine': {
    readonly request: string | null;
    readonly delegation: string | null;
    readonly agent: string;
    readonly agentName: string;
    readonly state: 'waiting' | 'live' | 'revoked' | 'expired' | 'declined' | 'failed';
    readonly ops: readonly string[];
    readonly grantedAt: number | null;
    readonly expiresAt: number;
    readonly revokedAt: number | null;
  };
  /** End a live delegation now. */
  readonly 'std.delegation.revoke': {
    readonly delegation: string;
    readonly outcome: 'revoked' | 'already ended';
  };
}

export type PlatformOpName = keyof PlatformRows;

const TEXT = 'text' as const;
const DURATION = 'duration' as const;
const MEMBER_CHANGE = ['member', 'principal', 'role', 'status', 'outcome', 'door'] as const;

/** The table. Keyed by op name; each value is an ordinary {@link OpDescriptor}. */
export const PLATFORM_OPS: { readonly [N in PlatformOpName]: OpDescriptor } = {
  'std.space.share': platformOp(
    'std.space.share',
    'write',
    [
      ['ops', TEXT, true],
      ['rows', TEXT, true],
      ['expires', DURATION, true],
      ['to', TEXT, false],
      ['label', TEXT, false],
    ],
    ['ticket', 'expiresAt', 'windowEndsAt', 'ops', 'outcome'],
  ),
  'std.space.revokeShare': platformOp(
    'std.space.revokeShare',
    'write',
    [['seat', TEXT, true]],
    ['seat', 'principal', 'grants', 'aclRows', 'suspended', 'outcome', 'door'],
  ),
  'std.space.shares': platformOp('std.space.shares', 'read', [], [
    'seat',
    'displayName',
    'guest',
    'ops',
    'grantedAt',
    'expiresAt',
  ]),
  'std.space.shareLink': platformOp(
    'std.space.shareLink',
    'write',
    [
      ['ops', TEXT, true],
      ['rows', TEXT, true],
      ['expires', DURATION, false],
      ['label', TEXT, false],
      ['password', TEXT, false],
      ['maxOpens', TEXT, false],
    ],
    ['link', 'id', 'seat', 'expiresAt', 'ops', 'outcome', 'hasPassword', 'maxOpens'],
  ),
  'std.space.setLinkMaxOpens': platformOp(
    'std.space.setLinkMaxOpens',
    'write',
    [
      ['link', TEXT, true],
      ['maxOpens', TEXT, true],
    ],
    ['id', 'maxOpens', 'opensUsed', 'state', 'outcome'],
  ),
  'std.space.setLinkPassword': platformOp(
    'std.space.setLinkPassword',
    'write',
    [
      ['link', TEXT, true],
      ['password', TEXT, false],
    ],
    ['id', 'hasPassword', 'outcome'],
  ),
  'std.space.revokeLink': platformOp(
    'std.space.revokeLink',
    'write',
    [['link', TEXT, true]],
    ['id', 'seat', 'grants', 'aclRows', 'outcome', 'door'],
  ),
  'std.space.links': platformOp('std.space.links', 'read', [], [
    'id',
    'label',
    'ops',
    'rows',
    'opens',
    'createdAt',
    'expiresAt',
    'state',
    'issuedBy',
    'hasPassword',
    'lockedUntil',
    'opensUsed',
    'maxOpens',
  ]),
  'std.space.invite': platformOp(
    'std.space.invite',
    'write',
    [
      ['role', TEXT, false],
      ['expires', DURATION, false],
      ['to', TEXT, false],
      ['label', TEXT, false],
    ],
    ['ticket', 'expiresAt', 'role', 'outcome'],
  ),
  'std.space.invitations': platformOp('std.space.invitations', 'read', [], [
    'id',
    'role',
    'for',
    'label',
    'issuedBy',
    'issuedAt',
    'expiresAt',
    'status',
    'declinedAt',
  ]),
  'std.space.revokeInvitation': platformOp('std.space.revokeInvitation', 'write', [['id', TEXT, true]], [
    'id',
    'outcome',
  ]),
  'std.space.setRole': platformOp(
    'std.space.setRole',
    'write',
    [
      ['member', TEXT, true],
      ['role', TEXT, true],
    ],
    ['member', 'principal', 'role', 'status', 'outcome', 'door'],
  ),
  'std.space.profile': platformOp('std.space.profile', 'read', [['op', TEXT, false]], [
    'op',
    'roles',
    'groups',
    'allow',
    'deny',
    'changed',
  ]),
  'std.space.setOpAccess': platformOp(
    'std.space.setOpAccess',
    'write',
    [
      ['op', TEXT, true],
      ['roles', TEXT, false],
      ['groups', TEXT, false],
    ],
    ['op', 'roles', 'groups', 'outcome', 'door'],
  ),
  'std.space.resetOpAccess': platformOp('std.space.resetOpAccess', 'write', [['op', TEXT, true]], [
    'op',
    'outcome',
    'door',
  ]),
  'std.space.setMemberAccess': platformOp(
    'std.space.setMemberAccess',
    'write',
    [
      ['member', TEXT, true],
      ['op', TEXT, true],
      ['effect', TEXT, true],
    ],
    ['member', 'op', 'effect', 'door'],
  ),
  'std.space.createGroup': platformOp('std.space.createGroup', 'write', [['name', TEXT, true]], [
    'group',
    'name',
    'outcome',
    'door',
  ]),
  'std.space.addToGroup': platformOp(
    'std.space.addToGroup',
    'write',
    [
      ['group', TEXT, true],
      ['member', TEXT, true],
    ],
    ['group', 'member', 'outcome', 'door'],
  ),
  'std.space.removeFromGroup': platformOp(
    'std.space.removeFromGroup',
    'write',
    [
      ['group', TEXT, true],
      ['member', TEXT, true],
    ],
    ['group', 'member', 'outcome', 'door'],
  ),
  'std.space.deleteGroup': platformOp(
    'std.space.deleteGroup',
    'write',
    [['group', TEXT, true]],
    ['group', 'outcome', 'links', 'door'],
  ),
  'std.space.groups': platformOp('std.space.groups', 'read', [], ['group', 'name', 'members']),
  'std.space.admit': platformOp(
    'std.space.admit',
    'write',
    [
      ['principal', TEXT, true],
      ['role', TEXT, true],
      ['display', TEXT, false],
    ],
    MEMBER_CHANGE,
  ),
  'std.space.suspend': platformOp('std.space.suspend', 'write', [['member', TEXT, true]], MEMBER_CHANGE),
  'std.space.reinstate': platformOp('std.space.reinstate', 'write', [['member', TEXT, true]], MEMBER_CHANGE),
  'std.space.remove': platformOp('std.space.remove', 'write', [['member', TEXT, true]], MEMBER_CHANGE),
  'std.space.leave': platformOp('std.space.leave', 'write', [], ['member', 'status', 'rule', 'effects', 'door']),
  'std.space.members': platformOp('std.space.members', 'read', [['after', TEXT, false]], [
    'member',
    'principal',
    'name',
    'role',
    'status',
    'joined',
    'groups',
  ]),
  'std.delegation.review': platformOp('std.delegation.review', 'read', [['request', TEXT, true]], [
    'request',
    'tenant',
    'agent',
    'person',
    'term',
    'requestExpiresAt',
    'ops',
    'notGranted',
    'unknownNames',
    'cannot',
    'ceiling',
    'revoke',
    'ownerSees',
  ]),
  'std.delegation.approve': platformOp('std.delegation.approve', 'write', [['request', TEXT, true]], [
    'request',
    'delegation',
    'agent',
    'ops',
    'expiresAt',
    'replaced',
    'outcome',
  ]),
  'std.delegation.decline': platformOp('std.delegation.decline', 'write', [['request', TEXT, true]], [
    'request',
    'outcome',
  ]),
  'std.delegation.mine': platformOp('std.delegation.mine', 'read', [], [
    'request',
    'delegation',
    'agent',
    'agentName',
    'state',
    'ops',
    'grantedAt',
    'expiresAt',
    'revokedAt',
  ]),
  'std.delegation.revoke': platformOp('std.delegation.revoke', 'write', [['delegation', TEXT, true]], [
    'delegation',
    'outcome',
  ]),
};

/** Whether `name` is an op this table describes. */
export function isPlatformOp(name: string): name is PlatformOpName {
  return Object.prototype.hasOwnProperty.call(PLATFORM_OPS, name);
}

/**
 * The interface with the named platform ops added, so the hooks can call
 * them. Ops the document already carries are left as the door described them.
 * Naming an op is the opt-in; see the module note.
 */
export function withPlatformOps(iface: OrdoInterface, ...names: readonly PlatformOpName[]): OrdoInterface {
  const extra = names.filter((n) => iface.op(n) === undefined).map((n) => PLATFORM_OPS[n]);
  if (extra.length === 0) return iface;
  return new OrdoInterface({ ...iface.doc, ops: [...iface.doc.ops, ...extra] });
}
