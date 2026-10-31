/**
 * One sign-in across subdomains: the hand-off.
 *
 * The core site (for example `login.example.com`) signs the person in. It then
 * sends the browser back to the subdomain page with a one-time code. The
 * subdomain page swaps the code for its own session. There is no shared cookie
 * and no token in a URL.
 *
 * | Function | Side | Route | Yields |
 * |---|---|---|---|
 * | `startSubdomainSignIn` | subdomain | none | the core site address to send the person to |
 * | `readHandoffRequest` | core | none | `returnTo` and `binderHash`, or `null` |
 * | `issueHandoff` | core | `POST /auth/handoff` | the address to send the browser to |
 * | `finishSubdomainSignIn` | subdomain | `POST /auth/handoff/redeem` | a session, or `null` |
 *
 * # The rules that make it work
 *
 * The **binder** is a random secret the subdomain page makes and keeps in
 * `sessionStorage`. Only its SHA-256 (`binder_hash`) travels in the URL. The
 * code in the URL is useless without the binder, so a stolen code cannot be
 * redeemed.
 *
 * The hash is taken over the binder's text, the 43 base64url characters, not
 * over the raw bytes.
 *
 * The door refuses a `return_to` whose origin the app developer did not allow
 * in `ORDO_AUTH_RETURN_URLS`. A refusal is a `PasskeyError` with failure
 * `refused`. A store that cannot be read is `unavailable`.
 *
 * The new session has no presence. A step-up on the subdomain runs its own
 * passkey ceremony.
 */

import {
  call,
  session,
  text,
  toBase64Url,
  PasskeyError,
  type PasskeyEndpoint,
  type PasskeySession,
} from './passkey.js';

/** The part of `Storage` this module uses. `sessionStorage` fits. */
export interface HandoffStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** The part of `History` this module uses. */
export interface HandoffHistory {
  replaceState(data: unknown, unused: string, url?: string | URL | null): void;
}

/** Where the binder is kept between the two legs. */
export const HANDOFF_BINDER_KEY = 'ordo.handoff.binder';

const CODE_PARAM = 'handoff_code';

/** 32 random bytes as 43 base64url characters. */
export function makeBinder(): string {
  const bytes = new Uint8Array(32);
  globalThis.crypto.getRandomValues(bytes);
  return toBase64Url(bytes);
}

/** SHA-256 of the binder's text, base64url without padding. */
export async function binderHash(binder: string): Promise<string> {
  const digest = await globalThis.crypto.subtle.digest('SHA-256', new TextEncoder().encode(binder));
  return toBase64Url(digest);
}

function storageOf(given: HandoffStorage | undefined): HandoffStorage {
  const s = given ?? (globalThis as { sessionStorage?: HandoffStorage }).sessionStorage;
  if (!s) throw new PasskeyError('unsupported', 'this page has no sessionStorage to keep the binder in');
  return s;
}

function here(): string {
  const loc = (globalThis as { location?: { href: string } }).location;
  if (!loc) throw new PasskeyError('unsupported', 'there is no page address; pass `url` or `returnTo`');
  return loc.href;
}

/** What the subdomain page passes to start. */
export interface SubdomainSignInStart {
  /** The core site page that signs the person in and calls `issueHandoff`. */
  readonly coreUrl: string;
  /** Where to land afterwards. Defaults to the current address without `handoff_code`. */
  readonly returnTo?: string;
  /** Where the binder is kept. Defaults to `sessionStorage`. */
  readonly storage?: HandoffStorage;
}

function withoutCode(url: string): string {
  const u = new URL(url);
  u.searchParams.delete(CODE_PARAM);
  return u.toString();
}

/**
 * Make a binder, keep it, and return the core site address to send the person
 * to. The address carries `return_to` and `binder_hash`. The binder itself
 * stays in storage.
 */
