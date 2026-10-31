/**
 * The guest's half of sharing between tenants, and the person's own list of
 * tenants (`plan-sharing.md` G7, `plan-provisioning-on-signup.md` P4).
 *
 * | Function | Route | What it does |
 * |---|---|---|
 * | `peekShare` | `POST /auth/share/peek` | what a ticket offers, without spending it; no session needed |
 * | `acceptShare` | `POST /auth/share/accept` | becomes a guest in the sharer's tenant, disclosing a name |
 * | `openWindow` | `POST /auth/window` | a client bound to that tenant that reads through the window |
 * | `leaveShare` | `POST /auth/share/leave` | closes the guest's own window |
 * | `declineShare` | `POST /auth/share/decline` | refuses a share ticket before it is accepted; spends it |
 * | `peekInvitation` | `POST /auth/invitation/peek` | what an invitation offers, without spending it |
 * | `declineInvitation` | `POST /auth/invitation/decline` | refuses an invitation; spends it, and the inviter sees it declined |
 * | `openLink` | `POST /auth/link` | a client for an open share link, with no sign-in |
 * | `windowFeed` | `POST /auth/window` | the window's tenant and a renewing token source, for a client built elsewhere (`@hominemai/ordo-react`) |
 * | `linkFeed` | `POST /auth/link` | the link's tenant and a renewing token source, the same |
 * | `listSpaces` | `GET /auth/spaces` | the tenants this person owns, belongs to or is a guest in |
 *
 * # A window is not a session
 *
 * `openWindow` answers an `OrdoClient` bound to the shared tenant whose token
 * is the **window token**, a capability token for exactly the ops and rows the
 * sharer offered and the guest consented to. It is minted from the person's
 * session, lives no longer than one session life, and is minted again a little
 * before it ends, so a page that stays open keeps reading. It cannot step up,
 * cannot reach a platform op and cannot open a window of its own.
 *
 * When the sharer closes the window, or the guest leaves, the next mint is
 * refused and the client's next call is refused by the door. Nothing is
 * cached that outlives that: a device replica for this tenant is purged by the
 * sync manager on the door's refusal (`SyncManager`), and it is per tenant and
 * per person, so the guest's replica of their own tenant is untouched.
 *
 * A ticket is a bearer secret. It goes to the door and nowhere else, and the
 * display name a guest types is what the sharer is shown.
 */

import { OrdoClient, type TokenSource } from '../core/client.js';
import { PasskeyError, call, list, text, type PasskeyEndpoint, type Wire } from './passkey.js';
import { authRequest } from './request.js';

/** One row a share opens. */
export interface SharedRow {
  readonly entity: string;
  readonly id: string;
  readonly level: string;
}

/** What a ticket offers, as the accept page shows it. */
export interface PeekedShare {
  /** The sharer's tenant. */
  readonly space: string;
  /** Its name, when the deployment has one for it. */
  readonly name: string | null;
  /** What the sharer called this share. */
  readonly label: string;
  /** The ops the guest may call. */
  readonly ops: readonly string[];
  readonly rows: readonly SharedRow[];
  /** When the window closes, seconds since the epoch, whenever it is accepted. */
  readonly windowEndsAt: number;
  /** When the ticket stops being acceptable. */
  readonly ticketExpiresAt: number;
  /** The ticket names a person, and only that person may accept it. */
  readonly named: boolean;
  /** What accepting discloses. Show it. */
  readonly discloses: string;
  /** The owner-reach sentence. Show it. */
  readonly notice: string;
}

/** What accepting made. */
export interface AcceptedShare {
  readonly space: string;
  readonly name: string | null;
  /** The guest's seat in the sharer's tenant. */
  readonly seat: string;
  readonly grant: string;
  /** When the window closes. */
  readonly expires: number;
  readonly ops: readonly string[];
}

/** A tenant a person is in, and how. */
export interface ListedSpace {
  readonly space: string;
  readonly name: string | null;
  /** `null` for a guest. */
  readonly role: string | null;
  /** `member` for a seat with a role, `guest` for a share. */
  readonly via: 'member' | 'guest';
}

/**
 * One call. The door gives one answer to every refusal of a share, so a
 * refused call says only that the share cannot be used.
 */
