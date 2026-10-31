/**
 * One session object: the front door for signing in and staying signed in.
 *
 * {@link createOrdoAuth} holds the person's session and wraps the ceremonies
 * this package already has (email code, passkey, resume, step-up, tenants).
 * It adds the 2 calls a page needed raw `fetch` for: renewing the token
 * (`POST /auth/refresh`) and signing out (`POST /auth/logout`). Both are
 * also plain functions here, {@link refreshSession} and {@link signOut}.
 *
 *     const auth = createOrdoAuth({ baseUrl: DOOR });
 *     await auth.resume();                         // null: show sign-in
 *     const client = new OrdoClient({ baseUrl: DOOR, space, token: auth.token });
 *
 * `auth.token` is a function: every call gets a live token, renewed before
 * it expires. A timer also renews it ahead of expiry, so an idle page wakes
 * to a working token.
 */

import type { OrdoClientOptions } from '../core/client.js';
import { beginEmailSignIn, completeEmailSignIn, type EmailCodeSent, type EmailComplete, type EmailSignInStart } from './email.js';
import {
  PasskeyError,
  call,
  registerPasskey,
  session as sessionOf,
  signInWithPasskey,
  stepUp,
  type PasskeyEndpoint,
  type PasskeySession,
  type RegisterOptions,
  type SignInOptions,
} from './passkey.js';
import { waitForTenant, type WaitForTenantOptions } from './provisioning.js';
import { resumeSession, type ResumeOptions } from './resume.js';
import type { SignInResult } from './second-step.js';
import { listSpaces, type ListedSpace } from './share.js';

/**
 * Continue a session with its own token: `POST /auth/refresh`. Answers the
 * same session with a new token. Throws a `PasskeyError` of failure
 * `refused` when the session ended (signed out, past its limit, or the token
 * already expired), and `unavailable` when the door cannot check right now.
 */
export async function refreshSession(endpoint: PasskeyEndpoint, token: string): Promise<PasskeySession> {
  return sessionOf(await call(endpoint, '/auth/refresh', null, token));
}

/**
 * End a session: `POST /auth/logout`. The door also clears the renewal
 * cookie, so a reload does not resume it. A token that was already ended is
 * the same success. Throws a `PasskeyError` of failure `unavailable` when
 * the door could not record it: the session may still work, so try again.
 */
export async function signOut(endpoint: PasskeyEndpoint, token: string): Promise<void> {
  await call(endpoint, '/auth/logout', null, token);
}

/** Seconds before expiry that a token is renewed. */
export const DEFAULT_RENEW_BEFORE_S = 60;

export interface OrdoAuthOptions extends PasskeyEndpoint {
  /** Renew this many seconds before the token expires. Default {@link DEFAULT_RENEW_BEFORE_S}. */
  readonly renewBeforeS?: number;
  /** Renew on a timer ahead of expiry. Default `true`. Off, `token()` still renews when called. */
  readonly autoRenew?: boolean;
  /** For tests: the clock, in milliseconds. */
  readonly now?: () => number;
}

/** Told the session after every change: a sign-in, a renewal, a sign-out (`null`). */
export type SessionListener = (session: PasskeySession | null) => void;

export interface OrdoAuth {
  /** The door, for the ceremonies this object does not wrap. */
  readonly door: PasskeyEndpoint;
  /** The session now, or `null` when nobody is signed in. */
  readonly session: () => PasskeySession | null;
  /**
   * A live token. Pass it as `token` to `OrdoClient`, `DescriptorClient`,
   * `TenantGate`. Renews when the token is near expiry. Throws a
   * `PasskeyError` of failure `refused` when nobody is signed in.
   */
  readonly token: () => Promise<string>;
  /**
   * Renew now. Answers `true` with a new token, `false` when the session
   * ended (the person is then signed out). Pass it as `renewToken` to
   * `OrdoClient` and `DescriptorClient`. Throws when the door cannot be reached.
   */
  readonly renew: () => Promise<boolean>;
  /** Start signed in when the browser still holds a sign-in. `null`: show sign-in. */
  readonly resume: (options?: ResumeOptions) => Promise<PasskeySession | null>;
  /** Mail a code. Keep the answer in memory only. */
  readonly beginEmail: (start: EmailSignInStart) => Promise<EmailCodeSent>;
  /**
   * Send the code back. `display` names a new person. A session is kept;
   * a `second-step` answer is the page's to finish, then `use` its session.
   */
  readonly completeEmail: (sent: EmailCodeSent, code: string, options?: EmailComplete) => Promise<SignInResult>;
  /** Sign in with a passkey the person has. */
  readonly signInWithPasskey: (options?: SignInOptions) => Promise<PasskeySession>;
  /** Sign up with a new passkey. `display` names the person. */
  readonly registerPasskey: (options?: RegisterOptions) => Promise<PasskeySession>;
  /** Prove presence again; the re-minted session is kept. */
  readonly stepUp: () => Promise<PasskeySession>;
  /** Keep a session another ceremony made (a second step, a handoff, a guest). */
  readonly use: (session: PasskeySession | null) => void;
  /** Sign out at the door and here. Throws `unavailable` when the door could not record it. */
  readonly signOut: () => Promise<void>;
  /** The person's tenants and roles: `[{ space, name, role, via }]`. */
  readonly spaces: () => Promise<readonly ListedSpace[]>;
  /** Wait for a new person's own tenant (`ORDO_PROVISION_SELF_SERVE=on-signup`). */
  readonly waitForTenant: (options?: WaitForTenantOptions) => Promise<string>;
  /** Options for `OrdoClient` and `DescriptorClient`: `{ baseUrl, token, renewToken }`. */
  readonly clientOptions: () => Pick<OrdoClientOptions, 'baseUrl' | 'token' | 'renewToken'>;
  /** Listen for changes. Answers the function that stops listening. */
  readonly subscribe: (listener: SessionListener) => () => void;
}

