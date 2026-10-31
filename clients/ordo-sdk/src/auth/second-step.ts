/**
 * The second step: what a client does when a sign-in that succeeded is not yet
 * a session, and how a person manages the factors that step asks for.
 *
 * A sign-in whose door requires a second step answers `200` with **no token**:
 * a handle, a binder, and the methods on offer. Everything below is the loop
 * that follows.
 *
 * | Function | Routes | Yields |
 * |---|---|---|
 * | `signInWithPassword` | `POST /auth/begin`, `/auth/complete` | a session, or a `SecondStep` |
 * | `signInWithPasskeyStep` | `POST /auth/begin`, `/auth/complete` | a session, or a `SecondStep` (a passkey owes one only when a person or a tenant turned it on) |
 * | `completeSecondStep` | `POST /auth/second-step` (and `/begin` for a key) | the session, or a proof when the step was for a change |
 * | `reauthenticateWithPassword`, `reauthenticateWithPasskey` | `POST /auth/complete` with `reauth` | a proof, or a `SecondStep` that ends in one |
 * | `listFactors`, `removeFactor`, `setSecondStepLevel` | `GET /auth/factors`, `POST /auth/factors/{id}/remove`, `/policy` | the person's factors, and the changes to them |
 * | `beginTotpEnrolment`, `confirmTotpEnrolment`, `enrolSecurityKey` | `POST /auth/factors/totp/*`, `/key/*` | a factor |
 *
 * # The rules that make it verify
 *
 * A security key's answer is the same WebAuthn assertion a passkey gives, so it
 * is encoded by the passkey module's code and by nothing else: the challenge the
 * door sent is decoded to the bytes the key signs over, `clientDataJSON` is
 * posted as the exact bytes the browser produced, and every byte string is
 * base64url without padding. Nothing here re-encodes any of it.
 *
 * A **binder** is a second secret the door hands back beside the handle, so a
 * handle seen in a log is worthless alone. This client keeps the two together in
 * a `SecondStep` and posts both every time. It never stores either.
 *
 * A **proof** (`reauth`) is a handle for one change: it comes from
 * `reauthenticateWith*`, goes in the `x-reauth` header, and the door spends it
 * on the act it was sent with.
 *
 * # What this does not do
 *
 * It decides nothing about who is asked for a second step. The door does, from
 * the app developer's setting, the tenant's and the person's own, and this
 * client only follows what it is told. A code the person types is sent as typed,
 * trimmed; the door normalises it.
 */

import {
  PasskeyError,
  assertion,
  attestation,
  call,
  list,
  session,
  text,
  type PasskeyEndpoint,
  type PasskeySession,
  type SignInOptions,
  type Wire,
} from './passkey.js';

/** A way to answer a second step. */
export type SecondStepMethod = 'totp' | 'security-key' | 'backup-code';

/** What a person has to do to finish a sign-in that is not yet a session. */
export interface SecondStep {
  /** Names the half-signed-in state. Not a secret on its own. */
  readonly handle: string;
  /** The second secret that goes with it. Post both, every time. */
  readonly binder: string;
  /** When the state stops answering, seconds since the epoch. */
  readonly expiresAt: number;
  /** What the person may answer with. Empty when `enrol` is set. */
  readonly methods: readonly SecondStepMethod[];
  /**
   * The person holds no factor and a level requires one. They enrol one with
   * `beginTotpEnrolment` or `enrolSecurityKey` (passing this step as `who`), then
   * answer with it. Nobody is signed in without a factor, and nobody is locked
   * out for lacking one.
   */
  readonly enrol: boolean;
}

/** A sign-in that succeeded: a session, or a second step still to take. */
export type SignInResult =
  | { readonly kind: 'session'; readonly session: PasskeySession }
  | { readonly kind: 'second-step'; readonly step: SecondStep };

/** A person's password, as a deployment with a password method takes it. */
export interface PasswordCredentials {
  readonly identifier: string;
  readonly password: string;
}

/** One answer to a second step. */
export type SecondStepAnswer =
  | { readonly kind: 'totp'; readonly code: string }
  | { readonly kind: 'backup-code'; readonly code: string }
  | { readonly kind: 'security-key' };

/** What completing a second step produced. */
export type SecondStepDone =
  | { readonly kind: 'session'; readonly session: PasskeySession; readonly via: string }
  | {
      /** The step was for a change to a factor: this is its proof, not a session. */
      readonly kind: 'proof';
      readonly reauth: string;
      readonly expiresAt: number;
    };

