/**
 * The signed-in person: a token held in memory, kept fresh, and the client
 * bound to their own tenant.
 *
 * Every way in ends here: a passkey, an emailed code, an outside connection,
 * or a guest start. The token is never written to `localStorage` or anywhere
 * else the page can be read back from after a reload. A reload resumes the
 * sign-in through the renewal cookie the door set (`Session.resume`), which
 * scripts cannot read. Only when there is nothing to resume does the page ask
 * the person to sign in again.
 */

import {
  PasskeyError,
  registerPasskey,
  signInWithPasskey,
  stepUp,
  type PasskeyEndpoint,
  type PasskeySession,
} from '@hominemai/ordo-sdk/auth/passkey.js';
import { beginEmailSignIn, completeEmailSignIn, type EmailCodeSent } from '@hominemai/ordo-sdk/auth/email.js';
import { guestCeremony, resumeGuest, startGuest } from '@hominemai/ordo-sdk/auth/guest.js';
import {
  beginClaim,
  claimPasskey,
  emailCodeCeremony,
  handoffCeremony,
  type ClaimCeremony,
} from '@hominemai/ordo-sdk/auth/claim.js';
import { listAuthenticators } from '@hominemai/ordo-sdk/auth/recovery.js';
import { resumeSession } from '@hominemai/ordo-sdk/auth/resume.js';
import { TenantError, waitForTenant, type TenantStatus } from '@hominemai/ordo-sdk/auth/provisioning.js';
import type { SignInResult } from '@hominemai/ordo-sdk/auth/second-step.js';
import { DescriptorClient as HooksClient } from '@hominemai/ordo-react';
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { namedFetch } from '@hominemai/ordo-sdk/core/network.js';
import { OrdoRefusal } from '@hominemai/ordo-sdk/core/refusal.js';
import { myNotes } from './generated/ops.js';
import { INTERFACE } from './interface.js';
import {
  beginOutside,
  completeOutside,
  deviceMarker,
  guestSecret,
  keepDeviceMarker,
  keepGuestSecret,
  type Carried,
} from './signin.js';
import { principalOf } from './token.js';

export interface SessionOptions {
  readonly baseUrl: string;
  readonly fetch?: typeof globalThis.fetch;
  readonly credentials?: PasskeyEndpoint['credentials'];
  /** How long each sign-in call waits for the door. The SDK default when unset. */
  readonly callTimeoutMs?: number;
  /** How a call that got no answer is named. The SDK defaults when unset; tests set the page origin. */
  readonly network?: PasskeyEndpoint['network'];
  /** Seconds since the epoch. Replaced in tests. */
  readonly now?: () => number;
  /**
   * Told each step while a new person's tenant is being set up, after the
   * sign-in answered and before the session exists. The page shows it.
   */
  readonly onSettingUp?: (status: TenantStatus) => void;
  /** For tests: how `waitForTenant` pauses between reads. */
  readonly settingUpPause?: (ms: number) => Promise<void>;
}

/** The sign-in worked and the deployment gave this person no tenant. */
export class NoTenantError extends Error {
  readonly reason: string;
  /** The person's account id, which a deployment owner needs to name them. */
  readonly principal: string | null;
  /** `said` is the door's own sentence for the person, when it gave one. */
  constructor(reason: string, principal: string | null = null, said?: string) {
    const who = principal === null ? '' : ` Your account id is ${principal}.`;
    super(
      said !== undefined
        ? `${said.charAt(0).toUpperCase()}${said.slice(1)}.${who}`
        : reason === 'in progress'
          ? `Your notes are still being set up. Try again in a moment.${who}`
          : `Your notes could not be set up (${reason}). Try again, and tell the deployment owner if it keeps happening.${who}`,
    );
    this.name = 'NoTenantError';
    this.reason = reason;
    this.principal = principal;
  }
}

/** The session ended: logged out, expired for good, or refused by the door. */
export class SessionEnded extends Error {
  constructor() {
    super('You are signed out. Sign in again to continue.');
    this.name = 'SessionEnded';
  }
}

/** The deployment asks for a second step (an app code or a security key), which this page does not offer. */
export class SecondStepNeeded extends Error {
  constructor() {
    super(
      'This deployment asks for a second step after sign-in, and this page does not offer one. ' +
        'Sign in with a passkey, or ask the deployment owner.',
    );
    this.name = 'SecondStepNeeded';
  }
}

