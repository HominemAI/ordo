/**
 * The signed-in person on one of the three pages: a token kept in this
 * origin's `sessionStorage`, renewed before it ends, and the client bound to
 * the shop's tenant.
 *
 * Each page keeps its own session under its own origin, so the shop's token
 * is never readable from the sign-in page or the console, and the reverse.
 * `sessionStorage` lives as long as the tab: a reload keeps the person
 * signed in, closing the tab does not.
 */

import type { PasskeyEndpoint, PasskeySession } from '@hominemai/ordo-sdk/auth/passkey.js';
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';

/** Storage that may be missing or may throw (a private window, blocked site data). */
export type Store = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

export function sessionStore(): Store | undefined {
  try {
    return typeof sessionStorage === 'undefined' ? undefined : sessionStorage;
  } catch {
    return undefined;
  }
}

/** What is kept between page loads. */
interface Saved {
  readonly token: string;
  readonly expiresAt: number;
  readonly sessionExpiresAt: number;
  readonly admission?: { readonly space: string; readonly outcome: string } | undefined;
}

export interface SessionOptions {
  /** Where this session is kept: a key in `store`. */
  readonly key: string;
  readonly baseUrl: string;
  /** The shop's tenant. */
  readonly space: string;
  readonly store?: Store | undefined;
  readonly fetch?: typeof globalThis.fetch;
  /** Seconds since the epoch. Replaced in tests. */
  readonly now?: () => number;
}

/** The session ended: logged out, expired for good, or refused by the door. */
export class SessionEnded extends Error {
  constructor() {
    super('You are signed out. Sign in again to continue.');
    this.name = 'SessionEnded';
  }
}

/** Renew this many seconds before the token ends. */
const RENEW_BEFORE_SECS = 60;

export class Session {
  readonly space: string;
  readonly endpoint: PasskeyEndpoint;
  /** What the door did about the person's arrival in the shop's tenant, if the sign-in named it. */
  readonly admission: { readonly space: string; readonly outcome: string } | undefined;

  readonly #options: SessionOptions;
  readonly #now: () => number;
  readonly #fetch: typeof globalThis.fetch;
  #token: string;
  #expiresAt: number;
  #sessionExpiresAt: number;
  #refreshing: Promise<void> | null = null;
  #ended = false;
  readonly #onEnd = new Set<() => void>();

  private constructor(options: SessionOptions, saved: Saved) {
    this.#options = options;
    this.space = options.space;
    this.endpoint = { baseUrl: options.baseUrl, ...(options.fetch ? { fetch: options.fetch } : {}) };
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.#now = options.now ?? (() => Math.floor(Date.now() / 1000));
    this.#token = saved.token;
    this.#expiresAt = saved.expiresAt;
    this.#sessionExpiresAt = saved.sessionExpiresAt;
    this.admission = saved.admission;
  }

  /** Take a sign-in's answer and keep it. */
  static from(options: SessionOptions, done: PasskeySession): Session {
    const session = new Session(options, {
      token: done.token,
      expiresAt: done.expiresAt,
      sessionExpiresAt: done.sessionExpiresAt,
      admission: done.admission,
    });
    session.#save();
    return session;
  }

  /** The session this tab kept, or null when there is none or it is over. */
  static restore(options: SessionOptions): Session | null {
    let raw: string | null = null;
    try {
      raw = options.store?.getItem(options.key) ?? null;
    } catch {
      return null;
    }
    if (raw === null) return null;
    try {
      const saved = JSON.parse(raw) as Partial<Saved> | null;
      if (
        !saved ||
        typeof saved.token !== 'string' ||
        typeof saved.expiresAt !== 'number' ||
        typeof saved.sessionExpiresAt !== 'number'
      ) {
        return null;
      }
      const now = options.now ?? (() => Math.floor(Date.now() / 1000));
      if (saved.sessionExpiresAt <= now()) {
        options.store?.removeItem(options.key);
        return null;
      }
      return new Session(options, saved as Saved);
    } catch {
      return null;
    }
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

  /** A client bound to the shop's tenant. */
  client(): OrdoClient {
    return new OrdoClient({
      baseUrl: this.endpoint.baseUrl,
      space: this.space,
      token: () => this.token(),
      ...(this.endpoint.fetch ? { fetch: this.endpoint.fetch } : {}),
    });
  }

  /** Run `listener` once when the session ends. */
  onEnd(listener: () => void): () => void {
    this.#onEnd.add(listener);
    return () => this.#onEnd.delete(listener);
  }

  /** Forget this session in this tab. The door still honours the token until it ends. */
  forget(): void {
    this.#end();
  }

  /** Sign out at the door: the token is refused from now on, whoever holds it. */
  async signOut(): Promise<void> {
    if (this.#ended) return;
    const response = await this.#fetch(`${this.endpoint.baseUrl.replace(/\/+$/, '')}/auth/logout`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${this.#token}`, Accept: 'application/json' },
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
    if (typeof body['session_expires_at'] === 'number') this.#sessionExpiresAt = body['session_expires_at'];
    this.#save();
  }

  #save(): void {
    const saved: Saved = {
      token: this.#token,
      expiresAt: this.#expiresAt,
      sessionExpiresAt: this.#sessionExpiresAt,
      admission: this.admission,
    };
    try {
      this.#options.store?.setItem(this.#options.key, JSON.stringify(saved));
    } catch {
      // Nothing kept. The person signs in again after a reload.
    }
  }

  #end(): void {
    this.#ended = true;
    try {
      this.#options.store?.removeItem(this.#options.key);
    } catch {
      // Nothing kept, nothing to forget.
    }
    for (const listener of this.#onEnd) listener();
    this.#onEnd.clear();
  }
}