/** A fresh first factor for one change, or a second step that ends in one. */
export type ReauthResult =
  | { readonly kind: 'proof'; readonly reauth: string; readonly expiresAt: number }
  | { readonly kind: 'second-step'; readonly step: SecondStep };

const METHODS: readonly SecondStepMethod[] = ['totp', 'security-key', 'backup-code'];

function stepOf(o: Wire): SecondStep {
  const expires = o['expires_at'];
  return {
    handle: text(o, 'second_step'),
    binder: text(o, 'binder'),
    expiresAt: typeof expires === 'number' ? expires : 0,
    methods: list(o, 'methods').filter((m): m is SecondStepMethod =>
      (METHODS as readonly string[]).includes(m),
    ),
    enrol: o['enrol'] === true,
  };
}

/** A token, or a second step: whichever `/auth/complete` answered. */
export function result(o: Wire): SignInResult {
  if (typeof o['second_step'] === 'string') return { kind: 'second-step', step: stepOf(o) };
  return { kind: 'session', session: session(o) };
}

export function named(body: Wire, options: SignInOptions): Wire {
  if (options.space) body['space'] = options.space;
  if (options.invitation) body['invitation'] = options.invitation;
  return body;
}

// ── signing in ───────────────────────────────────────────────────────────

/** Sign in with a password. A deployment that asks for a second step answers a `SecondStep`. */
export async function signInWithPassword(
  endpoint: PasskeyEndpoint,
  credentials: PasswordCredentials,
  options: SignInOptions = {},
): Promise<SignInResult> {
  const begun = await call(endpoint, '/auth/begin', {
    method: 'password',
    identifier: credentials.identifier,
  });
  const body = named(
    {
      method: 'password',
      kind: 'password',
      challenge: text(begun, 'challenge'),
      identifier: credentials.identifier,
      password: credentials.password,
    },
    options,
  );
  return result(await call(endpoint, '/auth/complete', body));
}

/**
 * Sign in with a passkey the person already has. A passkey owes a second step
 * only when the person or the tenant turned it on, so most of the time the
 * answer is a session.
 */
export async function signInWithPasskeyStep(
  endpoint: PasskeyEndpoint,
  options: SignInOptions = {},
): Promise<SignInResult> {
  const begun = await call(endpoint, '/auth/begin', { method: 'passkey' });
  const { challenge, fields } = await assertion(endpoint, begun);
  const body = named(
    { method: 'passkey', kind: 'passkey_assertion', challenge, ...fields },
    options,
  );
  return result(await call(endpoint, '/auth/complete', body));
}

// ── answering ────────────────────────────────────────────────────────────

/**
 * Answer a second step. A code is one request. A security key is two: the door
 * mints a challenge narrowed to the person's own keys, the browser signs it, and
 * the assertion goes back. The wrong answer, a handle that expired and a handle
 * without its binder are one refusal, and after too many wrong answers the
 * person signs in again.
 */
export async function completeSecondStep(
  endpoint: PasskeyEndpoint,
  step: SecondStep,
  answer: SecondStepAnswer,
): Promise<SecondStepDone> {
  const base: Wire = { handle: step.handle, binder: step.binder };
  let body: Wire;
  if (answer.kind === 'security-key') {
    const begun = await call(endpoint, '/auth/second-step/begin', {
      ...base,
      kind: 'security-key',
    });
    const { challenge, fields } = await assertion(endpoint, begun);
    body = { ...base, kind: 'passkey_assertion', challenge, ...fields };
  } else {
    body = {
      ...base,
      kind: answer.kind === 'backup-code' ? 'backup_code' : 'totp',
      code: answer.code.trim(),
    };
  }
  const o = await call(endpoint, '/auth/second-step', body);
  if (typeof o['reauth'] === 'string') {
    const expires = o['expires_at'];
    return {
      kind: 'proof',
      reauth: o['reauth'],
      expiresAt: typeof expires === 'number' ? expires : 0,
    };
  }
  return { kind: 'session', session: session(o), via: text(o, 'second_step') };
}

// ── proving a first factor again ─────────────────────────────────────────

