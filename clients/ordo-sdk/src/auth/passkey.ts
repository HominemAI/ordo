/**
 * The browser half of passkey sign-in: three functions that run a WebAuthn
 * ceremony against the door's `/auth/*` routes.
 *
 * | Function | Routes | Yields |
 * |---|---|---|
 * | `registerPasskey` | `POST /auth/register`, then `POST /auth/complete` | a session token with presence |
 * | `signInWithPasskey` | `POST /auth/begin`, then `POST /auth/complete` | a session token with presence |
 * | `stepUp` | `POST /auth/step-up/begin`, then `POST /auth/step-up` | the same session, re-minted with fresh presence |
 *
 * # The two rules that make it verify
 *
 * The server verifies a signature over `authenticatorData || SHA-256(clientDataJSON)`.
 * So **`clientDataJSON` is posted as the exact bytes the browser produced**:
 * it is base64url-encoded from the `ArrayBuffer` and never parsed and
 * re-serialised on the way, because re-serialising changes the bytes and the
 * signature no longer matches.
 *
 * And the **challenge is two things**. The string the server sent is what is
 * posted back (it names the ceremony); the bytes it decodes to are what the
 * authenticator signs over. Passing the string to `navigator.credentials` as
 * if it were the bytes would sign the wrong thing, and every ceremony would
 * fail with a signature that verifies against nothing.
 *
 * All byte strings on the wire are base64url **without padding**.
 *
 * # What this does not do
 *
 * It fakes no user gesture. The tests stub `navigator.credentials` and prove
 * the encoding; a person clicks through the first real one.
 */

import { retryAfterMs, type BusyRetryOptions } from '../core/busy.js';
import type { NetworkOptions } from '../core/network.js';
import { OrdoRefusal, failureDetail, type WireAnswer } from '../core/refusal.js';
import { authRequest, siteOf } from './request.js';

export { siteOf };

/** Where the door is, and how to reach it. */
export interface PasskeyEndpoint {
  /** Door base URL. `/auth/...` is appended. */
  readonly baseUrl: string;
  /** For tests and for runtimes without a global `fetch`. */
  readonly fetch?: typeof globalThis.fetch;
  /** How long each call waits for the door. Default {@link DEFAULT_CALL_TIMEOUT_MS}. */
  readonly callTimeoutMs?: number;
  /** How a call that got no answer is named (`OrdoNetworkError`). */
  readonly network?: NetworkOptions;
  /**
   * How a call is sent again when the door answers `server_busy` (a `503`
   * with `Retry-After`: nothing ran), as `OrdoClient` does for ops. Same
   * options and defaults as `OrdoClientOptions.busyRetry`; `retries: 0`
   * turns it off.
   */
  readonly busyRetry?: BusyRetryOptions;
  /** For tests. Defaults to `navigator.credentials`. */
  readonly credentials?: Pick<CredentialsContainer, 'create' | 'get'>;
}

/** A session, as `/auth/complete`, `/auth/refresh` and `/auth/step-up` answer it. */
export interface PasskeySession {
  readonly token: string;
  /** When this token stops working, seconds since the epoch. */
  readonly expiresAt: number;
  /** When `/auth/refresh` stops continuing the session. */
  readonly sessionExpiresAt: number;
  /**
   * Present when the sign-in named a tenant: what the deployment did about
   * the person's arrival there. `refused` says nothing about why.
   */
  readonly admission?: { readonly space: string; readonly outcome: string } | undefined;
  /**
   * What the door wants this person to know, by word: `single_authenticator`
   * (they hold one way in), `recovery_codes_low`. Information: nothing is
   * enforced on it, and what an application does with it is its own.
   */
  readonly notices?: readonly string[] | undefined;
  /**
   * When step-up becomes available, in seconds since the epoch, for a person
   * whose passkey a recovery added. Until then the session works for everything
   * except the operations that ask for presence.
   */
  readonly presenceAvailableAt?: number | undefined;
  /**
   * Recoveries of this person's own that they have not acknowledged. Carried at
   * every sign-in until they call `acknowledgeRecovery`.
   */
  readonly recoveries?: readonly RecoveryNotice[] | undefined;
  /**
   * A marker an email sign-in may hand back. Keep it where the page keeps
   * things (for example `localStorage`) and send it as `device` on the next
   * `beginEmailSignIn`. This client never stores it.
   */
  readonly device?: string | undefined;
  /** The door's whole answer, for a field this client does not name. */
  readonly raw: Readonly<Record<string, unknown>>;
}