/** Renew this many seconds before the token ends. */
const RENEW_BEFORE_SECS = 60;

export class Session {
  /** The tenant of this person's own: where their notes live. */
  readonly space: string;
  /** The person, as the deployment names them. */
  readonly principal: string;
  readonly endpoint: PasskeyEndpoint;
  /**
   * How this session began, as the door says it: `passkey`, `email-otp`,
   * `guest`, `oauth2:discord`, `oidc:google`, and so on.
   */
  readonly via: string;
  /** The address an emailed-code sign-in used, so a claim can mail it again. */
  readonly address: string | null;

  readonly #now: () => number;
  readonly #fetch: typeof globalThis.fetch;
  #token: string;
  #expiresAt: number;
  #sessionExpiresAt: number;
  #refreshing: Promise<void> | null = null;
  #ended = false;
  /** The provider's proof, got on the way back from a claim's trip. Good once. */
  #outsideProof: ClaimCeremony | null = null;
  /** A token pasted in for development: there is no session at the door to renew or end. */
  #pasted = false;
  readonly #onEnd = new Set<() => void>();

  private constructor(options: SessionOptions, done: PasskeySession, space: string, address: string | null = null) {
    this.endpoint = {
      baseUrl: options.baseUrl,
      ...(options.fetch ? { fetch: options.fetch } : {}),
      ...(options.credentials ? { credentials: options.credentials } : {}),
      ...(options.callTimeoutMs !== undefined ? { callTimeoutMs: options.callTimeoutMs } : {}),
      ...(options.network ? { network: options.network } : {}),
    };
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.#now = options.now ?? (() => Math.floor(Date.now() / 1000));
    this.#token = done.token;
    this.#expiresAt = done.expiresAt;
    this.#sessionExpiresAt = done.sessionExpiresAt;
    this.space = space;
    this.principal = principalOf(done.token);
    const via = done.raw['via'];
    this.via = typeof via === 'string' ? via : 'passkey';
    this.address = address;
  }

  /** A form and a passkey: register, and land in a tenant of one's own. */
  static async signUp(options: SessionOptions, display: string, human?: string): Promise<Session> {
    const endpoint = endpointOf(options);
    return Session.#ready(options, await registerPasskey(endpoint, { display, ...(human ? { human } : {}) }));
  }

  /** The second half of an emailed-code sign-in: the code the person typed. */
  static async withEmail(options: SessionOptions, sent: EmailCodeSent, code: string, address: string): Promise<Session> {
    const done = Session.#session(await completeEmailSignIn(endpointOf(options), sent, code));
    keepDeviceMarker(done.device);
    return Session.#ready(options, done, address);
  }

  /**
   * Continue as a guest. With a guest secret on this device, that guest comes
   * back. Otherwise a new one starts and its secret is kept here.
   */
  static async asGuest(options: SessionOptions, human?: string): Promise<Session> {
    const endpoint = endpointOf(options);
    const secret = guestSecret();
    if (secret !== null) {
      return Session.#ready(options, Session.#session(await resumeGuest(endpoint, secret)));
    }
    const started = await startGuest(endpoint, human ? { human } : {});
    keepGuestSecret(started.secret);
    return Session.#ready(options, Session.#session(started.result));
  }

  /** The person came back from an outside connection with a hand-off code. */
  static async fromOutside(
    options: SessionOptions,
    back: { readonly method: string; readonly handoff: string; readonly binder: string },
  ): Promise<Session> {
    return Session.#ready(options, Session.#session(await completeOutside(endpointOf(options), back)));
  }

  /**
   * Back from the trip that began "Add a passkey" for an outside-connection
   * account. The trip kept the session (see `Carried`), and the code the
   * provider sent back is the claim's first proof, not a sign-in: signing in
   * with it would spend it. The person then presses the button once more,
   * because the browser wants a touch to come from a click.
   */
  static resumeForClaim(
    options: SessionOptions,
    carried: Carried,
    back: { readonly method: string; readonly handoff: string; readonly binder: string } | null,
  ): Session {
    const done: PasskeySession = {
      token: carried.token,
      expiresAt: carried.expiresAt,
      sessionExpiresAt: carried.sessionExpiresAt,
      raw: { space: carried.space, via: carried.via },
    };
    const session = Session.#from(options, done);
    if (back !== null) session.#outsideProof = handoffCeremony(back.method, back.handoff, back.binder);
    return session;
  }

