/**
 * Link a sign-in to the account that is signed in: linking by claim.
 *
 * The person is signed in. They prove one sign-in the account already holds,
 * then they prove the new one. The door links the new one to the account.
 *
 * Two calls. `beginClaim` takes the first proof and yields a claim.
 * `completeClaim` takes the claim and the second proof, and links.
 *
 * | Function | Route | Yields |
 * |---|---|---|
 * | `beginClaim` | `POST /auth/claim/begin` | a `ClaimOpened`: a claim string and its expiry |
 * | `completeClaim` | `POST /auth/claim/complete` | a `ClaimLinked` |
 * | `claimPasskey` | `POST /auth/claim/passkey`, then `/auth/claim/complete` | a `ClaimLinked` for a new passkey |
 *
 * # The proofs
 *
 * Each proof is a `ClaimCeremony`: the body of an `/auth/complete` call for a
 * fresh ceremony. `emailCodeCeremony`, `handoffCeremony` and `passwordCeremony`
 * build the common ones. A passkey assertion is passed as its own body.
 *
 * The first proof must be a sign-in the account already holds. The second is
 * the sign-in being claimed.
 *
 * To claim an emailed code, begin it with `beginEmailSignIn` and pass the
 * claim string as `claim`. The door then mails an address it does not know.
 *
 * # The rules that make it work
 *
 * Every call carries the session token as a bearer. The token never goes in a
 * URL.
 *
 * The claim is short-lived. `expiresAt` says when. Finish before then.
 *
 * A wrong proof, a spent claim and a stale session are the same refusal
 * (`PasskeyError` with failure `refused`).
 *
 * The app developer can switch linking off with `ORDO_AUTH_LINKING`. The door
 * then answers `PasskeyError` with failure `linking_off`. Its `setting` and
 * `value` name what to change.
 *
 * A sign-in that already belongs to another account is `identifier_taken`. A
 * store that cannot be read is `unavailable`.
 */

import { attestation, call, text, PasskeyError, type PasskeyEndpoint, type Wire } from './passkey.js';

/**
 * One proof: the body of an `/auth/complete` call for a fresh ceremony, for
 * example `{ method: 'email', kind: 'email_code', challenge, binder, code }`.
 */
export type ClaimCeremony = Readonly<Record<string, unknown>>;

/** An open claim. Hold the string in memory until the second proof is ready. */
export interface ClaimOpened {
  readonly claim: string;
  /** When the claim stops working, in the door's time unit. */
  readonly expiresAt: number;
}

/** What `completeClaim` yields. */
export interface ClaimLinked {
  /** The authenticator that was linked. */
  readonly linked: string;
  /** How it signs in: `email-otp`, `password`, and so on. */
  readonly via: string;
  /** How many sign-ins the account holds now. Absent when `already` is true. */
  readonly authenticators?: number;
  /** True when the sign-in was already this account's. Nothing changed. */
  readonly already: boolean;
}

/** The proof of an emailed code. The code is trimmed. */
export function emailCodeCeremony(
  sent: { readonly challenge: string; readonly binder: string },
  code: string,
): ClaimCeremony {
  return {
    method: 'email',
    kind: 'email_code',
    challenge: sent.challenge,
    binder: sent.binder,
    code: code.trim(),
  };
}

/** The proof of a hand-off from another sign-in provider. */
export function handoffCeremony(method: string, handoff: string, binder: string): ClaimCeremony {
  return { method, kind: 'handoff', handoff, binder };
}

/** The proof of a password. `challenge` comes from `POST /auth/begin`. */
export function passwordCeremony(
  challenge: string,
  identifier: string,
  password: string,
): ClaimCeremony {
  return { method: 'password', kind: 'password', challenge, identifier, password };
}

/**
 * Open a claim. `token` is the session token. `held` proves a sign-in the
 * account already holds.
 */
export async function beginClaim(
  endpoint: PasskeyEndpoint,
  token: string,
  held: ClaimCeremony,
): Promise<ClaimOpened> {
  const begun = await call(endpoint, '/auth/claim/begin', { ...held } as Wire, token);
  const expires = begun['expires_at'];
  if (typeof expires !== 'number') {
    throw new PasskeyError('malformed', "the door's answer has no `expires_at`");
  }
  return { claim: text(begun, 'claim'), expiresAt: expires };
}

/**
 * Finish a claim. `claimed` proves the sign-in being linked. `claim` is the
 * string `beginClaim` returned.
 */
export async function completeClaim(
  endpoint: PasskeyEndpoint,
  token: string,
  claim: string,
  claimed: ClaimCeremony,
): Promise<ClaimLinked> {
  const done = await call(endpoint, '/auth/claim/complete', { ...claimed, claim } as Wire, token);
  const linked: { linked: string; via: string; already: boolean; authenticators?: number } = {
    linked: text(done, 'linked'),
    via: text(done, 'via'),
    already: done['already'] === true,
  };
  const held = done['authenticators'];
  if (typeof held === 'number') linked.authenticators = held;
  return linked;
}

/**
 * Add a new passkey to the signed-in account inside an open claim. Any
 * sign-in the account holds may have opened it, so a person who signed up
 * with Discord, an emailed code or a password can add a first passkey.
 *
 * Asks the door for creation options bound to this account
 * (`POST /auth/claim/passkey`), runs `navigator.credentials.create`, and
 * posts the attestation as the claimed proof. From then on the passkey is an
 * ordinary one: it signs in and proves presence.
 */
export async function claimPasskey(
  endpoint: PasskeyEndpoint,
  token: string,
  claim: string,
): Promise<ClaimLinked> {
  const options = await call(endpoint, '/auth/claim/passkey', { claim } as Wire, token);
  const { challenge, fields } = await attestation(endpoint, options);
  return completeClaim(endpoint, token, claim, {
    method: 'passkey',
    kind: 'passkey_registration',
    challenge,
    ...fields,
  });
}
