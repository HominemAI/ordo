/**
 * Sign in as a guest: a visitor with no email, password or passkey.
 *
 * The door makes a guest account and hands back a **guest secret**. The secret
 * is the only way back into that account. A guest can later upgrade by a claim
 * (see `claim.ts`), which links a real sign-in to the same account.
 *
 * | Function | Route | Yields |
 * |---|---|---|
 * | `startGuest` | `POST /auth/complete` with `kind: "new"` | a `GuestStarted`: the secret and a sign-in result |
 * | `resumeGuest` | `POST /auth/complete` with `kind: "resume"` | a sign-in result |
 * | `guestCeremony` | none | a `ClaimCeremony` that proves the guest to `beginClaim` |
 *
 * # The rules that make it work
 *
 * Guest sign-in is off by default. The app developer turns it on with
 * `ORDO_AUTH_GUEST=on` or the op `std.deployment.setGuest`. While it is off,
 * the door answers `PasskeyError` with failure `guests_off`. Its `setting` and
 * `value` name what to change.
 *
 * The secret is returned once, by `startGuest`. This client does not store it.
 * The page keeps it (for example in `localStorage`). It is the only way back in
 * until the guest claims another sign-in. Lose it and the account is gone.
 * Never put it in a URL or a log.
 *
 * Going over the sign-up ceiling is a `PasskeyError` with failure `refused`
 * like every other refusal. A missing human check is the same refusal.
 *
 * `human` is a Turnstile token. It is needed only when the deployment turned
 * the human check on.
 *
 * To upgrade a guest, pass `guestCeremony(secret)` to `beginClaim` as the
 * proof the account already holds.
 */

import { call, PasskeyError, type PasskeyEndpoint, type SignInOptions } from './passkey.js';
import { named, result, type SignInResult } from './second-step.js';
import type { ClaimCeremony } from './claim.js';

/** What the page passes to start a guest. All of it is optional. */
export interface GuestStart {
  /** The Turnstile token, for a deployment that turned the human check on. */
  readonly human?: string;
  /** A display name for the guest. */
  readonly display?: string;
  /** A tenant the guest is arriving at. */
  readonly space?: string;
  /** An invitation ticket for `space`. */
  readonly invitation?: string;
}

/** A new guest. The page must keep `secret`: it is shown once. */
export interface GuestStarted {
  /** The guest secret. Keep it (for example in `localStorage`). Never put it in a URL. */
  readonly secret: string;
  /** The sign-in: a session, or a `SecondStep`. */
  readonly result: SignInResult;
}

/**
 * Start a guest account. The secret comes back once and is not stored here.
 * The page keeps it. It is the only way back in until the guest claims another
 * sign-in.
 */
export async function startGuest(
  endpoint: PasskeyEndpoint,
  start: GuestStart = {},
  options: SignInOptions = {},
): Promise<GuestStarted> {
  const body: Record<string, unknown> = { method: 'guest', kind: 'new' };
  if (start.human) body['human'] = start.human;
  if (start.display) body['display'] = start.display;
  if (start.space) body['space'] = start.space;
  if (start.invitation) body['invitation'] = start.invitation;
  const answer = await call(endpoint, '/auth/complete', named(body, options));
  const secret = answer['guest_secret'];
  if (typeof secret !== 'string' || secret === '') {
    throw new PasskeyError('malformed', "the door's answer has no `guest_secret`");
  }
  return { secret, result: result(answer) };
}

/** Come back as a guest with the secret `startGuest` returned. */
export async function resumeGuest(
  endpoint: PasskeyEndpoint,
  secret: string,
  options: SignInOptions = {},
): Promise<SignInResult> {
  const body = named({ method: 'guest', kind: 'resume', secret }, options);
  return result(await call(endpoint, '/auth/complete', body));
}

/** The proof of a guest secret, for `beginClaim` or `completeClaim`. */
export function guestCeremony(secret: string): ClaimCeremony {
  return { method: 'guest', kind: 'resume', secret };
}