/** A recovery of this person's own, as a sign-in carries it. */
export interface RecoveryNotice {
  readonly case: string;
  /** How it was started: `code`, `tenant`, `operator`, `host`. */
  readonly path: string;
  /** The class of authority that acted, never a person's name. */
  readonly authority: string;
  readonly evidence: string;
  /** `completed`, or `closed` once the contest window ended. */
  readonly state: string;
  /** When the passkeys it retired stop being able to contest it. */
  readonly contestUntil: number;
}

export interface RegisterOptions {
  /** A ticket that lets this person register a passkey for an identity an administrator created. */
  readonly ticket?: string;
  /** A name the authenticator shows. Shown by it and by nothing else. */
  readonly display?: string;
  /** A tenant the person is arriving at, so the sign-in can admit them there. */
  readonly space?: string;
  /** An invitation ticket for `space`. Not the passkey `ticket` above. */
  readonly invitation?: string;
  /**
   * The Cloudflare Turnstile token, for a deployment that turned the human
   * check on (`ORDO_AUTH_HUMAN_CHECK`). Sent with the first leg, `register`.
   */
  readonly human?: string;
}

export interface SignInOptions {
  readonly space?: string;
  /** An invitation ticket for `space`. */
  readonly invitation?: string;
  /**
   * The Cloudflare Turnstile token, for a deployment that turned the human
   * check on (`ORDO_AUTH_HUMAN_CHECK`). Sent with `begin` as `human`.
   */
  readonly human?: string;
}

export type PasskeyFailure =
  /** This browser has no `navigator.credentials`. */
  | 'unsupported'
  /** The person dismissed the prompt, or the ceremony timed out. */
  | 'cancelled'
  /** The door refused. It says one thing whatever the reason. */
  | 'refused'
  /** The door answered 503: it could not check right now. Try again. */
  | 'unavailable'
  /**
   * Not thrown by a call any more: a call that gets no answer throws an
   * `OrdoNetworkError` of kind `timeout`, `unreachable` or `blocked`.
   */
  | 'timeout'
  /** The door answered something this client cannot read. */
  | 'malformed'
  /**
   * The person is signed in and the change they asked for needs a fresh first
   * factor: `beginReauthentication` (`second-step.ts`), then send the handle.
   */
  | 'reauthentication_required'
  /**
   * Linking by claim is switched off on this deployment. `setting` names the
   * app developer's setting that switches it on.
   */
  | 'linking_off'
  /**
   * Guest sign-in is switched off on this deployment. `setting` names the app
   * developer's setting that switches it on (`ORDO_AUTH_GUEST`).
   */
  | 'guests_off'
  /** The sign-in being claimed already belongs to another account. */
  | 'identifier_taken'
  /**
   * Tenant-owned sign-in providers are switched off on this deployment.
   * `setting` names the app developer's setting that switches them on
   * (`ORDO_AUTH_TENANT_PROVIDERS`).
   */
  | 'tenant_providers_off'
  /**
   * The deployment sends no mail, so no email address can be confirmed.
   * `setting` names the deployment setting that turns mail on (`ORDO_MAIL_KIND`).
   */
  | 'email_unavailable'
  /** The share link has a password and none was given. Ask for it and try again. */
  | 'link_password_required'
  /** The password given is not the link's. Ask again. */
  | 'link_password_wrong'
  /**
   * Too many wrong passwords: the link is locked. `retryAfter` says in how
   * many seconds it opens again, and `setting` names the tenant's limits op.
   */
  | 'link_locked'
  /**
   * Too many wrong passwords in a row on this account or this link: this try
   * was not checked. `retryAfter` says in how many seconds to try again (it
   * doubles, up to a minute by default), and `setting` names the op that
   * changes the slowdown. Never a lock: the right password works after the
   * wait.
   */
  | 'too_many_attempts';

/** What a {@link PasskeyError} carries beside its failure. */
export interface PasskeyErrorDetail extends WireAnswer {
  setting?: string | undefined;
  value?: string | undefined;
  retryAfter?: number | undefined;
}

/**
 * A sign-in call that did not work. An {@link OrdoRefusal}: `code` and
 * `status` are the door's when it answered, `failure` is what to do.
 */
