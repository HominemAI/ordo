/**
 * Tenant-owned sign-in providers: a tenant's owner adds an outside identity
 * provider (OpenID Connect or OAuth 2) to their own space, and people sign in
 * to that space with it.
 *
 * | Function | Route | Yields |
 * |---|---|---|
 * | `addConnection` | `POST /op/std.space.addConnection` | the new `Connection` |
 * | `updateConnection` | `POST /op/std.space.updateConnection` | the changed `Connection` |
 * | `retireConnection` | `POST /op/std.space.retireConnection` | the retired `Connection` |
 * | `listConnections` | `GET /op/std.space.connections` | every `Connection` of the space |
 * | `beginTenantSignIn` | `POST /auth/begin` | where to send the browser, and a binder |
 * | `completeTenantSignIn` | `POST /auth/complete` | a session, or a `SecondStep` |
 *
 * # The rules that make it work
 *
 * The four `std.space.*` ops are platform ops. They run in the tenant's own
 * space, only for its owner, and only with a token that has presence (a recent
 * passkey step-up). A call without presence is refused.
 *
 * The app developer switches the feature on with `ORDO_AUTH_TENANT_PROVIDERS`.
 * While it is off, the ops fail with a `ConnectionError` and the sign-in with a
 * `PasskeyError`, both with failure `tenant_providers_off`. Their `setting`
 * names that setting.
 *
 * The client secret goes in and never comes out. Every `Connection` has
 * `secret: "set"`. To change the secret, pass `clientSecret` to
 * `updateConnection`.
 *
 * `kind` and `issuer` cannot change after `addConnection`. Retire the
 * connection and add a new one.
 *
 * Register `Connection.callback` as the redirect URI at the provider.
 *
 * The sign-in uses the connection `name` as the `method`, with the tenant's
 * space id as `tenant`. The browser goes to `redirect`, the provider sends the
 * person back with a one-time code, and `completeTenantSignIn` swaps the code
 * and the `binder` for a session. Keep the binder in memory or
 * `sessionStorage`. Never put it in a URL.
 *
 * `role` is the role a newcomer is admitted at. It defaults to `viewer`.
 */

import { OrdoRefusal, failureDetail } from '../core/refusal.js';
import { authRequest } from './request.js';
import {
  call,
  text,
  PasskeyError,
  type PasskeyEndpoint,
  type SignInOptions,
  type Wire,
} from './passkey.js';
import { named, result, type SignInResult } from './second-step.js';

/** How the provider speaks. */
export type ConnectionKind = 'oidc' | 'oauth2';

/** The role a newcomer is admitted at. */
export type ConnectionRole = 'owner' | 'manager' | 'editor' | 'viewer';

/** The settings an add takes and an update may change. */
export interface ConnectionSettings {
  /** OpenID Connect: the issuer address. Fixed once added. */
  readonly issuer?: string;
  /** OpenID Connect: all three of the three addresses, or none (discovery). */
  readonly authorizeUrl?: string;
  readonly tokenUrl?: string;
  readonly jwksUrl?: string;
  /** OAuth 2: where the person's profile is read. */
  readonly userinfoUrl?: string;
  readonly clientId?: string;
  /** Sent to the door, never returned. */
  readonly clientSecret?: string;
  readonly scope?: string;
  /** A comma list of the claims shown as the person's display name. */
  readonly displayClaims?: string;
  readonly avatarClaim?: string;
  readonly useEmail?: boolean;
  readonly subjectClaim?: string;
  readonly emailClaim?: string;
  readonly emailVerifiedClaim?: string;
  readonly role?: ConnectionRole;
}

/** What `addConnection` takes. */
export interface NewConnection extends ConnectionSettings {
  /** 1 to 32 characters of `a-z`, `0-9` and `-`. It is the sign-in `method`. */
  readonly name: string;
  readonly kind: ConnectionKind;
  readonly clientId: string;
  readonly clientSecret: string;
}

/** What `updateConnection` may change: everything except `kind` and `issuer`. */
export type ConnectionChanges = Omit<ConnectionSettings, 'issuer'>;

/** One connection, as the door answers it. */
export interface Connection {
  readonly name: string;
  readonly key: string;
  readonly kind: ConnectionKind;
  readonly state: 'active' | 'retired';
  readonly issuer?: string;
  readonly authorizeUrl?: string;
  readonly tokenUrl?: string;
  readonly jwksUrl?: string;
  readonly userinfoUrl?: string;
  readonly clientId: string;
  /** Always `"set"`. The secret is never returned. */
  readonly secret: string;
  readonly scope?: string;
  readonly displayClaims?: string;
  readonly avatarClaim?: string;
  readonly useEmail?: boolean;
  readonly subjectClaim?: string;
  readonly emailClaim?: string;
  readonly emailVerifiedClaim?: string;
  readonly role: ConnectionRole;
  /** The redirect URI to register at the provider. */
  readonly callback: string;
  /** Writes only: what the call did. */
  readonly outcome?: 'added' | 'updated' | 'retired';
}

/** Where the ops are called, and as whom. */
export interface ConnectionEndpoint extends Pick<PasskeyEndpoint, 'baseUrl' | 'fetch' | 'callTimeoutMs' | 'network' | 'busyRetry'> {
  /** The tenant's space id. The ops run in this space. */
  readonly space: string;
  /** The owner's session token. A write needs presence. */
  readonly token: string;
}

/** Why a connection call failed. */
export type ConnectionFailure =
  /** The app developer has not switched tenant providers on. */
  | 'tenant_providers_off'
  /** The tenant may not do this. */
  | 'tenant_refused'
  /** A connection of that name exists. */
  | 'duplicate'
  | 'not_found'
  /** The provider could not be reached. */
  | 'identity_provider_unavailable'
  /** The door could not be reached or answered something unreadable. */
  | 'unavailable'
  /** The door did not answer within the call's deadline. */
  | 'timeout'
  /** Any other refusal. `code` has the door's word. */
  | 'refused';