async function share(
  endpoint: PasskeyEndpoint,
  path: string,
  body: Wire | null,
  token?: string,
  method: 'POST' | 'GET' = 'POST',
): Promise<Wire> {
  try {
    return await call(endpoint, path, body, token, method);
  } catch (e) {
    if (e instanceof PasskeyError && e.failure === 'refused') {
      throw new PasskeyError('refused', 'that share cannot be used');
    }
    throw e;
  }
}

function optText(o: Wire, key: string): string | null {
  const v = o[key];
  return typeof v === 'string' ? v : null;
}

function num(o: Wire, key: string): number {
  const v = o[key];
  if (typeof v !== 'number') throw new PasskeyError('malformed', `the door's answer has no \`${key}\``);
  return v;
}

function rowsOf(o: Wire): SharedRow[] {
  const v = o['rows'];
  if (!Array.isArray(v)) return [];
  return (v as Wire[]).map((r) => ({
    entity: text(r, 'entity'),
    id: text(r, 'id'),
    level: text(r, 'level'),
  }));
}

/**
 * What a ticket offers, without spending it. Needs no session: the page shows
 * this before the person has signed in or signed up.
 */
export async function peekShare(endpoint: PasskeyEndpoint, ticket: string): Promise<PeekedShare> {
  const o = await share(endpoint, '/auth/share/peek', { ticket });
  return {
    space: text(o, 'space'),
    name: optText(o, 'name'),
    label: optText(o, 'label') ?? '',
    ops: list(o, 'ops'),
    rows: rowsOf(o),
    windowEndsAt: num(o, 'window_ends_at'),
    ticketExpiresAt: num(o, 'ticket_expires_at'),
    named: o['named'] === true,
    discloses: text(o, 'discloses'),
    notice: text(o, 'notice'),
  };
}

/**
 * Accept a share with the person's session token. `displayName` is the name
 * the sharer is shown: the guest's consent is to disclose it, so an
 * application asks for it rather than choosing one.
 */
export async function acceptShare(
  endpoint: PasskeyEndpoint,
  token: string,
  ticket: string,
  displayName: string,
): Promise<AcceptedShare> {
  const o = await share(
    endpoint,
    '/auth/share/accept',
    { ticket, display_name: displayName },
    token,
  );
  return {
    space: text(o, 'space'),
    name: optText(o, 'name'),
    seat: text(o, 'seat'),
    grant: text(o, 'grant'),
    expires: num(o, 'expires'),
    ops: list(o, 'ops'),
  };
}

/** Close the guest's own window in `space`. */
export async function leaveShare(
  endpoint: PasskeyEndpoint,
  token: string,
  space: string,
): Promise<void> {
  await share(endpoint, '/auth/share/leave', { space }, token);
}

/**
 * Decline a share ticket before it is accepted. The ticket is spent, so
 * nobody can accept it afterwards, and the decline is on the sharer's tenant
 * record. A ticket that names a person needs that person's session `token`;
 * one that names nobody may be declined by whoever holds it, with none.
 */
export async function declineShare(
  endpoint: PasskeyEndpoint,
  ticket: string,
  token?: string,
): Promise<void> {
  await share(endpoint, '/auth/share/decline', { ticket }, token);
}

/** What an invitation offers, as the page shows it before sign-in. */
export interface PeekedInvitation {
  /** The tenant it admits to. */
  readonly space: string;
  /** The role it admits at; `null` is the tenant's default. */
  readonly role: string | null;
  /** When the invitation stops being redeemable. */
  readonly ticketExpiresAt: number;
  /** The invitation names a person, and only that person may use or decline it. */
  readonly named: boolean;
}

async function invitation(
  endpoint: PasskeyEndpoint,
  path: string,
  body: Wire,
  token?: string,
): Promise<Wire> {
  try {
    return await call(endpoint, path, body, token);
  } catch (e) {
    if (e instanceof PasskeyError && e.failure === 'refused') {
      throw new PasskeyError('refused', 'that invitation cannot be used');
    }
    throw e;
  }
}

