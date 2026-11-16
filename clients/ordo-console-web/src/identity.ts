/**
 * Sign-in at the identity service (docs/plan-identity-service.md 5.4).
 *
 * When the console names an identity service (`status.identity`), people
 * sign in there, not here:
 *
 * 1. With no token, the page sends the browser to the service's sign-in
 *    page, which sends it back here once signed in.
 * 2. The page asks the service for a 10 minute token with its session
 *    cookie (`POST /token`, `credentials: 'include'`). The token lives in
 *    this module's memory only, never in storage, and is renewed 1 minute
 *    before it ends.
 * 3. Every console call carries it as a bearer (`api.ts`).
 * 4. A call that needs a fresh touch (a reauth) is decided from the RP ID
 *    the console's status carries, before any network call. When this
 *    page's host is the RP ID or under it, the ceremony runs here against
 *    `/touch/begin` and `/touch/finish`, which answers a token with the
 *    new touch. Otherwise the browser cannot use the passkey here, so
 *    the page goes to the service's sign-in page with `touch=1`, the
 *    ceremony runs there, and the browser comes back; the next token
 *    carries the touch. The person repeats the action after the return
 *    (`touch.ts`).
 * 5. Sign-out ends the service session and forgets the token.
 */

import { ConsoleCallError } from './api';
import { assertWith } from './webauthn';

export interface IdentityInfo {
  origin: string;
  audience: string;
  /** The passkeys' RP ID, from the console's status. Absent on an old console. */
  rpId?: string;
}

interface Issued {
  token: string;
  exp: number;
}

/** Renew this many seconds before the token ends. */
const RENEW_MARGIN_SECS = 60;

let service: IdentityInfo | null = null;
let held: Issued | null = null;
let pending: Promise<string> | null = null;

/** Called once the console's status names the service. */
export function setIdentity(info: IdentityInfo | null | undefined): void {
  service = info ?? null;
}

/** Whether people sign in at the identity service. */
export function identityMode(): boolean {
  return service !== null;
}

/** Drop the token held, so the next call asks the service for a new one. */
export function forgetToken(): void {
  held = null;
}

/** A POST to the identity service with its session cookie. */
async function servicePost<T>(path: string, body: unknown = {}): Promise<T> {
  if (!service) throw new ConsoleCallError('not_signed_in', 'No identity service is set.', 0, false);
  let res: Response;
  try {
    res = await fetch(`${service.origin}${path}`, {
      method: 'POST',
      mode: 'cors',
      credentials: 'include',
      cache: 'no-store',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });
  } catch {
    throw new ConsoleCallError(
      'identity_not_answering',
      'The identity service did not answer. It may be starting; try again shortly.',
      0,
      true,
    );
  }
  let json: unknown = null;
  try {
    json = await res.json();
  } catch {
    json = null;
  }
  if (!res.ok) {
    const e = (json ?? {}) as { error?: string; message?: string; retry?: boolean };
    throw new ConsoleCallError(
      e.error ?? 'unknown',
      e.message ?? `The identity service answered ${res.status}.`,
      res.status,
      e.retry ?? false,
    );
  }
  return json as T;
}

function keep(issued: Issued): string {
  held = { token: issued.token, exp: issued.exp };
  return issued.token;
}

/**
 * A token for this console, from memory while it has more than a minute
 * left, else from the service. One request at a time. A service that has
 * no session for this browser answers `not_signed_in`.
 */
export function token(): Promise<string> {
  const now = Date.now() / 1000;
  if (held && held.exp - RENEW_MARGIN_SECS > now) return Promise.resolve(held.token);
  if (!pending) {
    pending = servicePost<Issued>('/token', { aud: service?.audience })
      .then(keep)
      .finally(() => {
        pending = null;
      });
  }
  return pending;
}

/** The service's sign-in page, with what it needs to send the browser back. */
function signInUrl(info: IdentityInfo, extra = ''): string {
  const back = encodeURIComponent(window.location.href);
  // The service checks `return` against this deployment's own list, so a
  // shared service (many deployments) knows whose list to read.
  const handle = encodeURIComponent(info.audience.replace(/^ordo-deployment:/, ''));
  return `${info.origin}/sign-in?client=console&deployment=${handle}&return=${back}${extra}`;
}

/** Send the browser to the service's sign-in page; it comes back here. */
export function goSignIn(): void {
  if (!service) return;
  window.location.assign(signInUrl(service));
}

/**
 * Whether a passkey with this RP ID can be used at `host`. The server's
 * rule (`RelyingParty` in crates/ordo-identity/src/passkey.rs): lowercase
 * host, equal to the RP ID or ending in `.` and the RP ID.
 */
function hostUnder(host: string, rpId: string): boolean {
  const h = host.toLowerCase();
  const rp = rpId.toLowerCase();
  return h === rp || h.endsWith(`.${rp}`);
}

/**
 * Whether this page may run the passkey ceremony itself. A status with no
 * `rpId` (an older console) is judged by the service's own host, the
 * service's default RP ID: this page can run it only when it shares that
 * host.
 */
function ceremonyHere(info: IdentityInfo): boolean {
  const host = window.location.hostname;
  if (info.rpId) return hostUnder(host, info.rpId);
  return hostUnder(host, new URL(info.origin).hostname);
}

/** How long the page waits to be taken away before it says the redirect failed. */
const REDIRECT_WAIT_MS = 5000;

/**
 * A reauth inside the service session: the token held after it carries the
 * fresh touch. In page when this host may use the passkey; otherwise the
 * page goes to the service's sign-in page, and the person repeats the
 * action after the return.
 */
export async function touch(): Promise<void> {
  if (!service) throw new ConsoleCallError('not_signed_in', 'No identity service is set.', 0, false);
  const info = service;
  if (!ceremonyHere(info)) {
    window.location.assign(signInUrl(info, '&touch=1'));
    // The page is leaving. If it is still here, the browser did not go.
    await new Promise((resolve) => setTimeout(resolve, REDIRECT_WAIT_MS));
    throw new ConsoleCallError(
      'reauth_redirect',
      'Confirm it is you at the sign-in page, then try again.',
      0,
      false,
    );
  }
  // The answer is a token for this console's deployment, which may not be
  // the service's own when the service is shared.
  const aud = info.audience;
  const send = <R>(path: string, body?: unknown): Promise<R> =>
    servicePost<R>(path, path === '/touch/finish' ? { ...(body as object), aud } : body);
  const issued = await assertWith<Issued>(send, '/touch/begin', '/touch/finish');
  keep(issued);
}

/** End the service session and forget the token. */
export async function signOut(): Promise<void> {
  held = null;
  try {
    await servicePost('/sessions/end');
  } catch {
    // Ended already, or the service is down: the token is gone either way.
  }
}
