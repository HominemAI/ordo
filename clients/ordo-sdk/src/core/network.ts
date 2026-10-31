/**
 * One error for a call that never got an answer.
 *
 * A browser reports three different failures the same way: a cross-origin
 * call the server refuses, a server that is down, and a server too slow. All
 * three reach a page as `TypeError: Failed to fetch`, or as a timeout the SDK
 * set itself. {@link OrdoNetworkError} tells them apart, so an app developer
 * writes one `catch` and each kind names what fixes it.
 *
 * - `timeout`: the server did not answer within the wait. The error names the
 *   option that sets the wait.
 * - `unreachable`: the server could not be reached at all.
 * - `blocked`: the server is up, and the browser refused the call because the
 *   page's origin is not allowed. The deployment owner adds the origin to
 *   `ORDO_CORS_ORIGINS` (and to `ORDO_AUTH_ORIGINS` for a passkey ceremony).
 *
 * To tell `blocked` from `unreachable`, the SDK sends one `mode: 'no-cors'`
 * request to a cheap health path. The browser hides what the server answered,
 * but it does not hide that the server answered: an opaque answer means the
 * server is up. The answer is kept for a few seconds, so a burst of failures
 * sends one probe.
 */

import { CallTimeout } from '../auth/deadline.js';
import { doorRequest, fetchOf, fetchWithDeadline } from './door.js';

/** Why a call got no answer. */
export type NetworkFailure = 'timeout' | 'unreachable' | 'blocked';

/** Settings for naming a network failure. Every one has a default. */
export interface NetworkOptions {
  /** The path probed to tell `blocked` from `unreachable`. Default `/health/live`. */
  readonly probePath?: string;
  /** How long a probe's answer is kept, in milliseconds. Default 5,000. */
  readonly probeCacheMs?: number;
  /** How long the probe waits for the server, in milliseconds. Default 5,000. */
  readonly probeTimeoutMs?: number;
  /**
   * The page's origin, which a `blocked` error names. Default: the page's own
   * (`location.origin`). Without one (Node) a failure is never `blocked`.
   */
  readonly pageOrigin?: string;
}

/** How long an op call waits for the door, in milliseconds, when `callTimeoutMs` is not set. */
export const DEFAULT_CLIENT_CALL_TIMEOUT_MS = 60_000;

/** {@link NetworkOptions.probePath} when none is given. */
export const DEFAULT_PROBE_PATH = '/health/live';
/** {@link NetworkOptions.probeCacheMs} when none is given. */
export const DEFAULT_PROBE_CACHE_MS = 5_000;
/** {@link NetworkOptions.probeTimeoutMs} when none is given. */
export const DEFAULT_PROBE_TIMEOUT_MS = 5_000;

/** What a failure needs to be named: where the call went and how it was set up. */
export interface NetworkSite {
  readonly baseUrl: string;
  /** The `fetch` the call used; the probe uses it too. */
  readonly fetch?: typeof globalThis.fetch | undefined;
  readonly network?: NetworkOptions | undefined;
  /** The option that sets the wait, named by a `timeout`. */
  readonly timeoutOption: string;
  /** The wait that applied, in milliseconds. */
  readonly timeoutMs?: number | undefined;
  /** A passkey or sign-in call: a `blocked` error then names `ORDO_AUTH_ORIGINS` too. */
  readonly auth?: boolean;
}

export class OrdoNetworkError extends Error {
  readonly kind: NetworkFailure;
  /** The door the call was for. */
  readonly baseUrl: string;
  /** The page's origin, when there is one. */
  readonly pageOrigin: string | null;
  /** For `timeout`, the SDK option that sets the wait; otherwise `null`. */
  readonly option: string | null;
  /** For `blocked`, the deployment settings that add the origin; otherwise empty. */
  readonly settings: readonly string[];

  constructor(
    kind: NetworkFailure,
    detail: {
      readonly baseUrl: string;
      readonly pageOrigin: string | null;
      readonly option?: string | null;
      readonly timeoutMs?: number | undefined;
      readonly auth?: boolean;
    },
    cause?: unknown,
  ) {
    const settings =
      kind === 'blocked' ? (detail.auth ? ['ORDO_CORS_ORIGINS', 'ORDO_AUTH_ORIGINS'] : ['ORDO_CORS_ORIGINS']) : [];
    super(messageOf(kind, detail, settings), cause === undefined ? undefined : { cause });
    this.name = 'OrdoNetworkError';
    this.kind = kind;
    this.baseUrl = detail.baseUrl;
    this.pageOrigin = detail.pageOrigin;
    this.option = kind === 'timeout' ? (detail.option ?? null) : null;
    this.settings = settings;
  }
}

function messageOf(
  kind: NetworkFailure,
  d: { readonly baseUrl: string; readonly pageOrigin: string | null; readonly option?: string | null; readonly timeoutMs?: number | undefined },
  settings: readonly string[],
): string {
  switch (kind) {
    case 'timeout': {
      const wait = d.timeoutMs === undefined ? '' : ` within ${d.timeoutMs} ms`;
      const set = d.option ? ` The wait is set by the ${d.option} option.` : '';
      return `The server at ${d.baseUrl} did not answer${wait}.${set}`;
    }
    case 'unreachable':
      return `The server at ${d.baseUrl} could not be reached.`;
    case 'blocked': {
      const origin = d.pageOrigin ?? 'the page';
      const sets = settings.map((s) => `${s}=${origin}`).join(' and ');
      return (
        `The server at ${d.baseUrl} is up, and the browser refused the call because ${origin} is not allowed to call it. ` +
        `The deployment owner sets ${sets} on the server, then restarts it.`
      );
    }
  }
}

