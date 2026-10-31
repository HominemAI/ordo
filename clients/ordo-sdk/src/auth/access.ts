/**
 * Who may call which op in a tenant's own space: the manager's side of the
 * access profile, and the groups it names.
 *
 * | Function | Route | Yields |
 * |---|---|---|
 * | `profile` | `GET /op/std.space.profile` | one `OpAccess` per op |
 * | `setOpAccess` | `POST /op/std.space.setOpAccess` | the op's new roles and groups |
 * | `resetOpAccess` | `POST /op/std.space.resetOpAccess` | the op back at its default |
 * | `setMemberAccess` | `POST /op/std.space.setMemberAccess` | one member's allow, deny or default |
 * | `createGroup` | `POST /op/std.space.createGroup` | the new group |
 * | `addToGroup` | `POST /op/std.space.addToGroup` | the group and member |
 * | `removeFromGroup` | `POST /op/std.space.removeFromGroup` | the group and member |
 * | `deleteGroup` | `POST /op/std.space.deleteGroup` | the group and the links removed |
 * | `groups` | `GET /op/std.space.groups` | every group with its members |
 *
 * The nine are platform ops. They run in the tenant's own space, only for its
 * manager or owner, and only with a token that has presence. A call without
 * presence is refused.
 *
 * Reads send their parameters in the query string. Writes send a JSON body.
 * `roles` and `groups` go on the wire as comma-separated text. The functions
 * take arrays and join them.
 */

import { OrdoRefusal, failureDetail } from '../core/refusal.js';
import type { PasskeyEndpoint } from './passkey.js';
import { authRequest } from './request.js';

type Wire = Record<string, unknown>;

/** The roles of a space, most powerful first. */
export type AccessRole = 'owner' | 'manager' | 'editor' | 'viewer';

/** What a member's own entry does to an op. */
export type MemberEffect = 'allow' | 'deny' | 'default';

/** Where the access ops are called, and as whom. */
export interface AccessEndpoint extends Pick<PasskeyEndpoint, 'baseUrl' | 'fetch' | 'callTimeoutMs' | 'network' | 'busyRetry'> {
  /** The tenant's space id. The ops run in this space. */
  readonly space: string;
  /** The manager's or owner's session token. A write needs presence. */
  readonly token: string;
}

/** One op's row of the profile. */
export interface OpAccess {
  readonly op: string;
  /** The roles that may call the op. */
  readonly roles: readonly string[];
  /** The groups that may call the op. */
  readonly groups: readonly string[];
  /** Members allowed by their own entry. */
  readonly allow: readonly string[];
  /** Members denied by their own entry. */
  readonly deny: readonly string[];
  /** Whether the default access was changed. */
  readonly changed: boolean;
}

/** What `setOpAccess` takes. */
export interface OpAccessChange {
  readonly op: string;
  readonly roles?: readonly AccessRole[];
  readonly groups?: readonly string[];
}

/** What `setOpAccess` answers. */
export interface OpAccessSet {
  readonly op: string;
  readonly roles: readonly string[];
  readonly groups: readonly string[];
  readonly outcome: string;
  readonly door: unknown;
}

/** What `resetOpAccess` answers. */
export interface OpAccessReset {
  readonly op: string;
  readonly outcome: string;
  readonly door: unknown;
}

/** What `setMemberAccess` takes. */
export interface MemberAccessChange {
  readonly member: string;
  readonly op: string;
  readonly effect: MemberEffect;
}

/** What `setMemberAccess` answers. */
export interface MemberAccessSet extends MemberAccessChange {
  readonly door: unknown;
}

/** What `createGroup` answers. */
export interface GroupCreated {
  readonly group: string;
  readonly name: string;
  readonly outcome: string;
  readonly door: unknown;
}

/** What `addToGroup` and `removeFromGroup` answer. */
export interface GroupMembership {
  readonly group: string;
  readonly member: string;
  readonly outcome: string;
  readonly door: unknown;
}

/** What `deleteGroup` answers. */
export interface GroupDeleted {
  readonly group: string;
  readonly outcome: string;
  /** Links to the group that were removed. */
  readonly links: number;
  readonly door: unknown;
}

/** One group with its members. */
export interface Group {
  readonly group: string;
  readonly name: string;
  readonly members: readonly string[];
}

/** A refusal of an access op. */
export class AccessError extends OrdoRefusal<string> {
  /**
   * An {@link OrdoRefusal}: `code` is the door's error code when it sent one
   * (`refused` when it did not), `status` the HTTP status.
   */
  constructor(message: string, status: number, code?: string) {
    super(failureDetail('refused', message, { status, code }), null, { message });
    this.name = 'AccessError';
  }
}