export async function startSubdomainSignIn(start: SubdomainSignInStart): Promise<string> {
  const binder = makeBinder();
  storageOf(start.storage).setItem(HANDOFF_BINDER_KEY, binder);
  const target = new URL(start.coreUrl);
  target.searchParams.set('return_to', withoutCode(start.returnTo ?? here()));
  target.searchParams.set('binder_hash', await binderHash(binder));
  return target.toString();
}

/** What the subdomain page passes to finish. */
export interface SubdomainSignInFinish extends Pick<PasskeyEndpoint, 'baseUrl' | 'fetch'> {
  /** The page address. Defaults to the current location. */
  readonly url?: string;
  /** Where the binder was kept. Defaults to `sessionStorage`. */
  readonly storage?: HandoffStorage;
  /** Used to remove `handoff_code` from the address bar. Defaults to `history`. */
  readonly history?: HandoffHistory;
}

/**
 * Finish on the subdomain page. Returns `null` when the address has no
 * `handoff_code`. Otherwise redeems it with the stored binder, removes the
 * binder, and strips the code from the address bar.
 *
 * Throws a `PasskeyError` when the binder is missing (no door call is made), or
 * when the door refuses.
 */
export async function finishSubdomainSignIn(
  finish: SubdomainSignInFinish,
): Promise<PasskeySession | null> {
  const url = new URL(finish.url ?? here());
  const code = url.searchParams.get(CODE_PARAM);
  if (!code) return null;
  const store = storageOf(finish.storage);
  const binder = store.getItem(HANDOFF_BINDER_KEY);
  if (!binder) {
    throw new PasskeyError(
      'refused',
      'no binder is kept for this hand-off: start the sign-in on this page again',
    );
  }
  const endpoint: PasskeyEndpoint = { baseUrl: finish.baseUrl };
  const wire = await call(
    finish.fetch ? { ...endpoint, fetch: finish.fetch } : endpoint,
    '/auth/handoff/redeem',
    { code, binder },
  );
  const result = session(wire);
  store.removeItem(HANDOFF_BINDER_KEY);
  const history = finish.history ?? (globalThis as { history?: HandoffHistory }).history;
  if (history) {
    url.searchParams.delete(CODE_PARAM);
    history.replaceState(null, '', url.pathname + url.search + url.hash);
  }
  return result;
}

/** What the core site page was asked for. */
export interface HandoffRequest {
  readonly returnTo: string;
  readonly binderHash: string;
}

/**
 * Read `return_to` and `binder_hash` from the core page's own address. Returns
 * `null` when either is missing.
 */
export function readHandoffRequest(url: string): HandoffRequest | null {
  const params = new URL(url).searchParams;
  const returnTo = params.get('return_to');
  const hash = params.get('binder_hash');
  if (!returnTo || !hash) return null;
  return { returnTo, binderHash: hash };
}

/** What the core site passes to issue a hand-off. */
export interface HandoffIssue extends Pick<PasskeyEndpoint, 'baseUrl' | 'fetch'> {
  /** The signed-in session's token. */
  readonly token: string;
  readonly returnTo: string;
  readonly binderHash: string;
}

/** A hand-off the door filed. */
export interface HandoffIssued {
  /** `returnTo` with `handoff_code` added. Send the browser here. */
  readonly redirect: string;
  /** When the code stops working, seconds since the epoch. */
  readonly expiresAt: number;
}

/** Ask the door for a one-time code for `returnTo`, bound to `binderHash`. */
export async function issueHandoff(issue: HandoffIssue): Promise<HandoffIssued> {
  const endpoint: PasskeyEndpoint = { baseUrl: issue.baseUrl };
  const wire = await call(
    issue.fetch ? { ...endpoint, fetch: issue.fetch } : endpoint,
    '/auth/handoff',
    { return_to: issue.returnTo, binder_hash: issue.binderHash },
    issue.token,
  );
  const expires = wire['expires_at'];
  if (typeof expires !== 'number') {
    throw new PasskeyError('malformed', "the door's answer has no `expires_at`");
  }
  return { redirect: text(wire, 'redirect'), expiresAt: expires };
}