function proofOrStep(o: Wire): ReauthResult {
  if (typeof o['reauth'] === 'string') {
    const expires = o['expires_at'];
    return {
      kind: 'proof',
      reauth: o['reauth'],
      expiresAt: typeof expires === 'number' ? expires : 0,
    };
  }
  if (typeof o['second_step'] === 'string') return { kind: 'second-step', step: stepOf(o) };
  throw new PasskeyError('malformed', 'the door answered a reauthentication with neither a proof nor a step');
}

/**
 * Prove the first factor again for a change to a factor, with a password. The
 * session's own token is sent, so the proof is for this person and nobody else.
 * When the person holds a factor the answer is a second step, and answering it
 * with `completeSecondStep` yields the proof. **It never yields a session.**
 */
export async function reauthenticateWithPassword(
  endpoint: PasskeyEndpoint,
  token: string,
  credentials: PasswordCredentials,
): Promise<ReauthResult> {
  const begun = await call(endpoint, '/auth/begin', {
    method: 'password',
    identifier: credentials.identifier,
  });
  const o = await call(
    endpoint,
    '/auth/complete',
    {
      method: 'password',
      kind: 'password',
      reauth: true,
      challenge: text(begun, 'challenge'),
      identifier: credentials.identifier,
      password: credentials.password,
    },
    token,
  );
  return proofOrStep(o);
}

/** The same with a passkey. A session with fresh presence needs no proof at all. */
export async function reauthenticateWithPasskey(
  endpoint: PasskeyEndpoint,
  token: string,
): Promise<ReauthResult> {
  const begun = await call(endpoint, '/auth/begin', { method: 'passkey' });
  const { challenge, fields } = await assertion(endpoint, begun);
  const o = await call(
    endpoint,
    '/auth/complete',
    { method: 'passkey', kind: 'passkey_assertion', reauth: true, challenge, ...fields },
    token,
  );
  return proofOrStep(o);
}

// ── the person's factors ─────────────────────────────────────────────────

/** One of the person's second factors. Never a secret. */
export interface FactorRow {
  readonly id: string;
  /** `totp` or `security-key`. */
  readonly kind: string;
  /** What the person called it. */
  readonly label: string;
  readonly enrolledAt: number;
  readonly lastUsedAt: number;
  /** The door locked it after too many wrong answers. */
  readonly locked: boolean;
}

export interface FactorList {
  readonly factors: readonly FactorRow[];
  /** How many unused backup codes the person holds, when the deployment has them. */
  readonly backupCodesRemaining?: number | undefined;
  /** The person's own level: `when-enrolled` or `also-for-passkeys`. */
  readonly requireSecondStep: string;
  /** The app developer's setting: `off`, `enabled`, `required`, `required-for-non-passkey`. */
  readonly secondStep: string;
}

/** The person's own factors. */
export async function listFactors(endpoint: PasskeyEndpoint, token: string): Promise<FactorList> {
  const o = await call(endpoint, '/auth/factors', null, token, 'GET');
  const rows = Array.isArray(o['factors']) ? (o['factors'] as Wire[]) : [];
  const remaining = o['backup_codes_remaining'];
  const num = (r: Wire, k: string): number => (typeof r[k] === 'number' ? (r[k] as number) : 0);
  return {
    factors: rows.map((r) => ({
      id: text(r, 'id'),
      kind: text(r, 'kind'),
      label: typeof r['label'] === 'string' ? r['label'] : '',
      enrolledAt: num(r, 'enrolled_at'),
      lastUsedAt: num(r, 'last_used_at'),
      locked: r['locked'] === true,
    })),
    backupCodesRemaining: typeof remaining === 'number' ? remaining : undefined,
    requireSecondStep: text(o, 'require_second_step'),
    secondStep: text(o, 'second_step'),
  };
}

/** The proof a change needs when the session has no fresh presence. */
export interface ChangeProof {
  /** From `reauthenticateWith*`. Spent by the one act it is sent with. */
  readonly reauth?: string | undefined;
}

function proofHeaders(proof: ChangeProof | undefined): Record<string, string> | undefined {
  return proof?.reauth ? { 'x-reauth': proof.reauth } : undefined;
}

/**
 * Remove one of the person's factors. **Every other session ends**, and the
 * answer is a replacement session for this one, so the person who proved
 * themselves is not signed out for it. Use the returned token from then on.
 */