export class PasskeyError extends OrdoRefusal<string> {
  readonly failure: PasskeyFailure;
  /** For `linking_off` and `guests_off`: the setting the app developer changes. */
  readonly setting?: string;
  /** For `linking_off` and `guests_off`: the value that switches it on. */
  readonly value?: string;
  /** For `link_locked` and `too_many_attempts`: seconds to wait before trying again. */
  readonly retryAfter?: number;
  constructor(failure: PasskeyFailure, message: string, detail?: PasskeyErrorDetail) {
    super(
      failureDetail(failure, message, detail),
      detail?.retryAfter === undefined ? null : detail.retryAfter * 1000,
      { message },
    );
    this.name = 'PasskeyError';
    this.failure = failure;
    if (detail?.setting !== undefined) this.setting = detail.setting;
    if (detail?.value !== undefined) this.value = detail.value;
    if (detail?.retryAfter !== undefined) this.retryAfter = detail.retryAfter;
  }
}

// ── base64url, without padding ───────────────────────────────────────────

const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';

/** Bytes to base64url, no padding. */
export function toBase64Url(data: ArrayBuffer | ArrayBufferView): string {
  const bytes =
    data instanceof ArrayBuffer
      ? new Uint8Array(data)
      : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  let out = '';
  for (let i = 0; i < bytes.length; i += 3) {
    const a = bytes[i] as number;
    const b = bytes[i + 1];
    const c = bytes[i + 2];
    out += ALPHABET[a >> 2];
    out += ALPHABET[((a & 3) << 4) | ((b ?? 0) >> 4)];
    if (b !== undefined) out += ALPHABET[((b & 15) << 2) | ((c ?? 0) >> 6)];
    if (c !== undefined) out += ALPHABET[c & 63];
  }
  return out;
}

/** base64url (padding tolerated) to bytes. Throws on a character outside the alphabet. */
export function fromBase64Url(text: string): Uint8Array {
  const clean = text.replace(/=+$/, '');
  const out: number[] = [];
  let acc = 0;
  let bits = 0;
  for (const ch of clean) {
    const v = ALPHABET.indexOf(ch);
    if (v < 0) throw new PasskeyError('malformed', 'the door sent a byte string that is not base64url');
    acc = (acc << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push((acc >> bits) & 255);
    }
  }
  return Uint8Array.from(out);
}

// ── the door ─────────────────────────────────────────────────────────────

/** A JSON object as the door sends and takes it. */
export type Wire = Record<string, unknown>;

/**
 * One call to the door. Exported for the recovery ceremonies in this package
 * (`recovery.ts`), which are the same calls with more routes; it is not part of
 * the package's public surface.
 */
