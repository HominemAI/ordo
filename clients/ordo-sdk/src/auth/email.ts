/**
 * Sign in with a code sent by email.
 *
 * Two calls. `beginEmailSignIn` asks the door to mail a code. `completeEmailSignIn`
 * sends the code back and yields what a password sign-in yields: a session, or a
 * `SecondStep`.
 *
 * | Function | Route | Yields |
 * |---|---|---|
 * | `beginEmailSignIn` | `POST /auth/begin` | an `EmailCodeSent`: a challenge and a binder |
 * | `completeEmailSignIn` | `POST /auth/complete` | a session, or a `SecondStep` |
 *
 * # The rules that make it work
 *
 * The door answers `begin` the same way for every address. No code is in the
 * answer, and the answer does not say whether the address is known. A malformed
 * address is the one refusal, the same as every other failure.
 *
 * The **binder** is a second secret that goes with the challenge. Keep both in
 * memory in the `EmailCodeSent`, and post both with the code. Never put the
 * binder in a URL, a log or storage.
 *
 * A wrong, expired or used code, and a wrong binder, are the same refusal
 * (`PasskeyError` with failure `refused`). Nothing says which one it was.
 *
 * A session may carry a `device` marker. This client does not store it. The
 * page keeps it (for example in `localStorage`) and passes it as `device` to the
 * next `beginEmailSignIn`.
 *
 * Under invite-only provisioning, an invited newcomer passes `space` and
 * `invitation` to both calls.
 *
 * `human` is a Turnstile token. It is needed only when the deployment turned
 * the human check on.
 */

import { call, text, type PasskeyEndpoint, type SignInOptions } from './passkey.js';
import { named, result, type SignInResult } from './second-step.js';

/** What the page passes to begin an email sign-in. */
export interface EmailSignInStart {
  /** The address the code is mailed to. */
  readonly address: string;
  /** The Turnstile token, for a deployment that turned the human check on. */
  readonly human?: string;
  /** The `device` marker an earlier sign-in returned, if the page kept one. */
  readonly device?: string;
  /** A tenant the person is arriving at. */
  readonly space?: string;
  /**
   * An invitation ticket for `space`. Under invite-only, an unknown address is
   * mailed only with one. Pass the same `space` and `invitation` to
   * `completeEmailSignIn`.
   */
  readonly invitation?: string;
  /**
   * The claim string `beginClaim` returned (see `claim.ts`). It lets the door
   * mail an address it does not know, so the address can be linked to a
   * signed-in account.
   */
  readonly claim?: string;
}

/** What `completeEmailSignIn` may also send. */
export interface EmailComplete extends SignInOptions {
  /**
   * The name a new person is known by (`displayName`). The door uses it only
   * when this sign-in makes the account; a known person keeps their name.
   */
  readonly display?: string;
}

/** A code is on its way. Hold this in memory until the person types the code. */
export interface EmailCodeSent {
  readonly challenge: string;
  /** The second secret that goes with the challenge. Never put it in a URL. */
  readonly binder: string;
}

/** Ask the door to mail a code to `address`. */
export async function beginEmailSignIn(
  endpoint: PasskeyEndpoint,
  start: EmailSignInStart,
): Promise<EmailCodeSent> {
  const body: Record<string, unknown> = { method: 'email', identifier: start.address };
  if (start.human) body['human'] = start.human;
  if (start.device) body['device'] = start.device;
  if (start.space) body['space'] = start.space;
  if (start.invitation) body['invitation'] = start.invitation;
  if (start.claim) body['claim'] = start.claim;
  const begun = await call(endpoint, '/auth/begin', body);
  return { challenge: text(begun, 'challenge'), binder: text(begun, 'binder') };
}

/**
 * Send the code the person received. The code is trimmed. A deployment that
 * asks for a second step answers a `SecondStep`. A session may carry `device`.
 * `options.display` names a new person.
 */
export async function completeEmailSignIn(
  endpoint: PasskeyEndpoint,
  sent: EmailCodeSent,
  code: string,
  options: EmailComplete = {},
): Promise<SignInResult> {
  const body = named(
    {
      method: 'email',
      kind: 'email_code',
      challenge: sent.challenge,
      binder: sent.binder,
      code: code.trim(),
    },
    options,
  );
  const display = options.display?.trim();
  if (display) body['display'] = display;
  return result(await call(endpoint, '/auth/complete', body));
}