/** Make the session object for one door. See the module note. */
export function createOrdoAuth(options: OrdoAuthOptions): OrdoAuth {
  const door: PasskeyEndpoint = options;
  const before = options.renewBeforeS ?? DEFAULT_RENEW_BEFORE_S;
  const auto = options.autoRenew ?? true;
  const nowS = () => (options.now ?? Date.now)() / 1000;
  const listeners = new Set<SessionListener>();
  let current: PasskeySession | null = null;
  let renewing: Promise<boolean> | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;

  function schedule(): void {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
    if (!auto || current === null) return;
    const ms = Math.max(0, (current.expiresAt - before - nowS()) * 1000);
    timer = setTimeout(() => {
      timer = undefined;
      // A failed renewal is tried again by the next `token()`.
      renew().catch(() => undefined);
    }, ms);
    // A Node script must not stay alive for the timer.
    (timer as { unref?: () => void }).unref?.();
  }

  function use(next: PasskeySession | null): void {
    current = next;
    schedule();
    for (const l of listeners) l(next);
  }

  async function renewOnce(): Promise<boolean> {
    const held = current;
    if (held === null) return false;
    if (held.expiresAt > nowS()) {
      try {
        const next = await refreshSession(door, held.token);
        // A renewal answers a token and its times; the rest of the sign-in stays.
        const { token, expiresAt, sessionExpiresAt } = next;
        if (current === held) use({ ...held, token, expiresAt, sessionExpiresAt, raw: { ...held.raw, ...next.raw } });
        return true;
      } catch (e) {
        if (!(e instanceof PasskeyError) || e.failure !== 'refused') throw e;
      }
    }
    // The token expired, or the door refused it: the renewal cookie may
    // still continue the sign-in.
    const resumed = await resumeSession(door);
    if (current !== held) return current !== null;
    use(resumed);
    return resumed !== null;
  }

  function renew(): Promise<boolean> {
    renewing ??= renewOnce().finally(() => (renewing = null));
    return renewing;
  }

  async function token(): Promise<string> {
    if (current !== null && current.expiresAt - nowS() > before) return current.token;
    if (current === null || !(await renew()) || current === null) {
      throw new PasskeyError('refused', 'nobody is signed in');
    }
    return (current as PasskeySession).token;
  }

  const keep = <S extends PasskeySession | null>(s: S): S => {
    use(s);
    return s;
  };

  return {
    door,
    session: () => current,
    token,
    renew,
    resume: async (o) => keep(await resumeSession(door, o)),
    beginEmail: (start) => beginEmailSignIn(door, start),
    async completeEmail(sent, code, o) {
      const r = await completeEmailSignIn(door, sent, code, o);
      if (r.kind === 'session') use(r.session);
      return r;
    },
    signInWithPasskey: async (o) => keep(await signInWithPasskey(door, o)),
    registerPasskey: async (o) => keep(await registerPasskey(door, o)),
    stepUp: async () => keep(await stepUp(door, await token())),
    use,
    async signOut() {
      const held = current;
      if (held === null) return;
      try {
        await signOut(door, held.token);
      } catch (e) {
        // Not recorded: the session may still work, so it is kept.
        if (e instanceof PasskeyError && e.failure === 'unavailable') throw e;
        // Any other answer: the door will not honour the token either way.
      }
      if (current === held) use(null);
    },
    spaces: async () => listSpaces(door, await token()),
    waitForTenant: (o) => waitForTenant(door, token, o),
    clientOptions: () => ({ baseUrl: door.baseUrl, token, renewToken: renew }),
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