  /**
   * A token `ordo dev` printed, for trying the page against the local loop,
   * which has no sign-in. It is not renewed and signing out only forgets it.
   */
  static fromToken(options: SessionOptions, token: string, space: string): Session {
    const done: PasskeySession = {
      token,
      expiresAt: Number.MAX_SAFE_INTEGER,
      sessionExpiresAt: Number.MAX_SAFE_INTEGER,
      raw: {},
    };
    const session = new Session(options, done, space);
    session.#pasted = true;
    return session;
  }

  /**
   * The page has just loaded: continue the sign-in this browser still holds,
   * or `null` when it holds none. Throws when the door cannot be asked, since
   * the person may still be signed in.
   */
  static async resume(options: SessionOptions): Promise<Session | null> {
    const done = await resumeSession(endpointOf(options));
    if (done === null) return null;
    const space = done.raw['space'];
    if (typeof space === 'string' && space !== '') return Session.#from(options, done);
    // A sign-in that named no tenant: ask the door which is this person's.
    return Session.#ready(options, { ...done, raw: { ...done.raw, provisioning: 'in progress' } });
  }

  /** The passkey button, for a person who has an account. */
  static async signIn(options: SessionOptions, human?: string): Promise<Session> {
    return Session.#ready(options, await signInWithPasskey(endpointOf(options), human ? { human } : {}));
  }

