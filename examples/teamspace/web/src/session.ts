/**
 * The signed-in person: a token held in memory, kept fresh, and the client
 * bound to the team they work in.
 *
 * Every way in ends here: a passkey, an emailed code, an outside connection,
 * or a guest start. The token is never written to `localStorage` or anywhere
 * else the page can be read back from after a reload. A reload signs the
 * person in again.
 *
 * A sign-in names one tenant of the person's own (`raw.space`). A colleague
 * who joined by invitation also belongs to the team that invited them, so the
 * session lists the person's teams and the page binds one with `useTeam`.
 */

import {
  PasskeyError,
  registerPasskey,
  signInWithPasskey,
  stepUp,
  type PasskeyEndpoint,
  type PasskeySession,
  type SignInOptions,
} from '@hominemai/ordo-sdk/auth/passkey.js';
import { beginEmailSignIn, completeEmailSignIn, type EmailCodeSent } from '@hominemai/ordo-sdk/auth/email.js';
import { resumeGuest, startGuest } from '@hominemai/ordo-sdk/auth/guest.js';
import type { SignInResult } from '@hominemai/ordo-sdk/auth/second-step.js';
import { listSpaces, type ListedSpace } from '@hominemai/ordo-sdk/auth/share.js';
import { waitForTenant } from '@hominemai/ordo-sdk/auth/provisioning.js';
import { DescriptorClient as HooksClient } from '@hominemai/ordo-react';
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { OrdoRefusal } from '@hominemai/ordo-sdk/core/refusal.js';
import { me, type MeRow } from './generated/ops.js';
import { INTERFACE } from './interface.js';
import { completeOutside, guestSecret, keepDeviceMarker, keepGuestSecret } from './signin.js';

export interface SessionOptions {
  readonly baseUrl: string;
  readonly fetch?: typeof globalThis.fetch;
  readonly credentials?: PasskeyEndpoint['credentials'];
  /** Seconds since the epoch. Replaced in tests. */
  readonly now?: () => number;
}

/** The team and the ticket from an invitation link. */
export interface Join {
  readonly space: string;
  readonly invitation: string;
}