/** What an invitation offers, without spending it. Needs no session. */
export async function peekInvitation(
  endpoint: PasskeyEndpoint,
  ticket: string,
): Promise<PeekedInvitation> {
  const o = await invitation(endpoint, '/auth/invitation/peek', { ticket });
  return {
    space: text(o, 'space'),
    role: optText(o, 'role'),
    ticketExpiresAt: num(o, 'ticket_expires_at'),
    named: o['named'] === true,
  };
}

/**
 * Decline an invitation. The ticket is spent, and the tenant's invitation
 * list shows it declined until it would have expired. A named invitation
 * needs that person's session `token`.
 */
export async function declineInvitation(
  endpoint: PasskeyEndpoint,
  ticket: string,
  token?: string,
): Promise<void> {
  await invitation(endpoint, '/auth/invitation/decline', { ticket }, token);
}

/** The tenants this person owns, belongs to or is a guest in. Only their own. */
export async function listSpaces(
  endpoint: PasskeyEndpoint,
  token: string,
): Promise<readonly ListedSpace[]> {
  const response = await authRequest(endpoint, '/auth/spaces', {
    method: 'GET',
    headers: { Accept: 'application/json', Authorization: `Bearer ${token}` },
  });
  if (response.status === 503) throw new PasskeyError('unavailable', 'the door cannot answer right now');
  if (!response.ok) throw new PasskeyError('refused', 'that did not authenticate');
  const json: unknown = await response.json().catch(() => null);
  if (!Array.isArray(json)) {
    throw new PasskeyError('malformed', 'the door answered with something that is not a list');
  }
  return (json as Wire[]).map((r) => ({
    space: text(r, 'space'),
    name: optText(r, 'name'),
    role: optText(r, 'role'),
    via: r['via'] === 'guest' ? 'guest' : 'member',
  }));
}

/** What `openWindow` may be told. */
export interface WindowOptions {
  /**
   * Mint a new window token this many seconds before the current one ends.
   * Default 30.
   */
  readonly refreshBeforeSecs?: number;
  /** Seconds since the epoch. For tests. */
  readonly now?: () => number;
  /** For tests. */
  readonly newIdempotencyKey?: () => string;
}

interface Held {
  readonly token: string;
  readonly expiresAt: number;
  readonly ops: readonly string[];
}

/** The `ops` of a door's answer; an answer without them lists none. */
function opsOf(o: Wire): string[] {
  const ops = o['ops'];
  return Array.isArray(ops) ? ops.filter((x): x is string => typeof x === 'string') : [];
}

/**
 * A client bound to `space` that reads through the window the person holds
 * there.
 *
 * `session` is the person's session token, or a function that answers the
 * current one (the app refreshes it). The first window token is minted here,
 * so a share that is closed or was never made is refused now rather than at
 * the first read. After that the token is minted again a little before it
 * ends, once for any number of calls at the same moment. A mint that is
 * refused after the sharer closed the window is thrown to the call that
 * needed it.
 */
export async function openWindow(
  endpoint: PasskeyEndpoint,
  session: TokenSource,
  space: string,
  options: WindowOptions = {},
): Promise<OrdoClient> {
  const feed = await windowFeed(endpoint, session, space, options);
  return clientOf(endpoint, feed, options);
}

/**
 * A guest's way in, for a client that is not this package's: the tenant, and
 * a function that answers the current token. The token string is never a
 * property of anything: it exists only as the answer of `token()`, which is
 * the shape a client takes as its `token` option (this package's
 * `OrdoClient` and `@hominemai/ordo-react`'s alike). So the page hands a
 * guest's hooks a client without ever holding the token itself.
 */
export interface GuestFeed {
  /** The tenant the window or the link opens. */
  readonly space: string;
  /**
   * The current token, minted again a little before it ends. The SDK's own
   * feeds answer a promise; a hand-built feed may answer the string.
   */
  readonly token: () => string | Promise<string>;
  /**
   * The ops the share or link lets this guest call, as the door answered at
   * the last mint. A page asks this before it reads an op the share may not
   * list: a read the door refuses with `access_denied` marks a device copy
   * revoked, and that empties every op it holds. A share made under an
   * earlier version of an app lists fewer ops than the app reads now.
   */
  readonly ops?: () => readonly string[];
  /**
   * Mint the token again now, after the door refused the one held. `true`
   * when a new one was minted, `false` when the door refused (the share is
   * closed). A window token names one grant, and a second accept replaces
   * that grant with their union, so the held token is refused while the
   * window is still open. Pass it to a client as its `renewToken`.
   */
  readonly renew?: () => Promise<boolean>;
}