export async function call(
  endpoint: PasskeyEndpoint,
  path: string,
  body: Wire | null,
  token?: string,
  method: 'POST' | 'GET' = 'POST',
  extraHeaders?: Readonly<Record<string, string>>,
): Promise<Wire> {
  const headers: Record<string, string> = { Accept: 'application/json', ...extraHeaders };
  if (body) headers['Content-Type'] = 'application/json';
  if (token) headers['Authorization'] = `Bearer ${token}`;
  // `include`: a sign-in's answer sets the renewal cookie, and a page on
  // another origin of the same site keeps it only when the request asked for
  // credentials. The cookie is sent nowhere but `/auth/refresh`.
  const init: RequestInit = { method, headers, credentials: 'include' };
  if (body) init.body = JSON.stringify(body);
  // `server_busy` says nothing ran, so every sign-in route is safe to send
  // again; `authRequest` does, and throws `ServerBusyError` past the last.
  const response = await authRequest(endpoint, path, init);
  let json: unknown = null;
  try {
    json = await response.json();
  } catch {
    // A 204 or an empty body: nothing to read.
  }
  // Every refusal below carries the door's status and code, as an
  // `OrdoRefusal` does.
  const refused = (failure: PasskeyFailure, message: string, detail: PasskeyErrorDetail = {}): PasskeyError =>
    new PasskeyError(failure, message, { ...detail, status: response.status, code: errorCode(json) });
  if (response.status === 503) {
    // A store that could not answer. Busy never reaches here.
    throw refused('unavailable', 'the door cannot check credentials right now');
  }
  if (response.status === 403 && errorCode(json) === 'reauthentication_required') {
    // The one refusal the door explains, because the person is signed in and is
    // owed the reason: prove a first factor again.
    throw refused(
      'reauthentication_required',
      'this change needs a fresh first factor: prove one again and send the handle',
    );
  }
  if (response.status === 403 && errorCode(json) === 'linking_off') {
    // Linking by claim is the app developer's choice, and the signed-in person
    // is owed the name of the setting.
    const e = (json as Wire)['error'] as Wire;
    const detail: { setting?: string; value?: string } = {};
    if (typeof e['setting'] === 'string') detail.setting = e['setting'];
    if (typeof e['value'] === 'string') detail.value = e['value'];
    throw refused('linking_off', 'linking sign-ins is switched off on this deployment', detail);
  }
  if (response.status === 403 && errorCode(json) === 'guests_off') {
    // Guest sign-in is the app developer's choice, and the page is owed the
    // name of the setting.
    const e = (json as Wire)['error'] as Wire;
    const detail: { setting?: string; value?: string } = {};
    if (typeof e['setting'] === 'string') detail.setting = e['setting'];
    if (typeof e['value'] === 'string') detail.value = e['value'];
    throw refused('guests_off', 'guest sign-in is switched off on this deployment', detail);
  }
  if (response.status === 400 && errorCode(json) === 'tenant_providers_off') {
    // Tenant providers are the app developer's choice, and the page is owed
    // the name of the setting.
    const e = (json as Wire)['error'] as Wire;
    const detail: { setting?: string; value?: string } = {};
    if (typeof e['setting'] === 'string') detail.setting = e['setting'];
    if (typeof e['value'] === 'string') detail.value = e['value'];
    throw refused(
      'tenant_providers_off',
      'tenant sign-in providers are switched off on this deployment',
      detail,
    );
  }
  if (response.status === 403 && errorCode(json) === 'email_unavailable') {
    const e = (json as Wire)['error'] as Wire;
    const detail: { setting?: string } = {};
    if (typeof e['setting'] === 'string') detail.setting = e['setting'];
    throw refused('email_unavailable', 'this deployment sends no mail, so no address can be confirmed', detail);
  }
  if (response.status === 401 && errorCode(json) === 'link_password_required') {
    // Only a live link answers this, to the holder of its secret.
    throw refused('link_password_required', 'this link needs its password');
  }
  if (response.status === 401 && errorCode(json) === 'link_password_wrong') {
    throw refused('link_password_wrong', "that is not the link's password");
  }
  if (response.status === 429 && errorCode(json) === 'link_locked') {
    const e = (json as Wire)['error'] as Wire;
    const detail: { setting?: string; retryAfter?: number } = {};
    if (typeof e['setting'] === 'string') detail.setting = e['setting'];
    if (typeof e['retry_after'] === 'number') detail.retryAfter = e['retry_after'];
    throw refused('link_locked', 'too many wrong passwords: this link is locked for a while', detail);
  }
  if (response.status === 429 && errorCode(json) === 'too_many_attempts') {
    const e = (json as Wire)['error'] as Wire;
    const detail: { setting?: string; retryAfter?: number } = {};
    if (typeof e['setting'] === 'string') detail.setting = e['setting'];
    if (typeof e['retry_after'] === 'number') {
      detail.retryAfter = e['retry_after'];
    } else {
      const ms = retryAfterMs(response);
      if (ms !== null) detail.retryAfter = Math.ceil(ms / 1000);
    }
    throw refused('too_many_attempts', 'too many wrong tries in a row: wait and try again', detail);
  }
  if (response.status === 409 && errorCode(json) === 'identifier_taken') {
    throw refused('identifier_taken', 'that sign-in already belongs to another account');
  }
  if (!response.ok) {
    // One message, because the door gives one answer.
    throw refused('refused', 'that did not authenticate');
  }
  // `204`: the door did it and has nothing to say (a sign-out, an
  // acknowledgement).
  if (response.status === 204) return {};
  if (json === null || typeof json !== 'object') {
    throw new PasskeyError('malformed', 'the door answered with something that is not JSON');
  }
  return json as Wire;
}

function errorCode(json: unknown): string | undefined {
  if (json === null || typeof json !== 'object') return undefined;
  const e = (json as Wire)['error'];
  if (e === null || typeof e !== 'object') return undefined;
  const c = (e as Wire)['code'];
  return typeof c === 'string' ? c : undefined;
}

