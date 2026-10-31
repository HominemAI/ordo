/**
 * A signed-in person's own email address (owner ruling 2026-10-22).
 *
 * A person who signs in by passkey or an outside connection has no address
 * Ordo knows, so mail from the app (a reminder, a routine's `notify`) cannot
 * reach them. These calls let them add one. It counts only once they type
 * the code mailed to it.
 *
 * | Function | Route | Yields |
 * |---|---|---|
 * | `accountEmail` | `GET /auth/email` | an `AccountEmail`: the address, and what the app requires |
 * | `beginAccountEmail` | `POST /auth/email/begin` | an `EmailCodeSent`: a code is on its way |
 * | `confirmAccountEmail` | `POST /auth/email/complete` | the address, now the account's |
 * | `removeAccountEmail` | `POST /auth/email/remove` | whether there was one |
 *
 * A wrong, expired or used code is the one refusal (`PasskeyError`, failure
 * `refused`). A deployment that sends no mail answers `email_unavailable`,
 * with `setting` naming `ORDO_MAIL_KIND`.
 *
 * Changing the address mails the old one to say so. Removing it does too.
 *
 * # When the app requires one
 *
 * The app developer may require a verified address for some ops, or for all
 * of the app's ops (`ORDO_EMAIL_REQUIRED`, `std.deployment.setEmailPolicy`).
 * A person without one calling such an op gets an `OrdoRefusal` of class
 * `email_required`; {@link isEmailRequired} tells it apart. `accountEmail`
 * says what is required, so a page can ask before the refusal.
 */

import type { TokenSource } from '../core/client.js';
import { OrdoNetworkError } from '../core/network.js';
import { OrdoRefusal, failureDetail } from '../core/refusal.js';
import type { EmailCodeSent } from './email.js';
import { call, PasskeyError, text, type PasskeyEndpoint, type Wire } from './passkey.js';

/** The person's address, and what the app developer requires. */
export interface AccountEmail {
  /** The verified address, or `null` when Ordo knows none. */
  readonly address: string | null;
  /**
   * Where it came from: `code` (the person confirmed it), `sign-in` (they sign
   * in with it by emailed code), or the name of the connection that vouched
   * for it, such as `discord`. `null` with no address.
   */
  readonly source: string | null;
  /** When it was confirmed, in Unix seconds. `null` with no address. */
  readonly verifiedAt: number | null;
  /** Which ops need an address: `'off'`, `'all'` (the app's ops), or a list. */
  readonly required: 'off' | 'all' | readonly string[];
  /** The setting that says so: `ORDO_EMAIL_REQUIRED` or `std.deployment.setEmailPolicy`. */
  readonly setting: string;
  /** Whether the deployment sends mail. Without it no address can be added. */
  readonly mail: boolean;
}

async function tokenOf(token: TokenSource): Promise<string> {
  return typeof token === 'string' ? token : token();
}

function emailOf(w: Wire): AccountEmail {
  const str = (k: string) => (typeof w[k] === 'string' ? (w[k] as string) : null);
  const r = w['required'];
  let required: AccountEmail['required'];
  if (r === 'off' || r === 'all') required = r;
  else if (Array.isArray(r) && r.every((x) => typeof x === 'string')) required = r as string[];
  else throw new PasskeyError('malformed', 'the door answered an email setting this client cannot read');
  return {
    address: str('address'),
    source: str('source'),
    verifiedAt: typeof w['verified_at'] === 'number' ? (w['verified_at'] as number) : null,
    required,
    setting: str('setting') ?? '',
    mail: w['mail'] === true,
  };
}

/** The signed-in person's address, and what the app developer requires. */
export async function accountEmail(endpoint: PasskeyEndpoint, token: TokenSource): Promise<AccountEmail> {
  return emailOf(await call(endpoint, '/auth/email', null, await tokenOf(token), 'GET'));
}

/**
 * Mail a code to `address`. Hold the answer in memory until the person types the code.
 *
 * The two-argument form, `(endpoint, address)` with the token inside the
 * endpoint, is `@hominemai/ordo-react` 0.3's, kept so its callers keep
 * working: it throws {@link AccountEmailError}.
 */
export function beginAccountEmail(endpoint: PasskeyEndpoint, token: TokenSource, address: string): Promise<EmailCodeSent>;
/** @deprecated The ordo-react 0.3 form. Pass the token on its own: `(endpoint, token, address)`. */
export function beginAccountEmail(endpoint: AccountEmailEndpoint, address: string): Promise<EmailCodeSent>;
export async function beginAccountEmail(
  endpoint: PasskeyEndpoint | AccountEmailEndpoint,
  ...rest: [TokenSource, string] | [string]
): Promise<EmailCodeSent> {
  if (rest.length === 1) {
    const legacy = endpoint as AccountEmailEndpoint;
    return asAccountEmailError(() => beginAccountEmail(legacy, legacy.token, rest[0]));
  }
  const [token, address] = rest;
  const begun = await call(endpoint, '/auth/email/begin', { address: address.trim() }, await tokenOf(token));
  return { challenge: text(begun, 'challenge'), binder: text(begun, 'binder') };
}

/** What `confirmAccountEmail` answers. */
export interface ConfirmedEmail {
  readonly address: string;
  /** True when it replaced another address, which was told. */
  readonly replaced: boolean;
}

/**
 * Send the code the person received. The address is the account's from now on.
 *
 * The three-argument form, `(endpoint, sent, code)` with the token inside
 * the endpoint, is ordo-react 0.3's: it answers the address alone and
 * throws {@link AccountEmailError}.
 */