export async function removeFactor(
  endpoint: PasskeyEndpoint,
  token: string,
  factor: string,
  proof?: ChangeProof,
): Promise<{ readonly removed: string; readonly session: PasskeySession }> {
  const o = await call(
    endpoint,
    `/auth/factors/${encodeURIComponent(factor)}/remove`,
    null,
    token,
    'POST',
    proofHeaders(proof),
  );
  return { removed: text(o, 'removed'), session: session(o) };
}

/** Set the person's own level: ask for a second step after a passkey too, or not. */
export async function setSecondStepLevel(
  endpoint: PasskeyEndpoint,
  token: string,
  level: 'when-enrolled' | 'also-for-passkeys',
  proof?: ChangeProof,
): Promise<string> {
  const o = await call(
    endpoint,
    '/auth/factors/policy',
    { require_second_step: level },
    token,
    'POST',
    proofHeaders(proof),
  );
  return text(o, 'require_second_step');
}

// ── enrolling ────────────────────────────────────────────────────────────

/**
 * Who is enrolling a factor: a person signed in (`token`, and `reauth` to start
 * one), or a person a level made enrol, who has no token yet and is identified by
 * the `SecondStep` their sign-in answered.
 */
export type Enroller =
  | { readonly token: string; readonly reauth?: string | undefined }
  | { readonly step: SecondStep };

async function enrolCall(
  endpoint: PasskeyEndpoint,
  who: Enroller,
  path: string,
  body: Wire,
): Promise<Wire> {
  if ('step' in who) {
    return call(endpoint, path, {
      ...body,
      second_step: who.step.handle,
      binder: who.step.binder,
    });
  }
  return call(endpoint, path, body, who.token, 'POST', proofHeaders({ reauth: who.reauth }));
}

/** A secret for the person's authenticator app, shown once. */
export interface TotpEnrolment {
  readonly factor: string;
  readonly label: string;
  /** `otpauth://totp/...`: what a QR code carries. */
  readonly uri: string;
  /** The same secret in base32, for typing. */
  readonly secret: string;
  readonly expiresAt: number;
}

/**
 * Start enrolling a code-generating factor. **Nothing is enrolled yet**: the
 * secret waits until a code from the person's app proves it holds it
 * (`confirmTotpEnrolment`). The secret is in this answer once.
 */
export async function beginTotpEnrolment(
  endpoint: PasskeyEndpoint,
  who: Enroller,
  options: { readonly label?: string; readonly account?: string } = {},
): Promise<TotpEnrolment> {
  const body: Wire = {};
  if (options.label) body['label'] = options.label;
  if (options.account) body['account'] = options.account;
  const o = await enrolCall(endpoint, who, '/auth/factors/totp/begin', body);
  const expires = o['expires_at'];
  return {
    factor: text(o, 'factor'),
    label: text(o, 'label'),
    uri: text(o, 'uri'),
    secret: text(o, 'secret'),
    expiresAt: typeof expires === 'number' ? expires : 0,
  };
}

/** Finish it with one code from the person's app. */
export async function confirmTotpEnrolment(
  endpoint: PasskeyEndpoint,
  who: Enroller,
  factor: string,
  code: string,
): Promise<{ readonly factor: string; readonly kind: string }> {
  const o = await enrolCall(endpoint, who, '/auth/factors/totp/confirm', {
    factor,
    code: code.trim(),
  });
  return { factor: text(o, 'factor'), kind: text(o, 'kind') };
}

/**
 * Register a security key as a second factor. Two calls: the door's creation
 * options (a credential that need not be discoverable, user verification
 * preferred, no attestation), the browser's ceremony, and the registration back.
 * The key is registered through the passkey module's encoding.
 */
export async function enrolSecurityKey(
  endpoint: PasskeyEndpoint,
  who: Enroller,
  options: { readonly label?: string } = {},
): Promise<{ readonly factor: string; readonly userVerified: boolean }> {
  const start: Wire = {};
  if (options.label) start['label'] = options.label;
  const o = await enrolCall(endpoint, who, '/auth/factors/key/begin', start);
  const { challenge, fields } = await attestation(endpoint, o);
  const body: Wire = { method: 'passkey', kind: 'passkey_registration', challenge, ...fields };
  if (options.label) body['label'] = options.label;
  const done = await enrolCall(endpoint, who, '/auth/factors/key/complete', body);
  return { factor: text(done, 'factor'), userVerified: done['user_verified'] === true };
}
