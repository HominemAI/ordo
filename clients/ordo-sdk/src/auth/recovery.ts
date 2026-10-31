/**
 * The browser half of recovery: what a person and an application call when a
 * passkey is lost, and to keep one from being.
 *
 * | Function | Routes | What it does |
 * |---|---|---|
 * | `listAuthenticators` | `GET /auth/authenticators` | the person's ways in, with a warning when there is one |
 * | `linkPasskey` | `POST /auth/authenticators/link/begin`, `/link` | adds a passkey, vouched by one they hold |
 * | `removePasskey` | `POST /auth/step-up/begin`, `/auth/authenticators/unlink` | removes one, its key with it |
 * | `endEverySession` | `POST /auth/logout-all` | signs out everywhere |
 * | `issueRecoveryCodes` | `POST /auth/recovery-codes` | ten codes, shown once |
 * | `redeemRecoveryCode` | `POST /auth/recover/code` | spends one; opens a recovery |
 * | `recoverWithTicket` | `POST /auth/recover/begin`, `/complete` | registers the new passkey a recovery ends in |
 * | `contestRecovery` | `POST /auth/begin`, `/auth/recover/contest` | a passkey a recovery retired says no |
 * | `acknowledgeRecovery` | `POST /auth/recover/ack` | the person has seen a recovery of theirs |
 *
 * Ordo provides the mechanisms and the application decides what is enforced.
 * Nothing here is compulsory: an application offers a second passkey and codes
 * when `notices` says a person holds one way in, and a person may say no.
 *
 * The rules in `passkey.ts` hold here too: `clientDataJSON` is posted as the
 * exact bytes the browser produced, the challenge is used two ways, and every
 * byte string is base64url without padding. A recovery code is a secret on
 * paper. It is never logged and never stored here; the caller decides what to
 * do with the ten strings `issueRecoveryCodes` returns, and the door will not
 * show them again.
 */

import {
  PasskeyError,
  assertion,
  attestation,
  call,
  list,
  text,
  type PasskeyEndpoint,
  type Wire,
} from './passkey.js';

/** One way in, as the door lists it. */
export interface AuthenticatorRow {
  readonly id: string;
  /** `passkey`, `password` or `other`. */
  readonly kind: string;
  /** `active`, or `retired` while a recovery holds it for a contest. */
  readonly state: string;
  /** For a passkey: the platform says it can sync. */
  readonly backupEligible?: boolean | undefined;
  /** For a passkey: it was already synced when it was made. */
  readonly backedUp?: boolean | undefined;
  /** Set on a passkey a recovery added and that cannot yet satisfy step-up. */
  readonly presenceAvailableAt?: number | undefined;
  /** Set on a retired passkey: when it stops being able to contest. */
  readonly contestUntil?: number | undefined;
}

export interface AuthenticatorList {
  readonly authenticators: readonly AuthenticatorRow[];
  /** `single_authenticator`, `recovery_codes_low`. Information only. */
  readonly notices: readonly string[];
  /** How many unused recovery codes the person holds, when the deployment has them. */
  readonly recoveryCodesRemaining?: number | undefined;
}

function optNumber(o: Wire, key: string): number | undefined {
  const v = o[key];
  return typeof v === 'number' ? v : undefined;
}

function optBool(o: Wire, key: string): boolean | undefined {
  const v = o[key];
  return typeof v === 'boolean' ? v : undefined;
}

/** The person's own ways in. */
export async function listAuthenticators(
  endpoint: PasskeyEndpoint,
  token: string,
): Promise<AuthenticatorList> {
  const o = await call(endpoint, '/auth/authenticators', null, token, 'GET');
  const rows = Array.isArray(o['authenticators']) ? (o['authenticators'] as Wire[]) : [];
  return {
    authenticators: rows.map((r) => ({
      id: text(r, 'id'),
      kind: text(r, 'kind'),
      state: text(r, 'state'),
      backupEligible: optBool(r, 'backup_eligible'),
      backedUp: optBool(r, 'backed_up'),
      presenceAvailableAt: optNumber(r, 'presence_available_at'),
      contestUntil: optNumber(r, 'contest_until'),
    })),
    notices: list(o, 'notices'),
    recoveryCodesRemaining: optNumber(o, 'recovery_codes_remaining'),
  };
}

/** Sign the person out on every device, this one included. */
export async function endEverySession(endpoint: PasskeyEndpoint, token: string): Promise<void> {
  await call(endpoint, '/auth/logout-all', null, token);
}

/**
 * Add a passkey. Runs two ceremonies: the browser creates the new passkey, and
 * a passkey the person already holds vouches for it with a fresh assertion. A
 * stolen session token alone cannot do this, and the audit event names the
 * passkey that vouched.
 */
export async function linkPasskey(
  endpoint: PasskeyEndpoint,
  token: string,
): Promise<{ authenticator: string; authenticators: number }> {
  const begun = await call(endpoint, '/auth/authenticators/link/begin', null, token);
  const creation = (begun['registration'] ?? {}) as Wire;
  const request = (begun['vouch'] ?? {}) as Wire;
  const created = await attestation(endpoint, creation);
  const vouched = await assertion(endpoint, request);
  const done = await call(
    endpoint,
    '/auth/authenticators/link',
    {
      registration: { challenge: created.challenge, ...created.fields },
      vouch: { challenge: vouched.challenge, ...vouched.fields },
    },
    token,
  );
  const n = done['authenticators'];
  return {
    authenticator: text(done, 'authenticator'),
    authenticators: typeof n === 'number' ? n : 0,
  };
}

/**
 * Remove a passkey the person lost, with one they still hold. The link and the
 * public key both go. **Every session ends by default**, this one included, so
 * a thief's session ends with it; pass `endSessions: false` to keep them. The
 * last way in is refused.
 */