export function text(o: Wire, key: string): string {
  const v = o[key];
  if (typeof v !== 'string') throw new PasskeyError('malformed', `the door's answer has no \`${key}\``);
  return v;
}

export function list(o: Wire, key: string): string[] {
  const v = o[key];
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
}

export function containerOf(endpoint: PasskeyEndpoint): Pick<CredentialsContainer, 'create' | 'get'> {
  const c = endpoint.credentials ?? (typeof navigator !== 'undefined' ? navigator.credentials : undefined);
  if (!c) throw new PasskeyError('unsupported', 'this browser cannot run a passkey ceremony');
  return c;
}

/** The person said no, or the prompt timed out. Anything else is a bug and is rethrown. */
export function cancelled(e: unknown): never {
  if (e instanceof PasskeyError) throw e;
  const name = (e as { name?: string } | null)?.name;
  if (name === 'NotAllowedError' || name === 'AbortError') {
    throw new PasskeyError('cancelled', 'the passkey prompt was dismissed or timed out');
  }
  throw e;
}

/** A session as the door answers one. Exported for `second-step.ts`. */
export function session(o: Wire): PasskeySession {
  const num = (k: string): number => {
    const v = o[k];
    if (typeof v !== 'number') throw new PasskeyError('malformed', `the door's answer has no \`${k}\``);
    return v;
  };
  const admission = o['admission'];
  const parsed =
    admission && typeof admission === 'object'
      ? {
          space: String((admission as Wire)['space'] ?? ''),
          outcome: String((admission as Wire)['outcome'] ?? ''),
        }
      : undefined;
  const notices = o['notices'];
  const held = o['presence_available_at'];
  const device = o['device'];
  const recoveries = Array.isArray(o['recoveries'])
    ? (o['recoveries'] as Wire[]).map((r) => ({
        case: String(r['case'] ?? ''),
        path: String(r['path'] ?? ''),
        authority: String(r['authority'] ?? ''),
        evidence: String(r['evidence'] ?? ''),
        state: String(r['state'] ?? ''),
        contestUntil: typeof r['contest_until'] === 'number' ? r['contest_until'] : 0,
      }))
    : undefined;
  return {
    token: text(o, 'token'),
    expiresAt: num('expires_at'),
    sessionExpiresAt: num('session_expires_at'),
    admission: parsed,
    notices: Array.isArray(notices) ? notices.filter((n): n is string => typeof n === 'string') : undefined,
    presenceAvailableAt: typeof held === 'number' ? held : undefined,
    recoveries,
    device: typeof device === 'string' ? device : undefined,
    raw: o,
  };
}

function timeout(o: Wire): number | undefined {
  const t = o['timeout_ms'];
  return typeof t === 'number' ? t : undefined;
}

export function toBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

function requestOptions(o: Wire): PublicKeyCredentialRequestOptions {
  const t = timeout(o);
  const options: PublicKeyCredentialRequestOptions = {
    // The bytes the server minted, **decoded**: this is what is signed.
    challenge: toBuffer(fromBase64Url(text(o, 'challenge'))),
    rpId: text(o, 'rp_id'),
    userVerification: text(o, 'user_verification') as UserVerificationRequirement,
    allowCredentials: list(o, 'allow_credentials').map((id) => ({
      type: 'public-key' as const,
      id: toBuffer(fromBase64Url(id)),
    })),
  };
  if (t !== undefined) options.timeout = t;
  return options;
}

export async function assertion(
  endpoint: PasskeyEndpoint,
  options: Wire,
): Promise<{ challenge: string; fields: Wire }> {
  let credential: Credential | null;
  try {
    credential = await containerOf(endpoint).get({ publicKey: requestOptions(options) });
  } catch (e) {
    cancelled(e);
  }
  const pk = credential as PublicKeyCredential | null;
  if (!pk) throw new PasskeyError('cancelled', 'no passkey was chosen');
  const r = pk.response as AuthenticatorAssertionResponse;
  return {
    // The string the server sent, not its bytes: it names the ceremony.
    challenge: text(options, 'challenge'),
    fields: {
      credential_id: toBase64Url(pk.rawId),
      client_data_json: toBase64Url(r.clientDataJSON),
      authenticator_data: toBase64Url(r.authenticatorData),
      signature: toBase64Url(r.signature),
    },
  };
}

// ── the three ceremonies ─────────────────────────────────────────────────