  /** A session, or the second step this page does not offer. */
  static #session(r: SignInResult): PasskeySession {
    if (r.kind === 'second-step') throw new SecondStepNeeded();
    return r.session;
  }

  /**
   * A sign-in's answer, once the person's tenant is ready. A new person's
   * tenant is made after the sign-in answers (`provisioning: "in progress"`),
   * so this waits for it, telling `onSettingUp` each step.
   */
  static async #ready(options: SessionOptions, done: PasskeySession, address: string | null = null): Promise<Session> {
    const space = done.raw['space'];
    if ((typeof space !== 'string' || space === '') && done.raw['provisioning'] === 'in progress') {
      try {
        const made = await waitForTenant(endpointOf(options), done.token, {
          ...(options.onSettingUp ? { onProgress: options.onSettingUp } : {}),
          ...(options.settingUpPause ? { wait: options.settingUpPause } : {}),
        });
        return new Session(options, done, made, address);
      } catch (e) {
        if (!(e instanceof TenantError)) throw e;
        let who: string | null = null;
        try {
          who = principalOf(done.token);
        } catch {
          // A token this page cannot read is one it cannot name.
        }
        throw new NoTenantError(e.failure, who, e.reason ?? e.message);
      }
    }
    return Session.#from(options, done, address);
  }

  /** A sign-in with a ticket-less answer: the tenant is `raw.space`. */
  static #from(options: SessionOptions, done: PasskeySession, address: string | null = null): Session {
    const space = done.raw['space'];
    if (typeof space !== 'string' || space === '') {
      const why = done.raw['provisioning'];
      let who: string | null = null;
      try {
        who = principalOf(done.token);
      } catch {
        // A token this page cannot read is one it cannot name.
      }
      throw new NoTenantError(typeof why === 'string' ? why : 'not provisioned', who);
    }
    return new Session(options, done, space, address);
  }

  get ended(): boolean {
    return this.#ended;
  }

  /** When the session cannot be renewed any more, in seconds since the epoch. */
  get sessionExpiresAt(): number {
    return this.#sessionExpiresAt;
  }

  /** The current token, renewed first when it is about to end. */
  async token(): Promise<string> {
    if (this.#ended) throw new SessionEnded();
    if (this.#expiresAt - this.#now() > RENEW_BEFORE_SECS) return this.#token;
    // One renewal for any number of calls that need it at the same moment.
    this.#refreshing ??= this.#renew().finally(() => {
      this.#refreshing = null;
    });
    await this.#refreshing;
    return this.#token;
  }

  /** A client bound to this person's own tenant. */
  client(): OrdoClient {
    return new OrdoClient({
      baseUrl: this.endpoint.baseUrl,
      space: this.space,
      token: () => this.token(),
      ...(this.endpoint.fetch ? { fetch: this.endpoint.fetch } : {}),
      ...(this.endpoint.network ? { network: this.endpoint.network } : {}),
    });
  }

  /** The same tenant and token for `@hominemai/ordo-react`'s hooks, renewed as the token is. */
  hooksClient(): HooksClient {
    return new HooksClient({
      baseUrl: this.endpoint.baseUrl,
      iface: INTERFACE,
      space: this.space,
      token: () => this.token(),
      fetch: namedFetchOf(this.endpoint),
    });
  }

  /** Whether the account holds a passkey that can sign in now. */
  async hasPasskey(): Promise<boolean> {
    if (this.#pasted) return true;
    const held = await listAuthenticators(this.endpoint, await this.token());
    return held.authenticators.some((a) => a.kind === 'passkey' && a.state === 'active');
  }

  /**
   * How this session can prove, a second time, a sign-in the account holds:
   * the guest secret on this device, or a new code to the address it used.
   * Null when this page has no such proof. An outside connection would need
   * a second trip to the provider, and the token does not survive leaving.
   */
  get claimProof(): 'guest' | 'email' | 'outside' | null {
    if (this.via === 'guest' && guestSecret() !== null) return 'guest';
    if (this.via === 'email-otp' && this.address !== null) return 'email';
    if (this.outsideConnection !== null) return 'outside';
    return null;
  }

  /** The connection this session came in by (`discord`), or null. */
  get outsideConnection(): string | null {
    const m = /^(?:oauth2|oidc):([a-z0-9][a-z0-9_-]*)$/.exec(this.via);
    return m?.[1] ?? null;
  }

  /** The provider has proved this person again and the claim can open. */
  get outsideProved(): boolean {
    return this.#outsideProof !== null;
  }

  /**
   * The first of the two trips: the address to send the browser to so the
   * provider can prove this person again. The session is kept for the trip
   * and picked up by `resumeForClaim` on return.
   */
  async beginOutsideClaim(returnTo: string, human?: string, hash = ''): Promise<string> {
    const connection = this.outsideConnection;
    if (connection === null) throw new Error('This session did not sign in through an outside connection.');
    const token = await this.token();
    return beginOutside(this.endpoint, connection, returnTo, human, hash, undefined, {
      token,
      expiresAt: this.#expiresAt,
      sessionExpiresAt: this.#sessionExpiresAt,
      space: this.space,
      via: this.via,
    });
  }

  /** Mail a fresh code to the address this session used, to prove it for a claim. */
  async sendClaimCode(human?: string): Promise<EmailCodeSent> {
    if (this.address === null) throw new Error('This session did not sign in with an email address.');
    const device = deviceMarker();
    return beginEmailSignIn(this.endpoint, {
      address: this.address,
      ...(human ? { human } : {}),
      ...(device ? { device } : {}),
    });
  }

  /**
   * Add a passkey to this account: linking by claim. `sent` and `code` are
   * the emailed code when the proof is `email`. The browser then asks for a
   * new passkey. A guest's secret is forgotten once the passkey is linked:
   * the passkey is the way back in from then on.
   */
  async addPasskey(sent?: EmailCodeSent, code?: string): Promise<void> {
    let held: ClaimCeremony;
    const proof = this.claimProof;
    if (proof === 'guest') {
      held = guestCeremony(guestSecret() ?? '');
    } else if (proof === 'email' && sent !== undefined && code !== undefined) {
      held = emailCodeCeremony(sent, code);
    } else if (proof === 'outside' && this.#outsideProof !== null) {
      held = this.#outsideProof;
      // Good once, whatever comes of it.
      this.#outsideProof = null;
    } else {
      throw new Error('This page cannot add a passkey to this account.');
    }
    const token = await this.token();
    const opened = await beginClaim(this.endpoint, token, held);
    await claimPasskey(this.endpoint, token, opened.claim);
    if (proof === 'guest') keepGuestSecret(null);
  }

  /** Run `listener` once when the session ends. */
  onEnd(listener: () => void): () => void {
    this.#onEnd.add(listener);
    return () => this.#onEnd.delete(listener);
  }

  /**
   * Ask for a passkey touch and take the session it re-mints, which carries
   * presence from now. Platform ops (`std.space.share` and the like) ask for
   * a touch no older than five minutes.
   */
  async stepUp(): Promise<void> {
    const done = await stepUp(this.endpoint, await this.token());
    this.#token = done.token;
    this.#expiresAt = done.expiresAt;
    this.#sessionExpiresAt = done.sessionExpiresAt;
  }

  /**
   * Sign out: the door ends the session, so the token is refused from now on
   * whoever holds it. The page then discards it. The caller purges the device.
   */
  async signOut(): Promise<void> {
    if (this.#ended) return;
    if (this.#pasted) {
      this.#end();
      return;
    }
    const token = this.#token;
    // The door answers 204 whatever the token was. A failure to reach it is
    // reported: pretending to be signed out while the door still honours the
    // token is worse than saying it did not work.
    // `include`, so the browser drops the renewal cookie the answer clears.
    const response = await this.#fetch(`${this.endpoint.baseUrl.replace(/\/+$/, '')}/auth/logout`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${token}`, Accept: 'application/json' },
      credentials: 'include',
    });
    if (response.status !== 204) {
      throw new Error('The deployment could not end this session. You are still signed in.');
    }
    this.#end();
  }

  async #renew(): Promise<void> {
    let response: Response;
    try {
      response = await this.#fetch(`${this.endpoint.baseUrl.replace(/\/+$/, '')}/auth/refresh`, {
        method: 'POST',
        headers: { Authorization: `Bearer ${this.#token}`, Accept: 'application/json' },
      });
    } catch {
      // No network. The session is not over; the next call tries again.
      throw new Error('The deployment could not be reached to renew the session.');
    }
    if (response.status === 401 || response.status === 403) {
      this.#end();
      throw new SessionEnded();
    }
    if (!response.ok) throw new Error('The deployment could not renew the session.');
    const body = (await response.json()) as Record<string, unknown>;
    if (typeof body['token'] !== 'string' || typeof body['expires_at'] !== 'number') {
      throw new Error('The deployment answered a renewal this page cannot read.');
    }
    this.#token = body['token'];
    this.#expiresAt = body['expires_at'];
    if (typeof body['session_expires_at'] === 'number') {
      this.#sessionExpiresAt = body['session_expires_at'];
    }
  }

  #end(): void {
    this.#ended = true;
    for (const listener of this.#onEnd) listener();
    this.#onEnd.clear();
  }
}

