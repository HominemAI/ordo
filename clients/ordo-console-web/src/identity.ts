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
 * 4. A call that needs a fresh touch runs the passkey here against the
 *    service's `/touch/begin` and `/touch/finish`; the answer is a token
 *    with the new touch in it (`touch.ts`).
 * 5. Sign-out ends the service session and forgets the token.
 */

import { ConsoleCallError } from './api';
import { assertWith } from './webauthn';

export interface IdentityInfo {
  origin: string;
  audience: string;
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

/** Send the browser to the service's sign-in page; it comes back here. */
export function goSignIn(): void {
  if (!service) return;
  const back = encodeURIComponent(window.location.href);
  window.location.assign(`${service.origin}/sign-in?client=console&return=${back}`);
}

/** A fresh passkey touch inside the service session: the token held now carries it. */
export async function touch(): Promise<void> {
  const issued = await assertWith<Issued>(servicePost, '/touch/begin', '/touch/finish');
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