/**
 * What {@link openWindow} does before it builds a client: the first window
 * token is minted here (a closed or unknown share is refused now), and
 * `token()` renews it as {@link openWindow} documents.
 */
export async function windowFeed(
  endpoint: PasskeyEndpoint,
  session: TokenSource,
  space: string,
  options: WindowOptions = {},
): Promise<GuestFeed> {
  const now = options.now ?? (() => Math.floor(Date.now() / 1000));
  const skew = options.refreshBeforeSecs ?? 30;
  const mint = async (): Promise<Held> => {
    const bearer = typeof session === 'function' ? await session() : session;
    const o = await share(endpoint, '/auth/window', { space }, bearer);
    return { token: text(o, 'token'), expiresAt: num(o, 'expires_at'), ops: opsOf(o) };
  };
  let held: Held = await mint();
  let minting: Promise<Held> | null = null;
  // One mint for every call that needs one at the same moment.
  const fresh = async (): Promise<Held> => {
    minting ??= mint().finally(() => {
      minting = null;
    });
    held = await minting;
    return held;
  };
  const token = async (): Promise<string> => {
    if (held.expiresAt - now() > skew) return held.token;
    return (await fresh()).token;
  };
  const renew = async (): Promise<boolean> => {
    try {
      await fresh();
      return true;
    } catch (e) {
      if (e instanceof PasskeyError && e.failure === 'refused') return false;
      throw e;
    }
  };
  return { space, token, ops: () => held.ops, renew };
}

function clientOf(
  endpoint: PasskeyEndpoint,
  feed: GuestFeed,
  options: { readonly newIdempotencyKey?: () => string },
): OrdoClient {
  const clientOptions: ConstructorParameters<typeof OrdoClient>[0] = {
    baseUrl: endpoint.baseUrl,
    space: feed.space,
    token: feed.token,
    ...(feed.renew ? { renewToken: feed.renew } : {}),
    ...(endpoint.fetch ? { fetch: endpoint.fetch } : {}),
    ...(endpoint.busyRetry ? { busyRetry: endpoint.busyRetry } : {}),
    ...(options.newIdempotencyKey ? { newIdempotencyKey: options.newIdempotencyKey } : {}),
  };
  return new OrdoClient(clientOptions);
}

/** What `openLink` may be told. */
export interface LinkOptions {
  /**
   * The link's password, when the sharer set one. It goes to `POST /auth/link`
   * beside the secret, on the first open and on each renewal, and nowhere else.
   */
  readonly password?: string;
  /**
   * A random id for this device, sent as `device` with each open. The door
   * counts an open against the link's maximum once per device, so a renewal
   * or a reload is not a new open. Default: one kept in `localStorage` under
   * `ordo.link.device`, else one for this call only (then a reload counts
   * again). It is not a credential and the door stores only a salted hash.
   */
  readonly device?: string;
  /**
   * Open the link again this many seconds before the current token ends.
   * Default 30.
   */
  readonly refreshBeforeSecs?: number;
  /** Seconds since the epoch. For tests. */
  readonly now?: () => number;
  /** For tests. */
  readonly newIdempotencyKey?: () => string;
}

/**
 * Where a link stands, as `std.space.links` answers it. `used-up` is a link
 * that reached its maximum number of opens.
 */
export type LinkState = 'live' | 'expired' | 'revoked' | 'used-up';

/** One row of `std.space.links`, as the sharer sees it. Never the secret. */
export interface ListedLink {
  readonly id: string;
  readonly label: string;
  readonly ops: readonly string[];
  readonly rows: readonly SharedRow[];
  /** Every successful open, renewals included. */
  readonly opens: number;
  readonly createdAt: number;
  readonly expiresAt: number;
  readonly state: LinkState;
  readonly issuedBy: string;
  readonly hasPassword: boolean;
  /** When a lock after too many wrong passwords ends, or `null`. */
  readonly lockedUntil: number | null;
  /** Opens counted against `maxOpens`: the devices that have opened it. */
  readonly opensUsed: number;
  /** The most opens the link allows, or `null` for no limit. */
  readonly maxOpens: number | null;
}