export function confirmAccountEmail(
  endpoint: PasskeyEndpoint,
  token: TokenSource,
  sent: EmailCodeSent,
  code: string,
): Promise<ConfirmedEmail>;
/** @deprecated The ordo-react 0.3 form. Pass the token on its own: `(endpoint, token, sent, code)`. */
export function confirmAccountEmail(endpoint: AccountEmailEndpoint, sent: EmailCodeSent, code: string): Promise<string>;
export async function confirmAccountEmail(
  endpoint: PasskeyEndpoint | AccountEmailEndpoint,
  ...rest: [TokenSource, EmailCodeSent, string] | [EmailCodeSent, string]
): Promise<ConfirmedEmail | string> {
  if (rest.length === 2) {
    const legacy = endpoint as AccountEmailEndpoint;
    const [sent, code] = rest;
    return asAccountEmailError(async () => (await confirmAccountEmail(legacy, legacy.token, sent, code)).address);
  }
  const [token, sent, code] = rest;
  const done = await call(
    endpoint,
    '/auth/email/complete',
    { challenge: sent.challenge, binder: sent.binder, code: code.trim() },
    await tokenOf(token),
  );
  return { address: text(done, 'address'), replaced: done['replaced'] === true };
}

/**
 * Take the added address off. Answers whether there was one. The old address is told.
 * `(endpoint)` with the token inside is ordo-react 0.3's form.
 */
export function removeAccountEmail(endpoint: PasskeyEndpoint, token: TokenSource): Promise<boolean>;
/** @deprecated The ordo-react 0.3 form. Pass the token on its own: `(endpoint, token)`. */
export function removeAccountEmail(endpoint: AccountEmailEndpoint): Promise<boolean>;
export async function removeAccountEmail(
  endpoint: PasskeyEndpoint | AccountEmailEndpoint,
  token?: TokenSource,
): Promise<boolean> {
  if (token === undefined) {
    const legacy = endpoint as AccountEmailEndpoint;
    return asAccountEmailError(() => removeAccountEmail(legacy, legacy.token));
  }
  const done = await call(endpoint, '/auth/email/remove', {}, await tokenOf(token));
  return done['removed'] === true;
}

/** Is this the refusal for an op that needs a verified address the person lacks? */
export function isEmailRequired(e: unknown): e is OrdoRefusal {
  return e instanceof OrdoRefusal && e.detail.class === 'email_required';
}

/** Does the app require an address of this person that they do not have? */
export function needsAccountEmail(e: AccountEmail | null): boolean {
  return e !== null && e.address === null && e.required !== 'off';
}

// ── the ordo-react 0.3 form ───────────────────────────────────────────────

/** Where the door is, and who is asking: the endpoint with the token inside. */
export interface AccountEmailEndpoint extends PasskeyEndpoint {
  /** The person's token, or a function that gives the current one. */
  readonly token: TokenSource;
}

/** What went wrong, in the ordo-react 0.3 words. */
export type AccountEmailProblem =
  /** The session, or the code, was refused. A wrong code is this. */
  | 'refused'
  /** The deployment sends no mail; `setting` names `ORDO_MAIL_KIND`. */
  | 'email_unavailable'
  /** Busy, slow or unreachable for a moment. Try again. */
  | 'unavailable'
  /** The door answered something this package cannot read. */
  | 'malformed';

/**
 * A refusal of an account-email call in its ordo-react 0.3 form, and in
 * `useAccountEmail`. An {@link OrdoRefusal}: `code` and `status` are the
 * door's when it answered; `problem` says what to do.
 */
export class AccountEmailError extends OrdoRefusal<string> {
  readonly problem: AccountEmailProblem;
  /** For `email_unavailable`: the setting the deployment owner changes. */
  readonly setting?: string;
  constructor(problem: AccountEmailProblem, message: string, setting?: string, cause?: unknown) {
    const wire = cause instanceof OrdoRefusal ? { status: cause.status, code: cause.code } : {};
    super(failureDetail(problem, message, wire), cause instanceof OrdoRefusal ? cause.retryAfterMs : null, {
      message,
      cause,
    });
    this.name = 'AccountEmailError';
    this.problem = problem;
    if (setting !== undefined) this.setting = setting;
  }
}

/** The person's address, and what the app developer requires. ordo-react 0.3's name for {@link accountEmail}. */
export async function readAccountEmail(endpoint: AccountEmailEndpoint): Promise<AccountEmail> {
  return asAccountEmailError(() => accountEmail(endpoint, endpoint.token));
}

/** Run a call and say any failure in the ordo-react 0.3 words. */
async function asAccountEmailError<T>(run: () => Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (e) {
    throw accountEmailErrorOf(e);
  }
}

function accountEmailErrorOf(e: unknown): unknown {
  if (e instanceof AccountEmailError) return e;
  if (e instanceof PasskeyError) {
    switch (e.failure) {
      case 'email_unavailable':
        return new AccountEmailError(
          'email_unavailable',
          'this deployment sends no mail, so no address can be confirmed',
          e.setting,
          e,
        );
      case 'unavailable':
        return new AccountEmailError('unavailable', 'the door cannot answer right now', undefined, e);
      case 'malformed':
        return new AccountEmailError('malformed', e.message, undefined, e);
      default:
        return new AccountEmailError('refused', 'that did not work: the session or the code was refused', undefined, e);
    }
  }
  // Busy past its retries, or no answer at all: try again in a moment.
  if (e instanceof OrdoRefusal || e instanceof OrdoNetworkError) {
    return new AccountEmailError('unavailable', 'the door cannot answer right now', undefined, e);
  }
  return e;
}