/** True for the abort a caller asked for, which is not a failure of the network. */
function isAbort(e: unknown): boolean {
  return typeof e === 'object' && e !== null && (e as { name?: unknown }).name === 'AbortError';
}

function originOf(url: string): string | null {
  try {
    return new URL(url).origin;
  } catch {
    return null;
  }
}

function pageOriginOf(options: NetworkOptions | undefined): string | null {
  if (options?.pageOrigin) return options.pageOrigin;
  const here = (globalThis as { location?: { origin?: unknown } }).location;
  return typeof here?.origin === 'string' && here.origin !== 'null' ? here.origin : null;
}

/** Recent probe answers by door origin: `true` when the server answered. */
const probes = new Map<string, { readonly at: number; readonly answered: Promise<boolean> }>();

/** Forget every kept probe answer. For tests. */
export function resetNetworkProbes(): void {
  probes.clear();
}

/** Did the server answer a `no-cors` request? An opaque answer says yes. */
function probe(site: NetworkSite, origin: string): Promise<boolean> {
  const keep = site.network?.probeCacheMs ?? DEFAULT_PROBE_CACHE_MS;
  const found = probes.get(origin);
  if (found && Date.now() - found.at < keep) return found.answered;
  const f = fetchOf(site.fetch);
  const path = site.network?.probePath ?? DEFAULT_PROBE_PATH;
  const url = `${site.baseUrl.replace(/\/+$/, '')}${path}`;
  const answered = fetchWithDeadline(
    f,
    url,
    { method: 'GET', mode: 'no-cors', cache: 'no-store', credentials: 'omit' },
    site.network?.probeTimeoutMs ?? DEFAULT_PROBE_TIMEOUT_MS,
  ).then(
    () => true,
    () => false,
  );
  probes.set(origin, { at: Date.now(), answered });
  return answered;
}

/**
 * The {@link OrdoNetworkError} for `e`, a failure of a call to `site`, or
 * `null` when `e` is not one: a caller's own abort is left alone.
 */
export async function networkError(e: unknown, site: NetworkSite): Promise<OrdoNetworkError | null> {
  if (e instanceof OrdoNetworkError) return e;
  const pageOrigin = pageOriginOf(site.network);
  const detail = { baseUrl: site.baseUrl, pageOrigin, auth: site.auth ?? false };
  if (e instanceof CallTimeout) {
    return new OrdoNetworkError('timeout', { ...detail, option: site.timeoutOption, timeoutMs: e.timeoutMs }, e);
  }
  if (isAbort(e)) return null;
  // Only a call to another origin can be refused for its origin, and only a
  // page has an origin to be refused for.
  const door = originOf(site.baseUrl);
  const crossOrigin = pageOrigin !== null && door !== null && door !== pageOrigin;
  if (crossOrigin && (await probe(site, door))) return new OrdoNetworkError('blocked', detail, e);
  return new OrdoNetworkError('unreachable', detail, e);
}

/**
 * The error for a feed that broke after it had been answering. The door was
 * reachable a moment ago and the browser's origin check had passed, so it is
 * `unreachable` and no probe is sent. A caller's own abort is `null`.
 */
export function streamError(e: unknown, site: NetworkSite): OrdoNetworkError | null {
  if (e instanceof OrdoNetworkError) return e;
  if (isAbort(e)) return null;
  return new OrdoNetworkError('unreachable', { baseUrl: site.baseUrl, pageOrigin: pageOriginOf(site.network) }, e);
}

/**
 * `fetch` with a deadline, and any failure to get an answer thrown as an
 * {@link OrdoNetworkError}. A caller's own abort is thrown as it came. One
 * attempt through the request helper (`core/door.ts`), retrying nothing.
 */
export async function fetchNamed(
  f: typeof globalThis.fetch,
  url: string,
  init: RequestInit,
  site: NetworkSite,
  signal?: AbortSignal,
): Promise<Response> {
  return doorRequest({ ...site, fetch: f }, url, signal ? { ...init, signal } : init);
}

/** What {@link namedFetch} needs: where the door is, and how a failure is named. */
export interface NamedFetchOptions extends NetworkOptions {
  /** The door's base URL, which a failure names and the probe is sent to. */
  readonly baseUrl: string;
  /** The `fetch` to wrap. Defaults to the global one. */
  readonly fetch?: typeof globalThis.fetch;
  /** How long a call waits for the door, in milliseconds. Default {@link DEFAULT_CLIENT_CALL_TIMEOUT_MS}. */
  readonly callTimeoutMs?: number;
  /** A passkey or sign-in call: a `blocked` error then names `ORDO_AUTH_ORIGINS` too. */
  readonly auth?: boolean;
}

/**
 * A `fetch` whose failures are {@link OrdoNetworkError}s, for a client that
 * takes a `fetch` and does not name network failures itself, such as the
 * `OrdoClient` of `@hominemai/ordo-react`:
 *
 *     new OrdoClient({ baseUrl, space, fetch: namedFetch({ baseUrl }) });
 *
 * The SDK's own clients and sign-in calls do this already.
 */
export function namedFetch(options: NamedFetchOptions): typeof globalThis.fetch {
  const f = fetchOf(options.fetch);
  const site: NetworkSite = {
    baseUrl: options.baseUrl,
    fetch: f,
    network: options,
    timeoutOption: 'callTimeoutMs',
    timeoutMs: options.callTimeoutMs ?? DEFAULT_CLIENT_CALL_TIMEOUT_MS,
    auth: options.auth ?? false,
  };
  return (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    return fetchNamed(f, url, init ?? {}, site, init?.signal ?? undefined);
  };
}
