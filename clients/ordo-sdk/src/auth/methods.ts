/**
 * The ways in a deployment serves, so a page builds its sign-in screen from
 * the door instead of from a build variable.
 *
 * | Function | Route | Yields |
 * |---|---|---|
 * | `signInMethods` | `GET /auth/methods` | a `SignInMethods` |
 *
 * The route is public and needs no token. It says only what a sign-in button
 * already shows: which methods are on, the outside connections by name, label
 * and kind, and the public Turnstile site key when the app developer set one.
 * It never carries a secret, a client id or an address inside a provider.
 *
 * Pass `tenant` to add that tenant's own active providers (each marked
 * `tenant: true`; begin them with `beginTenantSignIn`). A tenant with none and a
 * tenant that does not exist answer alike, so this cannot list tenants.
 *
 * The answer may be cached for a minute. A page asks once at start.
 */

import { call, PasskeyError, type PasskeyEndpoint } from './passkey.js';

/** `oauth2` or `oidc`. */
export type MethodConnectionKind = 'oauth2' | 'oidc';

/** One outside connection, as its button shows it. */
export interface MethodConnection {
  /** What a page passes to `begin` as the method. */
  readonly name: string;
  /** The button's words. */
  readonly label: string;
  readonly kind: MethodConnectionKind;
  /** True for a tenant's own provider: pass the tenant to `begin` too. */
  readonly tenant: boolean;
}

/** The Turnstile check, when the deployment asks for one at `begin`. */
export interface HumanCheckMethod {
  readonly kind: 'turnstile';
  /** The widget's public site key, when the app developer set `ORDO_AUTH_HUMAN_CHECK_SITE_KEY`. */
  readonly siteKey?: string;
}

/** What this deployment (and a named tenant) offers at sign-in. */
export interface SignInMethods {
  readonly passkey: boolean;
  readonly password: boolean;
  readonly email: boolean;
  readonly guest: boolean;
  /** `null` when the human check is off. */
  readonly humanCheck: HumanCheckMethod | null;
  readonly connections: readonly MethodConnection[];
}

/** Which tenant's providers to add. */
export interface SignInMethodsOptions {
  readonly tenant?: string;
}

function malformed(what: string): PasskeyError {
  return new PasskeyError('malformed', `the door's list of sign-in methods has a bad \`${what}\``);
}

/** Ask the door which ways in it serves. Rejects with `PasskeyError`. */
export async function signInMethods(
  endpoint: PasskeyEndpoint,
  options: SignInMethodsOptions = {},
): Promise<SignInMethods> {
  const path =
    options.tenant === undefined || options.tenant === ''
      ? '/auth/methods'
      : `/auth/methods?tenant=${encodeURIComponent(options.tenant)}`;
  const o = await call(endpoint, path, null, undefined, 'GET');
  const flag = (k: string): boolean => {
    const v = o[k];
    if (typeof v !== 'boolean') throw malformed(k);
    return v;
  };
  let humanCheck: HumanCheckMethod | null = null;
  const h = o['humanCheck'];
  if (h !== null && h !== undefined) {
    if (typeof h !== 'object' || (h as Record<string, unknown>)['kind'] !== 'turnstile') {
      throw malformed('humanCheck');
    }
    const key = (h as Record<string, unknown>)['siteKey'];
    humanCheck =
      typeof key === 'string' && key !== '' ? { kind: 'turnstile', siteKey: key } : { kind: 'turnstile' };
  }
  const raw = o['connections'];
  if (!Array.isArray(raw)) throw malformed('connections');
  const connections = raw.map((c: unknown): MethodConnection => {
    const r = (c ?? {}) as Record<string, unknown>;
    const { name, label, kind } = r;
    if (typeof name !== 'string' || typeof label !== 'string' || (kind !== 'oauth2' && kind !== 'oidc')) {
      throw malformed('connections');
    }
    return { name, label, kind, tenant: r['tenant'] === true };
  });
  return {
    passkey: flag('passkey'),
    password: flag('password'),
    email: flag('email'),
    guest: flag('guest'),
    humanCheck,
    connections,
  };
}