async function op(
  endpoint: AccessEndpoint,
  name: string,
  method: 'GET' | 'POST',
  params: Record<string, string | undefined>,
): Promise<readonly Wire[]> {
  const headers: Record<string, string> = {
    Accept: 'application/json',
    Authorization: `Bearer ${endpoint.token}`,
    'X-Ordo-Space': endpoint.space,
  };
  const init: RequestInit = { method, headers };
  let path = `/op/${encodeURIComponent(name)}`;
  const sent: Record<string, string> = {};
  for (const [k, v] of Object.entries(params)) if (v !== undefined) sent[k] = v;
  if (method === 'POST') {
    headers['Content-Type'] = 'application/json';
    headers['Idempotency-Key'] = globalThis.crypto.randomUUID();
    init.body = JSON.stringify(sent);
  } else if (Object.keys(sent).length > 0) {
    path += `?${new URLSearchParams(sent).toString()}`;
  }
  const response = await authRequest(endpoint, path, init, { auth: false });
  let json: unknown = null;
  try {
    json = await response.json();
  } catch {
    // An empty body: nothing to read.
  }
  if (!response.ok) {
    const body = (json ?? {}) as Wire;
    const inner = body['error'];
    const e = (inner !== null && typeof inner === 'object' ? inner : body) as Wire;
    const code = typeof e['code'] === 'string' ? e['code'] : undefined;
    const message =
      typeof e['message'] === 'string' ? e['message'] : (code ?? `HTTP ${response.status}`);
    throw new AccessError(message, response.status, code);
  }
  const rows = (json as Wire | null)?.['rows'];
  if (!Array.isArray(rows)) {
    throw new AccessError("the door's answer has no `rows`", response.status, 'unavailable');
  }
  return rows as Wire[];
}

function one<T>(rows: readonly Wire[]): T {
  const row = rows[0];
  if (!row) throw new AccessError("the door's answer has no row", 200, 'unavailable');
  return row as unknown as T;
}

/** The access profile: one row per op, or the one op named. */
export async function profile(
  endpoint: AccessEndpoint,
  opName?: string,
): Promise<readonly OpAccess[]> {
  return (await op(endpoint, 'std.space.profile', 'GET', {
    op: opName,
  })) as unknown as readonly OpAccess[];
}

/** Set the roles and groups that may call an op. */
export async function setOpAccess(
  endpoint: AccessEndpoint,
  change: OpAccessChange,
): Promise<OpAccessSet> {
  return one(
    await op(endpoint, 'std.space.setOpAccess', 'POST', {
      op: change.op,
      roles: change.roles?.join(','),
      groups: change.groups?.join(','),
    }),
  );
}

/** Put an op back at its default access. */
export async function resetOpAccess(
  endpoint: AccessEndpoint,
  opName: string,
): Promise<OpAccessReset> {
  return one(await op(endpoint, 'std.space.resetOpAccess', 'POST', { op: opName }));
}

/** Allow, deny or default one member for one op. */
export async function setMemberAccess(
  endpoint: AccessEndpoint,
  change: MemberAccessChange,
): Promise<MemberAccessSet> {
  return one(
    await op(endpoint, 'std.space.setMemberAccess', 'POST', {
      member: change.member,
      op: change.op,
      effect: change.effect,
    }),
  );
}

/** Make a group. */
export async function createGroup(endpoint: AccessEndpoint, name: string): Promise<GroupCreated> {
  return one(await op(endpoint, 'std.space.createGroup', 'POST', { name }));
}

/** Add a member to a group. */
export async function addToGroup(
  endpoint: AccessEndpoint,
  group: string,
  member: string,
): Promise<GroupMembership> {
  return one(await op(endpoint, 'std.space.addToGroup', 'POST', { group, member }));
}

/** Take a member out of a group. */
export async function removeFromGroup(
  endpoint: AccessEndpoint,
  group: string,
  member: string,
): Promise<GroupMembership> {
  return one(await op(endpoint, 'std.space.removeFromGroup', 'POST', { group, member }));
}

/**
 * Delete a group: its links and its row, as one command. Refused with
 * `group_in_use` while a profile line or an access grant on a record still
 * names it.
 */
export async function deleteGroup(endpoint: AccessEndpoint, group: string): Promise<GroupDeleted> {
  return one(await op(endpoint, 'std.space.deleteGroup', 'POST', { group }));
}

/** Every group of the space with its members. */
export async function groups(endpoint: AccessEndpoint): Promise<readonly Group[]> {
  return (await op(endpoint, 'std.space.groups', 'GET', {})) as unknown as readonly Group[];
}