/**
 * The `maxOpens` parameter of `std.space.shareLink` and
 * `std.space.setLinkMaxOpens`: a whole number from 1, as text, or `none`.
 */
export type MaxOpensParam = `${number}` | 'none';

const DEVICE_KEY = 'ordo.link.device';

/** This device's id: the caller's, else the stored one, else a new one. */
function deviceOf(options: LinkOptions): string {
  if (options.device) return options.device;
  const fresh = (): string => {
    const c = globalThis.crypto;
    if (c?.randomUUID) return c.randomUUID();
    return Array.from({ length: 4 }, () => Math.random().toString(16).slice(2, 10)).join('');
  };
  try {
    const store = globalThis.localStorage;
    const held = store?.getItem(DEVICE_KEY);
    if (held) return held;
    const made = fresh();
    store?.setItem(DEVICE_KEY, made);
    return made;
  } catch {
    return fresh();
  }
}

interface Opened extends Held {
  readonly space: string;
}

/**
 * A client for an open share link (`plan-sharing.md` L5). It reads what the
 * sharer opened, through the link token, with no sign-in.
 *
 * `link` is the secret the sharer minted, and it is a bearer credential:
 * whoever has it reads. It goes to `POST /auth/link` and nowhere else. The
 * tenant comes from the door's answer, not from the caller, because a link
 * belongs to one tenant and the holder need not know which. The first token
 * is opened here, so a link that is unknown, expired or revoked is refused
 * now and not at the first read. The link token is opened again a little
 * before it ends, once for any number of calls at that moment, so a page that
 * stays open keeps reading until the link ends. When the sharer revokes the
 * link, the next call is refused by the door and the next open is refused.
 *
 * A refusal is one message whatever the reason, and an address that has
 * missed too often is refused as one too.
 *
 * A link with a password is the exception, because the holder of the secret
 * is owed it. Each is a `PasskeyError` with its own `failure`, and each is
 * retryable: `link_password_required` (ask for the password and call again
 * with `password`), `link_password_wrong` (ask again), and `link_locked`
 * (too many wrong tries; `retryAfter` is the seconds until it opens again).
 * A lock is off unless the tenant, the app developer or the deployment owner
 * turned it on.
 *
 * A link the sharer limited to a number of opens, once that many devices have
 * opened it, is refused like an expired one: `link_refused`, the one message,
 * as {@link LinkState} `used-up` on the sharer's side. A device that already
 * opened it still renews.
 */
export async function openLink(
  endpoint: PasskeyEndpoint,
  link: string,
  options: LinkOptions = {},
): Promise<OrdoClient> {
  return clientOf(endpoint, await linkFeed(endpoint, link, options), options);
}

/** What {@link openLink} does before it builds a client; see {@link GuestFeed}. */
export async function linkFeed(
  endpoint: PasskeyEndpoint,
  link: string,
  options: LinkOptions = {},
): Promise<GuestFeed> {
  const now = options.now ?? (() => Math.floor(Date.now() / 1000));
  const skew = options.refreshBeforeSecs ?? 30;
  const device = deviceOf(options);
  const open = async (): Promise<Opened> => {
    try {
      const body: Wire =
        options.password === undefined ? { link, device } : { link, password: options.password, device };
      const o = await call(endpoint, '/auth/link', body);
      return { token: text(o, 'token'), expiresAt: num(o, 'expires_at'), space: text(o, 'space'), ops: opsOf(o) };
    } catch (e) {
      if (e instanceof PasskeyError && e.failure === 'refused') {
        throw new PasskeyError('refused', 'that link cannot be used');
      }
      throw e;
    }
  };
  let held: Opened = await open();
  const space = held.space;
  let opening: Promise<Opened> | null = null;
  const token = async (): Promise<string> => {
    if (held.expiresAt - now() > skew) return held.token;
    opening ??= open().finally(() => {
      opening = null;
    });
    held = await opening;
    return held.token;
  };
  return { space, token, ops: () => held.ops };
}