export async function removePasskey(
  endpoint: PasskeyEndpoint,
  token: string,
  authenticator: string,
  options: { readonly endSessions?: boolean } = {},
): Promise<{ removed: string; sessionsEnded: boolean }> {
  const begun = await call(endpoint, '/auth/step-up/begin', null, token);
  const vouched = await assertion(endpoint, begun);
  const body: Wire = {
    authenticator,
    vouch: { challenge: vouched.challenge, ...vouched.fields },
  };
  if (options.endSessions !== undefined) body['end_sessions'] = options.endSessions;
  const done = await call(endpoint, '/auth/authenticators/unlink', body, token);
  return {
    removed: text(done, 'removed'),
    sessionsEnded: done['sessions_ended'] === true,
  };
}

/**
 * Ask for a new set of ten recovery codes. Needs fresh presence, so call
 * `stepUp` first for a session that has none. **The codes are in this answer
 * once and the door never shows them again.** The old set is void.
 */
export async function issueRecoveryCodes(
  endpoint: PasskeyEndpoint,
  token: string,
): Promise<readonly string[]> {
  const o = await call(endpoint, '/auth/recovery-codes', null, token);
  return list(o, 'codes');
}

/** What a recovery gave the person who started it. */
export interface RecoveryStart {
  readonly case: string;
  /** Spend it with `recoverWithTicket`. Absent when the case has a delay to wait out. */
  readonly ticket?: string | undefined;
  readonly ticketExpiresAt?: number | undefined;
  /** When the case may yield a ticket. */
  readonly maturesAt: number;
  /** How many codes the person has left. */
  readonly remaining: number;
}

/**
 * Spend a recovery code. A code alone opens a recovery and nothing else: what
 * comes back is a ticket for a new passkey, and everything a recovery does
 * follows. It does not sign anybody in.
 */
export async function redeemRecoveryCode(
  endpoint: PasskeyEndpoint,
  code: string,
): Promise<RecoveryStart> {
  const o = await call(endpoint, '/auth/recover/code', { code });
  const num = (k: string): number => {
    const v = o[k];
    if (typeof v !== 'number') throw new PasskeyError('malformed', `the door's answer has no \`${k}\``);
    return v;
  };
  return {
    case: text(o, 'case'),
    ticket: typeof o['ticket'] === 'string' ? o['ticket'] : undefined,
    ticketExpiresAt: optNumber(o, 'ticket_expires_at'),
    maturesAt: num('matures_at'),
    remaining: num('remaining'),
  };
}

/** What a completed recovery did. No token: sign in with the new passkey. */
export interface Recovered {
  readonly case: string;
  readonly authenticator: string;
  /** The passkeys the recovery retired. They can contest it, and nothing else. */
  readonly retired: readonly string[];
  /** From then, the new passkey can satisfy step-up. Until then it cannot. */
  readonly presenceAvailableAt: number;
  /** Until then, a retired passkey may contest. */
  readonly contestUntil: number;
  readonly warnings: readonly string[];
}

/**
 * Register the new passkey a recovery ends in, with the ticket the recovery
 * gave. Every session the person had ends when this completes, so the next
 * step is `signInWithPasskey`. `lost` names the passkeys to retire; leave it out
 * to retire all of them.
 */
export async function recoverWithTicket(
  endpoint: PasskeyEndpoint,
  ticket: string,
  options: { readonly lost?: readonly string[] } = {},
): Promise<Recovered> {
  const begun = await call(endpoint, '/auth/recover/begin', { ticket });
  const created = await attestation(endpoint, begun);
  const body: Wire = {
    method: 'passkey',
    kind: 'passkey_registration',
    challenge: created.challenge,
    ticket,
    ...created.fields,
  };
  if (options.lost) body['lost'] = [...options.lost];
  const o = await call(endpoint, '/auth/recover/complete', body);
  const num = (k: string): number => {
    const v = o[k];
    if (typeof v !== 'number') throw new PasskeyError('malformed', `the door's answer has no \`${k}\``);
    return v;
  };
  return {
    case: text(o, 'case'),
    authenticator: text(o, 'authenticator'),
    retired: list(o, 'retired'),
    presenceAvailableAt: num('presence_available_at'),
    contestUntil: num('contest_until'),
    warnings: list(o, 'warnings'),
  };
}

/**
 * Say no to a recovery. A passkey a recovery **retired** contests a completed
 * one: what it added is removed, what it retired comes back, and every session
 * ends again. A passkey still **active** cancels one that has not completed.
 * The ceremony is an ordinary assertion.
 */
export async function contestRecovery(
  endpoint: PasskeyEndpoint,
): Promise<{ readonly contested?: string; readonly cancelled?: string }> {
  const begun = await call(endpoint, '/auth/begin', { method: 'passkey' });
  const { challenge, fields } = await assertion(endpoint, begun);
  const o = await call(endpoint, '/auth/recover/contest', {
    method: 'passkey',
    kind: 'passkey_assertion',
    challenge,
    ...fields,
  });
  const out: { contested?: string; cancelled?: string } = {};
  if (typeof o['contested'] === 'string') out.contested = o['contested'];
  if (typeof o['cancelled'] === 'string') out.cancelled = o['cancelled'];
  return out;
}

/**
 * The person has seen a recovery of theirs. Until they say so every sign-in
 * carries it (`PasskeySession.recoveries`). Needs a session with fresh
 * presence, so a session an intruder holds cannot clear the notice.
 */
export async function acknowledgeRecovery(
  endpoint: PasskeyEndpoint,
  token: string,
  recoveryCase: string,
): Promise<void> {
  await call(endpoint, '/auth/recover/ack', { case: recoveryCase }, token);
}
