/**
 * The one way the SDK reaches the door (`docs/audit-service-interfaces.md`,
 * fix order step 7).
 *
 * {@link doorRequest} sends every call: ops, sync and live feeds, files,
 * sign-in and account calls, the person's own download, links. It
 *
 * - waits a deadline for each attempt (`callTimeoutMs`; none for a file),
 * - names a call that got no answer (`OrdoNetworkError`: `timeout`,
 *   `unreachable` or `blocked`),
 * - sends the call again under one retry policy: `server_busy` and
 *   `tenant_not_ready` under `busyRetry`, `serialization_conflict` under
 *   `conflictRetry`, each bounded, each honouring the door's `Retry-After`
 *   (held to `maxDelayMs`). The classes are `RETRY_CLASSES`.
 *
 * {@link refusalOf} turns a non-2xx answer into the one error shape,
 * `OrdoRefusal`, with the door's `Retry-After` on it.
 *
 * No other file in ordo-sdk or ordo-react calls `fetch`, and no other file
 * holds a retry loop: `test/one-door.test.ts` (both packages) holds them to
 * it. The one exception is a file upload with progress, which needs
 * `XMLHttpRequest` (`core/files.ts`).
 */

import { CallTimeout, DEFAULT_CALL_TIMEOUT_MS } from '../auth/deadline.js';
import {
  busyDelayMs,
  busyRetryOf,
  conflictDelayMs,
  retryableClass,
  retryAfterMs,
  type OnRetry,
  type ResolvedBusyRetry,
  type ResolvedConflictRetry,
} from './busy.js';
import { networkError, type NetworkSite } from './network.js';
import { OrdoRefusal, decodeRefusal } from './refusal.js';

/** The `fetch` a client is given, or the global one. */
export type Fetch = typeof globalThis.fetch;

/** The `fetch` to use: the one given, or the global one bound to it. */
export function fetchOf(f?: Fetch | undefined): Fetch {
  return f ?? globalThis.fetch.bind(globalThis);
}

/** The door's base URL with `path` appended. */
export function doorUrl(baseUrl: string, path: string): string {
  return `${baseUrl.replace(/\/+$/, '')}${path}`;
}

/**
 * `fetch` with a deadline, combined with the caller's own signal if given.
 * Rejects with {@link CallTimeout} at the deadline, and with the fetch's own
 * error (or the abort reason) otherwise. Rejects at the deadline even when
 * the fetch ignores its signal.
 */
export function fetchWithDeadline(
  f: Fetch,
  url: string,
  init: RequestInit,
  timeoutMs: number = DEFAULT_CALL_TIMEOUT_MS,
  signal?: AbortSignal,
): Promise<Response> {
  const ctl = new AbortController();
  return new Promise<Response>((resolve, reject) => {
    const onCaller = () => {
      clearTimeout(timer);
      ctl.abort();
      reject(signal?.reason ?? new Error('aborted'));
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onCaller);
      ctl.abort();
      reject(new CallTimeout(timeoutMs));
    }, timeoutMs);
    if (signal?.aborted) {
      onCaller();
      return;
    }
    signal?.addEventListener('abort', onCaller, { once: true });
    const done = () => {
      clearTimeout(timer);
      signal?.removeEventListener('abort', onCaller);
      // The answer is in, but its body may be a stream the caller still ends
      // by aborting, so the abort keeps reaching the request.
      signal?.addEventListener('abort', () => ctl.abort(signal.reason), { once: true });
    };
    f(url, { ...init, signal: ctl.signal }).then(
      (r) => {
        done();
        resolve(r);
      },
      (e: unknown) => {
        done();
        reject(e);
      },
    );
  });
}

