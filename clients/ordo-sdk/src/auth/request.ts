/**
 * The one way every sign-in and account call reaches the door (owner ruling
 * 2026-10-23: busy versus broken "through some interface rather than
 * individual spots").
 *
 * {@link authRequest} sends a call, sends it again while the door answers
 * `server_busy` (a `503` with `Retry-After`: nothing ran) under the
 * endpoint's `busyRetry`, names a call that got no answer
 * (`OrdoNetworkError`), and throws {@link ServerBusyError} when the door was
 * still busy after the last retry. Any other answer, a broken store's `503`
 * included, comes back for the module to read.
 *
 * It is the SDK's one request helper (`core/door.ts`, {@link doorRequest})
 * with the sign-in defaults: a sign-in deadline, busy retry and no conflict
 * retry. No file in `src/auth/` calls `fetch` itself;
 * `test/one-door.test.ts` holds them to it.
 */

import { busyRetryOf, retryableClass, retryAfterMs, type BusyRetryOptions } from '../core/busy.js';
import { busyOnly, doorRequest, doorUrl } from '../core/door.js';
import type { NetworkOptions, NetworkSite } from '../core/network.js';
import { OrdoRefusal } from '../core/refusal.js';

/** Where the door is and how to reach it: the shape every auth endpoint has. */
export interface AuthEndpoint {
  /** Door base URL. The route is appended. */
  readonly baseUrl: string;
  /** For tests and for runtimes without a global `fetch`. */
  readonly fetch?: typeof globalThis.fetch | undefined;
  /** How long each call waits for the door. Default `DEFAULT_CALL_TIMEOUT_MS`. */
  readonly callTimeoutMs?: number | undefined;
  /** How a call that got no answer is named (`OrdoNetworkError`). */
  readonly network?: NetworkOptions | undefined;
  /**
   * How a call is sent again when the door answers `server_busy`, as
   * `OrdoClient` does for ops. Same options and defaults as
   * `OrdoClientOptions.busyRetry`; `retries: 0` turns it off.
   */
  readonly busyRetry?: BusyRetryOptions | undefined;
}

/**
 * The door stayed busy (`server_busy`) through every retry `busyRetry`
 * allowed. Nothing ran: the same call can be made again. Distinct from a
 * store that could not answer, which each module reports as its own
 * `unavailable`.
 */
export class ServerBusyError extends OrdoRefusal<'server_busy'> {
  readonly failure = 'busy' as const;
  constructor(retryAfter: number | null) {
    const message = 'the server is busy: nothing ran, and the same call can be made again shortly';
    super(
      { class: 'server_busy', message, op: null, param: null, status: 503, declared: false },
      retryAfter,
      { message },
    );
    this.name = 'ServerBusyError';
  }
}

/**
 * Where a call went, for naming a failure to get an answer. `auth` is a
 * sign-in route (a `blocked` error then names `ORDO_AUTH_ORIGINS` too).
 */
export function siteOf(endpoint: AuthEndpoint, auth = true): NetworkSite {
  return {
    baseUrl: endpoint.baseUrl,
    fetch: endpoint.fetch,
    network: endpoint.network,
    timeoutOption: 'callTimeoutMs',
    timeoutMs: endpoint.callTimeoutMs,
    auth,
  };
}

/** What a call adds to its endpoint. */
export interface AuthRequestOptions {
  /** A sign-in route (`/auth/...`) rather than an op. Default `true`. */
  readonly auth?: boolean;
  readonly signal?: AbortSignal;
}

/**
 * One call to the door at `path` (appended to `baseUrl`). See the module
 * doc. Throws {@link ServerBusyError} when busy outlasted the retries.
 */
export async function authRequest(
  endpoint: AuthEndpoint,
  path: string,
  init: RequestInit,
  options: AuthRequestOptions = {},
): Promise<Response> {
  // Only `server_busy` (and `tenant_not_ready`, the same "nothing ran") is
  // sent again here; a conflict is the caller's to read.
  const response = await doorRequest(
    siteOf(endpoint, options.auth ?? true),
    doorUrl(endpoint.baseUrl, path),
    options.signal ? { ...init, signal: options.signal } : init,
    { retry: busyOnly(busyRetryOf(endpoint.busyRetry)) },
  );
  if ((await retryableClass(response)) === 'server_busy') {
    throw new ServerBusyError(retryAfterMs(response));
  }
  return response;
}