/** The sign-in worked and the deployment gave this person no team. */
export class NoTeamError extends Error {
  readonly reason: string;
  constructor(reason: string) {
    super(
      reason === 'in progress'
        ? 'Your team is still being set up. Try again in a moment.'
        : `No team could be found for you (${reason}). Ask the person who invited you for a new link.`,
    );
    this.name = 'NoTeamError';
    this.reason = reason;
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

/** The sign-in options an invitation link adds. */
const optionsOf = (join?: Join): SignInOptions => (join ? { space: join.space, invitation: join.invitation } : {});

export class Session {
  readonly endpoint: PasskeyEndpoint;
  /** The person's own tenant, as the sign-in named it, or null. */
  readonly ownSpace: string | null;
  /** The team the sign-in admitted this person to (`admitted` or `already-member`), or null. */
  readonly admittedSpace: string | null;
  /** How this session began, as the door says it: `passkey`, `email-otp`, `guest`, `oauth2:discord`, ... */
  readonly via: string;

  readonly #now: () => number;
  readonly #fetch: typeof globalThis.fetch;
  #space: string | null;
  #token: string;
  #expiresAt: number;
  #refreshing: Promise<void> | null = null;
  #ended = false;
  readonly #onEnd = new Set<() => void>();

  private constructor(options: SessionOptions, done: PasskeySession) {
    this.endpoint = {
      baseUrl: options.baseUrl,
      ...(options.fetch ? { fetch: options.fetch } : {}),
      ...(options.credentials ? { credentials: options.credentials } : {}),
    };
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.#now = options.now ?? (() => Math.floor(Date.now() / 1000));
    this.#token = done.token;
    this.#expiresAt = done.expiresAt;
    const own = done.raw['space'];
    this.ownSpace = typeof own === 'string' && own !== '' ? own : null;
    const outcome = done.admission?.outcome;
    this.admittedSpace =
      done.admission && done.admission.space !== '' && (outcome === 'admitted' || outcome === 'already-member')
        ? done.admission.space
        : null;
    const via = done.raw['via'];
    this.via = typeof via === 'string' ? via : 'passkey';
    this.#space = null;
  }

  /** A form and a passkey: register. With a `join`, the person is admitted to that team. */
  static async signUp(options: SessionOptions, display: string, join?: Join): Promise<Session> {
    const done = await registerPasskey(endpointOf(options), { display, ...optionsOf(join) });
    return new Session(options, await withTenant(options, done));
  }

  /** The passkey button, for a person who has an account. */
  static async signIn(options: SessionOptions, join?: Join): Promise<Session> {
    return new Session(options, await withTenant(options, await signInWithPasskey(endpointOf(options), optionsOf(join))));
  }

  /** Mail a code. The same `join` goes to the second half. */
  static beginEmail(options: SessionOptions, address: string, device: string | undefined, join?: Join): Promise<EmailCodeSent> {
    return beginEmailSignIn(endpointOf(options), { address, ...(device ? { device } : {}), ...optionsOf(join) });
  }

  /** The second half of an emailed-code sign-in: the code the person typed. */
  static async withEmail(options: SessionOptions, sent: EmailCodeSent, code: string, join?: Join): Promise<Session> {
    const done = Session.#session(await completeEmailSignIn(endpointOf(options), sent, code, optionsOf(join)));
    keepDeviceMarker(done.device);
    return new Session(options, await withTenant(options, done));
  }

  /**
   * Continue as a guest. With a guest secret on this device, that guest comes
   * back. Otherwise a new one starts and its secret is kept here.
   */
  static async asGuest(options: SessionOptions, join?: Join): Promise<Session> {
    const endpoint = endpointOf(options);
    const secret = guestSecret();
    if (secret !== null) {
      return new Session(options, await withTenant(options, Session.#session(await resumeGuest(endpoint, secret, optionsOf(join)))));
    }
    const started = await startGuest(endpoint, optionsOf(join));
    keepGuestSecret(started.secret);
    return new Session(options, await withTenant(options, Session.#session(started.result)));
  }

  /** The person came back from an outside connection with a hand-off code. */
  static async fromOutside(
    options: SessionOptions,
    back: { readonly method: string; readonly handoff: string; readonly binder: string },
    join?: Join,
  ): Promise<Session> {
    return new Session(
      options,
      await withTenant(options, Session.#session(await completeOutside(endpointOf(options), back, optionsOf(join)))),
    );
  }

  /** A session, or the second step this page does not offer. */
  static #session(r: SignInResult): PasskeySession {
    if (r.kind === 'second-step') throw new SecondStepNeeded();
    return r.session;
  }

  /** The team this session works in, once `useTeam` has chosen one. */
  get space(): string | null {
    return this.#space;
  }

  /** Work in this team from now on. */
  useTeam(space: string): void {
    this.#space = space;
  }

  /** Every team this person is in, as the door lists them. */
  async teams(): Promise<readonly ListedSpace[]> {
    return listSpaces(this.endpoint, await this.token());
  }

  get ended(): boolean {
    return this.#ended;
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

  #bound(): string {
    if (this.#space === null) throw new NoTeamError('no team chosen');
    return this.#space;
  }

  /** A client bound to the chosen team. */
  client(): OrdoClient {
    return new OrdoClient({
      baseUrl: this.endpoint.baseUrl,
      space: this.#bound(),
      token: () => this.token(),
      ...(this.endpoint.fetch ? { fetch: this.endpoint.fetch } : {}),
    });
  }

  /** The same team and token for `@hominemai/ordo-react`'s hooks, renewed as the token is. */
  hooksClient(): HooksClient {
    return new HooksClient({
      baseUrl: this.endpoint.baseUrl,
      iface: INTERFACE,
      space: this.#bound(),
      token: () => this.token(),
      ...(this.endpoint.fetch ? { fetch: this.endpoint.fetch } : {}),
    });
  }

  /** Run `listener` once when the session ends. */
  onEnd(listener: () => void): () => void {
    this.#onEnd.add(listener);
    return () => this.#onEnd.delete(listener);
  }

  /**
   * Ask for a passkey touch and take the session it re-mints, which carries
   * presence from now. Platform ops (`std.space.invite` and the like) ask for
   * a touch no older than five minutes.
   */
  async stepUp(): Promise<void> {
    const done = await stepUp(this.endpoint, await this.token());
    this.#token = done.token;
    this.#expiresAt = done.expiresAt;
  }

  /**
   * Sign out: the door ends the session, so the token is refused from now on
   * whoever holds it. The page then discards it.
   */
  async signOut(): Promise<void> {
    if (this.#ended) return;
    const token = this.#token;
    // The door answers 204 whatever the token was. A failure to reach it is
    // reported: pretending to be signed out while the door still honours the
    // token is worse than saying it did not work.
    const response = await this.#fetch(`${this.endpoint.baseUrl.replace(/\/+$/, '')}/auth/logout`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${token}`, Accept: 'application/json' },
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
  };
}

export interface WaitOptions {
  readonly wait?: (ms: number) => Promise<void>;
  readonly attempts?: number;
  readonly everyMs?: number;
}

/**
 * Read the caller's seat, waiting until the door serves a team that was made
 * or joined a moment ago.
 *
 * The deployment tells every replica of a new tenant by a notification, and
 * each rebuilds its door, so the first call after a sign-up can still be
 * refused `access_denied` by a door that has not heard yet. A refusal that
 * goes on is believed after about six seconds.
 */
export async function untilServed(client: OrdoClient, options: WaitOptions = {}): Promise<MeRow | undefined> {
  const wait = options.wait ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const attempts = options.attempts ?? 40;
  const every = options.everyMs ?? 150;
  for (let i = 1; ; i++) {
    try {
      return (await me(client).fetch()).rows[0];
    } catch (e) {
      const refusedForNow = e instanceof OrdoRefusal && e.class === 'access_denied';
      if (!refusedForNow || i >= attempts) throw e;
      await wait(every);
    }
  }
}

export { PasskeyError };

/**
 * The sign-in's answer with the person's own team in it. On a deployment that
 * gives each person one, it is made after the sign-in answers
 * (`provisioning: "in progress"`), so this waits for it. A team that could
 * not be made leaves the person without one, as before.
 */
async function withTenant(options: SessionOptions, done: PasskeySession): Promise<PasskeySession> {
  const space = done.raw['space'];
  if ((typeof space === 'string' && space !== '') || done.raw['provisioning'] !== 'in progress') return done;
  try {
    const made = await waitForTenant(endpointOf(options), done.token);
    return { ...done, raw: { ...done.raw, space: made, provisioning: null } };
  } catch {
    return done;
  }
}