function pause(ms: number, signal: AbortSignal | null | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(signal.reason);
    const onAbort = (): void => {
      clearTimeout(timer);
      reject(signal?.reason);
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}

/**
 * `send`, and again on `server_busy` and on `tenant_not_ready` up to
 * `busy.retries` times each, and on `serialization_conflict` up to
 * `conflict.retries` times, each counted on its own. The last refusal is
 * returned when the retries run out, so the caller turns it into the
 * ordinary refusal. Every attempt sends the same `init`, so a write carries
 * the same `Idempotency-Key` each time. The SDK's one retry loop.
 */
export async function fetchRetrying(
  send: Fetch,
  url: string,
  init: RequestInit,
  busy: ResolvedBusyRetry,
  conflict: ResolvedConflictRetry,
  /** Told before each wait (L7). */
  onRetry?: OnRetry,
): Promise<Response> {
  let busyN = 0;
  let notReadyN = 0;
  let conflictN = 0;
  for (;;) {
    const response = await send(url, init);
    const reason = await retryableClass(response);
    let delay: number;
    let attempt: number;
    if (reason === 'server_busy' && busyN < busy.retries) {
      delay = busyDelayMs(busy, busyN, retryAfterMs(response));
      attempt = ++busyN;
    } else if (reason === 'tenant_not_ready' && notReadyN < busy.retries) {
      delay = busyDelayMs(busy, notReadyN, retryAfterMs(response));
      attempt = ++notReadyN;
    } else if (reason === 'serialization_conflict' && conflictN < conflict.retries) {
      delay = conflictDelayMs(conflict, conflictN, retryAfterMs(response));
      attempt = ++conflictN;
    } else {
      return response;
    }
    onRetry?.(attempt, delay, reason);
    await pause(delay, init.signal);
  }
}

/** Which refusals a call is sent again on, and how. */
export interface RetryPolicy {
  readonly busy: ResolvedBusyRetry;
  readonly conflict: ResolvedConflictRetry;
}

/**
 * Send once; retry nothing. Spelled out rather than built with
 * `busyRetryOf`, because `busy.ts` re-exports this module and a call at
 * load time would run before `busy.ts` has its defaults.
 */
const NEVER = Object.freeze({ retries: 0, firstDelayMs: 0, factor: 1, maxDelayMs: 0, jitter: false });
export const NO_RETRY: RetryPolicy = Object.freeze({ busy: NEVER, conflict: NEVER });

/**
 * Retry `server_busy` and `tenant_not_ready` under `busy`, and no
 * conflict: a call that is not an idempotent op write leaves a conflict to
 * its caller.
 */
export function busyOnly(busy: ResolvedBusyRetry = busyRetryOf(undefined)): RetryPolicy {
  return { busy, conflict: NO_RETRY.conflict };
}

/** How one call is sent. */
export interface DoorRequestOptions {
  /** Which refusals to send again on. Default {@link NO_RETRY}. */
  readonly retry?: RetryPolicy | undefined;
  /** Told before each wait. */
  readonly onRetry?: OnRetry | undefined;
  /**
   * The deadline for each attempt, in milliseconds; `null` for none (a file
   * transfer). Default the site's `timeoutMs`, else
   * `DEFAULT_CALL_TIMEOUT_MS`.
   */
  readonly timeoutMs?: number | null | undefined;
  /**
   * Name a call that got no answer as an `OrdoNetworkError`. Default true.
   * `false` throws the failure as it came, for a caller with its own naming
   * (a hand-built `FileDoor`'s `wrap`).
   */
  readonly named?: boolean | undefined;
}

/**
 * One call to the door at `url`. See the module doc. Resolves with the
 * answer, a refusal included (read it with {@link refusalOf}); rejects with
 * an `OrdoNetworkError` when no answer came, and with the caller's own
 * abort as it came.
 */
export async function doorRequest(
  site: NetworkSite,
  url: string,
  init: RequestInit,
  options: DoorRequestOptions = {},
): Promise<Response> {
  const f = fetchOf(site.fetch);
  const timeout = options.timeoutMs === undefined ? (site.timeoutMs ?? DEFAULT_CALL_TIMEOUT_MS) : options.timeoutMs;
  const once = async (u: string, i: RequestInit): Promise<Response> => {
    try {
      return timeout === null ? await f(u, i) : await fetchWithDeadline(f, u, i, timeout, i.signal ?? undefined);
    } catch (e) {
      if (options.named === false) throw e;
      throw (await networkError(e, site)) ?? e;
    }
  };
  const retry = options.retry ?? NO_RETRY;
  return fetchRetrying(
    ((u: string, i?: RequestInit) => once(u, i ?? {})) as Fetch,
    url,
    init,
    retry.busy,
    retry.conflict,
    options.onRetry,
  );
}

/** A response's body as JSON, its text when it is not JSON, or `null` when empty. */
export async function readBody(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) return null;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

/**
 * The {@link OrdoRefusal} for a non-2xx answer, with the door's
 * `Retry-After`. Reads the body. `op` names the call when the envelope does
 * not; `declared` is the op's own refusal list.
 */
export async function refusalOf(
  response: Response,
  op: string,
  declared: readonly string[] = [],
): Promise<OrdoRefusal> {
  return new OrdoRefusal(decodeRefusal(await readBody(response), response.status, op, declared), retryAfterMs(response));
}
