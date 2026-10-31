/**
 * The ways in that leave the page or keep something on the device.
 *
 * - **Outside connections** (Discord, GitHub, Google, Microsoft, a company
 *   login). `POST /auth/begin` answers the provider's address and a binder.
 *   The binder waits in `sessionStorage` while the browser is away, never in
 *   a URL. The provider sends the person back to this page with `?handoff=`,
 *   and `POST /auth/complete` swaps the code and the binder for a session.
 *   The SDK has no helper for a deployment's own connection, so this file
 *   sends the two bodies `docs/howto-sign-in.md` section 8 gives, with the
 *   SDK's `call`.
 * - **The guest secret.** `startGuest` returns it once. It is the guest's only
 *   way back in, so it is kept in `localStorage` until the guest adds a
 *   passkey.
 * - **The email device marker.** An emailed-code sign-in hands one back. Sent
 *   on the next `begin`, it gives this browser an allowance of its own.
 */

import { call, PasskeyError, type PasskeyEndpoint } from '@hominemai/ordo-sdk/auth/passkey.js';
import { result, type SignInResult } from '@hominemai/ordo-sdk/auth/second-step.js';

/** What the page keeps while the browser is at the provider. */
interface Pending {
  readonly method: string;
  readonly binder: string;
  /** The part after `#` when the person left (a share to accept), put back on return. */
  readonly hash: string;
  /**
   * Set when the trip is to add a passkey, not to sign in. The signed-in
   * session, kept for the trip so the claim has its bearer on return.
   */
  readonly carried?: Carried;
}

/**
 * What a claim's first trip to a provider keeps. A claim needs the session
 * token as a bearer, and a hand-off is good once: signing in with it again
 * would spend it, and the claim needs it. So this one trip keeps the session
 * in `sessionStorage` (this tab only), until the person is back, then forgets
 * it at once. It is written only when the person presses "Add a passkey".
 */
export interface Carried {
  readonly token: string;
  readonly expiresAt: number;
  readonly sessionExpiresAt: number;
  readonly space: string;
  readonly via: string;
}

const PENDING_KEY = 'ordo-notes.outside';
const GUEST_KEY = 'ordo-notes.guest';
const DEVICE_KEY = 'ordo-notes.device';

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
  human?: string,
  hash = '',
  store: Store | undefined = session(),
  carried?: Carried,
): Promise<string> {
  const body: Record<string, unknown> = { method, return_to: returnTo };
  if (human) body['human'] = human;
  const begun = await call(endpoint, '/auth/begin', body);
  const redirect = begun['redirect'];
  const binder = begun['binder'];
  if (typeof redirect !== 'string' || typeof binder !== 'string') {
    throw new PasskeyError('malformed', "the door's answer has no `redirect` or `binder`");
  }
  write(store, PENDING_KEY, JSON.stringify({ method, binder, hash, ...(carried ? { carried } : {}) } satisfies Pending));
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
      /** Present when the trip was to add a passkey: do not sign in with the code. */
      readonly carried?: Carried;
    }
  | { readonly kind: 'error'; readonly error: string; readonly hash: string; readonly carried?: Carried };

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
      const c = parsed.carried;
      const carried =
        c &&
        typeof c.token === 'string' &&
        typeof c.expiresAt === 'number' &&
        typeof c.sessionExpiresAt === 'number' &&
        typeof c.space === 'string' &&
        typeof c.via === 'string'
          ? c
          : undefined;
      pending = {
        method: parsed.method,
        binder: parsed.binder,
        hash: typeof parsed.hash === 'string' ? parsed.hash : '',
        ...(carried ? { carried } : {}),
      };
    }
  } catch {
    pending = null;
  }
  const hash = pending?.hash ?? '';
  const carried = pending?.carried ? { carried: pending.carried } : {};
  if (error !== null) return { kind: 'error', error, hash, ...carried };
  // A code with no binder was not started here, or the tab was closed in
  // between. The door would refuse it, so it is not sent.
  if (pending === null || handoff === null) return { kind: 'error', error: 'authentication_failed', hash, ...carried };
  return { kind: 'handoff', method: pending.method, handoff, binder: pending.binder, hash, ...carried };
}

/** Swap the hand-off code and the binder for a session. Good once, within 60 seconds. */
export async function completeOutside(
  endpoint: PasskeyEndpoint,
  back: { readonly method: string; readonly handoff: string; readonly binder: string },
): Promise<SignInResult> {
  return result(
    await call(endpoint, '/auth/complete', {
      method: back.method,
      kind: 'handoff',
      handoff: back.handoff,
      binder: back.binder,
    }),
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