/** A refusal of a connection op. */
export class ConnectionError extends OrdoRefusal<string> {
  /**
   * What happened. An {@link OrdoRefusal} besides: `code` is the door's
   * error code when it sent one (else the failure), `status` the HTTP status.
   */
  readonly failure: ConnectionFailure;
  /** For `tenant_providers_off`: the setting the app developer changes. */
  readonly setting?: string;
  constructor(
    failure: ConnectionFailure,
    message: string,
    status: number,
    code?: string,
    setting?: string,
  ) {
    super(failureDetail(failure, message, { status, code }), null, { message });
    this.name = 'ConnectionError';
    this.failure = failure;
    if (setting !== undefined) this.setting = setting;
  }
}

const KNOWN: readonly string[] = [
  'tenant_providers_off',
  'tenant_refused',
  'duplicate',
  'not_found',
  'identity_provider_unavailable',
];

async function op(
  endpoint: ConnectionEndpoint,
  name: string,
  method: 'GET' | 'POST',
  params: Wire,
): Promise<readonly Wire[]> {
  const headers: Record<string, string> = {
    Accept: 'application/json',
    Authorization: `Bearer ${endpoint.token}`,
    'X-Ordo-Space': endpoint.space,
  };
  const init: RequestInit = { method, headers };
  const path = `/op/${encodeURIComponent(name)}`;
  if (method === 'POST') {
    headers['Content-Type'] = 'application/json';
    headers['Idempotency-Key'] = globalThis.crypto.randomUUID();
    init.body = JSON.stringify(params);
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
    const failure: ConnectionFailure =
      code !== undefined && KNOWN.includes(code) ? (code as ConnectionFailure) : 'refused';
    const setting = typeof e['setting'] === 'string' ? e['setting'] : undefined;
    throw new ConnectionError(failure, message, response.status, code, setting);
  }
  const rows = (json as Wire | null)?.['rows'];
  if (!Array.isArray(rows)) {
    throw new ConnectionError('unavailable', "the door's answer has no `rows`", response.status);
  }
  return rows as Wire[];
}

function one(rows: readonly Wire[]): Connection {
  const row = rows[0];
  if (!row) throw new ConnectionError('unavailable', "the door's answer has no row", 200);
  return row as unknown as Connection;
}

/**
 * Add a provider to the space. The owner needs presence. The answer has the
 * `callback` to register at the provider.
 */
export async function addConnection(
  endpoint: ConnectionEndpoint,
  connection: NewConnection,
): Promise<Connection> {
  return one(await op(endpoint, 'std.space.addConnection', 'POST', { ...connection }));
}

/**
 * Change a connection. `kind` and `issuer` cannot change. `clientSecret`
 * replaces the secret.
 */
export async function updateConnection(
  endpoint: ConnectionEndpoint,
  name: string,
  changes: ConnectionChanges,
): Promise<Connection> {
  return one(await op(endpoint, 'std.space.updateConnection', 'POST', { ...changes, name }));
}

/** Retire a connection. People can no longer sign in with it. */
export async function retireConnection(
  endpoint: ConnectionEndpoint,
  name: string,
): Promise<Connection> {
  return one(await op(endpoint, 'std.space.retireConnection', 'POST', { name }));
}

/** Every connection of the space, retired ones included. */
export async function listConnections(
  endpoint: ConnectionEndpoint,
): Promise<readonly Connection[]> {
  return (await op(endpoint, 'std.space.connections', 'GET', {})) as unknown as readonly Connection[];
}

// ── signing in ───────────────────────────────────────────────────────────

/** What the page passes to begin a sign-in with a tenant's provider. */
export interface TenantSignInStart {
  /** The connection name. */
  readonly method: string;
  /** The tenant's space id. */
  readonly tenant: string;
  /** Where the browser lands after the provider. */
  readonly returnTo: string;
  /** Ask for presence on the new session. */
  readonly presence?: boolean;
}

/** A sign-in the door began. */
export interface TenantSignInBegun {
  /** Send the browser here: the provider's page. */
  readonly redirect: string;
  /** Keep it until the person comes back. Never put it in a URL. */
  readonly binder: string;
}

/** What the page passes to finish it. */
export interface TenantSignInFinish {
  readonly method: string;
  readonly tenant: string;
  /** The one-time code in the address the person came back to. */
  readonly handoff: string;
  readonly binder: string;
}

/** Begin a sign-in with a tenant's provider. */
export async function beginTenantSignIn(
  endpoint: PasskeyEndpoint,
  start: TenantSignInStart,
): Promise<TenantSignInBegun> {
  const body: Wire = { method: start.method, tenant: start.tenant, return_to: start.returnTo };
  if (start.presence) body['presence'] = true;
  const begun = await call(endpoint, '/auth/begin', body);
  return { redirect: text(begun, 'redirect'), binder: text(begun, 'binder') };
}

/**
 * Finish a sign-in with a tenant's provider. A deployment that asks for a
 * second step answers a `SecondStep`.
 */
export async function completeTenantSignIn(
  endpoint: PasskeyEndpoint,
  finish: TenantSignInFinish,
  options: SignInOptions = {},
): Promise<SignInResult> {
  if (!finish.handoff || !finish.binder) {
    throw new PasskeyError('refused', 'a tenant sign-in needs the hand-off code and the binder');
  }
  const body = named(
    {
      method: finish.method,
      tenant: finish.tenant,
      kind: 'handoff',
      handoff: finish.handoff,
      binder: finish.binder,
    },
    options,
  );
  return result(await call(endpoint, '/auth/complete', body));
}
