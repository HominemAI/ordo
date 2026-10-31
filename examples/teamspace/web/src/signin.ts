/**
 * The ways in that leave the page or keep something on the device.
 *
 * - **Outside connections** (Discord, GitHub, Google, Microsoft, a company
 *   login). `POST /auth/begin` answers the provider's address and a binder.
 *   The binder waits in `sessionStorage` while the browser is away, never in
 *   a URL. The provider sends the person back to this page with `?handoff=`,
 *   and `POST /auth/complete` swaps the code and the binder for a session.
 *   The part of the address after `#` (the join route) is kept with the
 *   binder and put back on return.
 * - **The guest secret.** `startGuest` returns it once. It is the guest's only
 *   way back in, so it is kept in `localStorage`.
 * - **The email device marker.** An emailed-code sign-in hands one back. Sent
 *   on the next `begin`, it gives this browser an allowance of its own.
 * - **The last team chosen**, so a person with several teams lands in the same one.
 */

import { call, PasskeyError, type PasskeyEndpoint, type SignInOptions } from '@hominemai/ordo-sdk/auth/passkey.js';
import { named, result, type SignInResult } from '@hominemai/ordo-sdk/auth/second-step.js';

interface Pending {
  readonly method: string;
  readonly binder: string;
  /** The part after `#` when the person left (a join route), put back on return. */
  readonly hash: string;
}

const PENDING_KEY = 'ordo-teamspace.outside';
const GUEST_KEY = 'ordo-teamspace.guest';
const DEVICE_KEY = 'ordo-teamspace.device';
const TEAM_KEY = 'ordo-teamspace.team';

/** Storage that may be missing or may throw (a private window, blocked site data). */
export type Store = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

function read(store: Store | undefined, key: string): string | null {
  try {
    return store?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function write(store: Store | undefined, key: string, value: string | null): void {
  try {
    if (value === null) store?.removeItem(key);
    else store?.setItem(key, value);
  } catch {
    // Nothing kept. The person signs in again next time.
  }
}

const session = (): Store | undefined => (typeof sessionStorage === 'undefined' ? undefined : sessionStorage);
const local = (): Store | undefined => (typeof localStorage === 'undefined' ? undefined : localStorage);

/**
 * Start a sign-in at an outside connection. Answers the address to send the
 * browser to; the binder is kept in `store` until the person comes back.
 */
export async function beginOutside(
  endpoint: PasskeyEndpoint,
  method: string,
  returnTo: string,
  hash = '',
  store: Store | undefined = session(),
): Promise<string> {
  const begun = await call(endpoint, '/auth/begin', { method, return_to: returnTo });
  const redirect = begun['redirect'];
  const binder = begun['binder'];
  if (typeof redirect !== 'string' || typeof binder !== 'string') {
    throw new PasskeyError('malformed', "the door's answer has no `redirect` or `binder`");
  }
  write(store, PENDING_KEY, JSON.stringify({ method, binder, hash } satisfies Pending));
  return redirect;
}

/** What the address says after a provider sent the person back, if anything. */
export type Returned =
  | { readonly kind: 'none' }
  | {
      readonly kind: 'handoff';
      readonly method: string;
      readonly handoff: string;
      readonly binder: string;
      readonly hash: string;
    }
  | { readonly kind: 'error'; readonly error: string; readonly hash: string };

/**
 * Read `?handoff=` or `?error=` from the address, with the binder kept at
 * `begin`. The binder is forgotten here whatever the outcome: a hand-off is
 * good once.
 */
export function returnedFrom(search: string, store: Store | undefined = session()): Returned {
  const q = new URLSearchParams(search);
  const handoff = q.get('handoff');
  const error = q.get('error');
  if (handoff === null && error === null) return { kind: 'none' };
  const kept = read(store, PENDING_KEY);
  write(store, PENDING_KEY, null);
  let pending: Pending | null = null;
  try {
    const parsed = JSON.parse(kept ?? 'null') as Partial<Pending> | null;
    if (parsed && typeof parsed.method === 'string' && typeof parsed.binder === 'string') {
      pending = {
        method: parsed.method,
        binder: parsed.binder,
        hash: typeof parsed.hash === 'string' ? parsed.hash : '',
      };
    }
  } catch {
    pending = null;
  }
  const hash = pending?.hash ?? '';
  if (error !== null) return { kind: 'error', error, hash };
  // A code with no binder was not started here, or the tab was closed in
  // between. The door would refuse it, so it is not sent.
  if (pending === null || handoff === null) return { kind: 'error', error: 'authentication_failed', hash };
  return { kind: 'handoff', method: pending.method, handoff, binder: pending.binder, hash };
}

/** Swap the hand-off code and the binder for a session. Good once, within 60 seconds. */
export async function completeOutside(
  endpoint: PasskeyEndpoint,
  back: { readonly method: string; readonly handoff: string; readonly binder: string },
  options: SignInOptions = {},
): Promise<SignInResult> {
  return result(
    await call(
      endpoint,
      '/auth/complete',
      named({ method: back.method, kind: 'handoff', handoff: back.handoff, binder: back.binder }, options),
    ),
  );
}

/** The guest secret this device holds, or null. */
export function guestSecret(store: Store | undefined = local()): string | null {
  return read(store, GUEST_KEY);
}

/** Keep the guest secret, or forget it with `null`. */
export function keepGuestSecret(secret: string | null, store: Store | undefined = local()): void {
  write(store, GUEST_KEY, secret);
}

/** The email device marker this browser holds, or undefined. */
export function deviceMarker(store: Store | undefined = local()): string | undefined {
  return read(store, DEVICE_KEY) ?? undefined;
}

export function keepDeviceMarker(marker: string | undefined, store: Store | undefined = local()): void {
  if (marker) write(store, DEVICE_KEY, marker);
}

/** The team (a space id) this browser chose last, or null. */
export function lastTeam(store: Store | undefined = local()): string | null {
  return read(store, TEAM_KEY);
}

export function keepLastTeam(space: string | null, store: Store | undefined = local()): void {
  write(store, TEAM_KEY, space);
}