/**
 * Run `navigator.credentials.create` for the creation options the door sent,
 * and answer the fields the door wants back: the challenge string it sent, and
 * the two byte strings the browser produced. Shared by sign-up, adding a
 * passkey, and recovery.
 */
export async function attestation(
  endpoint: PasskeyEndpoint,
  o: Wire,
): Promise<{ challenge: string; fields: Wire }> {
  const user = (o['user'] ?? {}) as Wire;
  const t = timeout(o);
  const publicKey: PublicKeyCredentialCreationOptions = {
    challenge: toBuffer(fromBase64Url(text(o, 'challenge'))),
    rp: { id: text(o, 'rp_id'), name: text(o, 'rp_name') },
    user: {
      id: toBuffer(fromBase64Url(text(user, 'id'))),
      name: text(user, 'name'),
      displayName: text(user, 'display_name'),
    },
    pubKeyCredParams: ((o['algorithms'] as unknown[]) ?? [])
      .filter((a): a is number => typeof a === 'number')
      .map((alg) => ({ type: 'public-key' as const, alg })),
    authenticatorSelection: {
      residentKey: text(o, 'resident_key') as ResidentKeyRequirement,
      userVerification: text(o, 'user_verification') as UserVerificationRequirement,
    },
    attestation: text(o, 'attestation') as AttestationConveyancePreference,
    excludeCredentials: list(o, 'exclude_credentials').map((id) => ({
      type: 'public-key' as const,
      id: toBuffer(fromBase64Url(id)),
    })),
  };
  if (t !== undefined) publicKey.timeout = t;

  let credential: Credential | null;
  try {
    credential = await containerOf(endpoint).create({ publicKey });
  } catch (e) {
    cancelled(e);
  }
  const pk = credential as PublicKeyCredential | null;
  if (!pk) throw new PasskeyError('cancelled', 'no passkey was created');
  const r = pk.response as AuthenticatorAttestationResponse;
  return {
    challenge: text(o, 'challenge'),
    fields: {
      client_data_json: toBase64Url(r.clientDataJSON),
      attestation_object: toBase64Url(r.attestationObject),
    },
  };
}


/** Sign in with a passkey the person already has. */
export async function signInWithPasskey(
  endpoint: PasskeyEndpoint,
  options: SignInOptions = {},
): Promise<PasskeySession> {
  const start: Wire = { method: 'passkey' };
  if (options.human) start['human'] = options.human;
  const begun = await call(endpoint, '/auth/begin', start);
  const { challenge, fields } = await assertion(endpoint, begun);
  const body: Wire = { method: 'passkey', kind: 'passkey_assertion', challenge, ...fields };
  if (options.space) body['space'] = options.space;
  if (options.invitation) body['invitation'] = options.invitation;
  return session(await call(endpoint, '/auth/complete', body));
}

/**
 * Register a passkey. Under an open deployment this is signing up; with a
 * ticket it is registering for an identity somebody created. Either way the
 * answer is a session.
 */
export async function registerPasskey(
  endpoint: PasskeyEndpoint,
  options: RegisterOptions = {},
): Promise<PasskeySession> {
  const start: Wire = { method: 'passkey' };
  if (options.display) start['display'] = options.display;
  if (options.ticket) start['ticket'] = options.ticket;
  if (options.human) start['human'] = options.human;
  const o = await call(endpoint, '/auth/register', start);

  const { challenge, fields } = await attestation(endpoint, o);
  const body: Wire = {
    method: 'passkey',
    kind: 'passkey_registration',
    challenge,
    ...fields,
  };
  if (options.display) body['display'] = options.display;
  if (options.ticket) body['ticket'] = options.ticket;
  if (options.space) body['space'] = options.space;
  if (options.invitation) body['invitation'] = options.invitation;
  return session(await call(endpoint, '/auth/complete', body));
}

/**
 * Prove presence again inside a session, so a step-up operation can run for
 * the next few minutes. Answers the **same session**, re-minted.
 */
export async function stepUp(endpoint: PasskeyEndpoint, token: string): Promise<PasskeySession> {
  const begun = await call(endpoint, '/auth/step-up/begin', null, token);
  const { challenge, fields } = await assertion(endpoint, begun);
  return session(
    await call(
      endpoint,
      '/auth/step-up',
      { method: 'passkey', kind: 'passkey_assertion', challenge, ...fields },
      token,
    ),
  );
}