function endpointOf(options: SessionOptions): PasskeyEndpoint {
  return {
    baseUrl: options.baseUrl,
    ...(options.fetch ? { fetch: options.fetch } : {}),
    ...(options.credentials ? { credentials: options.credentials } : {}),
    ...(options.callTimeoutMs !== undefined ? { callTimeoutMs: options.callTimeoutMs } : {}),
    ...(options.network ? { network: options.network } : {}),
  };
}

/**
 * The `fetch` the hooks' client sends through. It names a failure to get an
 * answer as the SDK does (`OrdoNetworkError`), which the hooks' own client
 * does not.
 */
export function namedFetchOf(endpoint: PasskeyEndpoint): typeof globalThis.fetch {
  return namedFetch({
    baseUrl: endpoint.baseUrl,
    ...(endpoint.fetch ? { fetch: endpoint.fetch } : {}),
    ...(endpoint.callTimeoutMs !== undefined ? { callTimeoutMs: endpoint.callTimeoutMs } : {}),
    ...endpoint.network,
  });
}

export interface WaitOptions {
  readonly wait?: (ms: number) => Promise<void>;
  readonly attempts?: number;
  readonly everyMs?: number;
}

/**
 * Wait until the door serves a tenant that was made a moment ago.
 *
 * The deployment tells every replica of a new tenant by a notification, and
 * each rebuilds its door, so the first call after a sign-up (or after
 * accepting a share) can still be refused `access_denied` by a door that has
 * not heard yet. A refusal that goes on is believed after about six seconds.
 */
export async function untilServed(client: OrdoClient, options: WaitOptions = {}): Promise<void> {
  const wait = options.wait ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const attempts = options.attempts ?? 40;
  const every = options.everyMs ?? 150;
  for (let i = 1; ; i++) {
    try {
      await myNotes(client).fetch();
      return;
    } catch (e) {
      const refusedForNow = e instanceof OrdoRefusal && e.class === 'access_denied';
      if (!refusedForNow || i >= attempts) throw e;
      await wait(every);
    }
  }
}

export { PasskeyError };
