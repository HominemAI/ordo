/**
 * Retrying a busy server, and a write that lost a race.
 *
 * Two refusals say that nothing ran and the same request can be sent again,
 * so the client sends it again on its own:
 *
 * - **`server_busy`**, a `503` with `Retry-After` in seconds: every database
 *   connection the deployment holds was in use. Up to `busyRetry.retries`
 *   times (`0` turns it off).
 * - **`tenant_not_ready`**, a `503` with `Retry-After` in seconds: the
 *   person's own tenant is still being set up (owner, 2026-10-21). It uses
 *   the `busyRetry` options, counted on its own.
 * - **`serialization_conflict`**, a `409`: the write lost a race with other
 *   writes to the same rows, after the server had already tried it again
 *   `ORDO_WRITE_RETRIES` times (owner ruling, 2026-10-19: the SDK retries a
 *   lost write race). Up to `conflictRetry.retries` times (`0` turns it off).
 *
 * The wait before retry `n` (counting from 0) is
 * `min(maxDelayMs, firstDelayMs * factor^n)`, never less than a
 * `Retry-After` the door sent (that one is itself held to `maxDelayMs`, so a
 * door cannot park a client for minutes). Jitter is on by default so many
 * clients refused together do not come back together. For `server_busy` it
 * adds up to 20%. For `serialization_conflict` the wait is a random time
 * from half the computed wait to all of it, because writers that collided on
 * one row collide again if they come back in step. Every number is a client
 * option; the defaults are {@link DEFAULT_BUSY_RETRY} and
 * {@link DEFAULT_CONFLICT_RETRY}.
 *
 * No other refusal is retried. Another 503 class (`space_unavailable`,
 * `statement_timeout`) or another 409 class (`cas_conflict`, `duplicate`)
 * means something a retry does not repair. A write is safe to send again
 * because the client mints its `Idempotency-Key` once per call and every
 * attempt carries the same one: the door replays an earlier success rather
 * than run the write twice, and a refusal like these, which applied nothing,
 * is never stored for replay, so the retry really runs.
 */

import { RETRY_CLASSES } from './refusal.js';

/** The shape both kinds of retry take. Every field is optional. */
interface RetryOptions {
  /** Retries after the first attempt. A whole number, 0 or more. */
  readonly retries?: number;
  /** The wait before the first retry, in milliseconds. 0 or more. */
  readonly firstDelayMs?: number;
  /** Each retry waits this many times the one before. 1 or more. */
  readonly factor?: number;
  /** No computed wait is longer than this, in milliseconds. 0 or more. */
  readonly maxDelayMs?: number;
  /** Spread the waits at random. */
  readonly jitter?: boolean;
}

/** How the client retries `server_busy`. Every field is optional. */
export interface BusyRetryOptions extends RetryOptions {
  /** Retries after the first attempt. A whole number, 0 or more. Default 2. */
  readonly retries?: number;
  /** The wait before the first retry, in milliseconds. 0 or more. Default 500. */
  readonly firstDelayMs?: number;
  /** Each retry waits this many times the one before. 1 or more. Default 2. */
  readonly factor?: number;
  /** No computed wait is longer than this, in milliseconds. 0 or more. Default 10000. */
  readonly maxDelayMs?: number;
  /** Add up to 20% at random to each wait. Default true. */
  readonly jitter?: boolean;
}

/** How the client retries `serialization_conflict`. Every field is optional. */
export interface ConflictRetryOptions extends RetryOptions {
  /** Retries after the first attempt. A whole number, 0 or more. Default 3. */
  readonly retries?: number;
  /** The wait before the first retry, in milliseconds. 0 or more. Default 50. */
  readonly firstDelayMs?: number;
  /** Each retry waits this many times the one before. 1 or more. Default 2. */
  readonly factor?: number;
  /** No computed wait is longer than this, in milliseconds. 0 or more. Default 1000. */
  readonly maxDelayMs?: number;
  /** Wait a random time from half the computed wait to all of it. Default true. */
  readonly jitter?: boolean;
}

/** The options after checking, with every default filled in. */
export type ResolvedBusyRetry = Required<BusyRetryOptions>;
/** The options after checking, with every default filled in. */
export type ResolvedConflictRetry = Required<ConflictRetryOptions>;

/** What the client uses for `server_busy` when its options say nothing. */
export const DEFAULT_BUSY_RETRY: ResolvedBusyRetry = Object.freeze({
  retries: 2,
  firstDelayMs: 500,
  factor: 2,
  maxDelayMs: 10_000,
  jitter: true,
});

/**
 * What the client uses for `serialization_conflict` when its options say
 * nothing. Short waits: the server has already spent its own retries, and a
 * hot row frees in milliseconds, so three more tries over about a third of
 * a second is enough for a burst and short enough that a person waiting does
 * not notice.
 */
export const DEFAULT_CONFLICT_RETRY: ResolvedConflictRetry = Object.freeze({
  retries: 3,
  firstDelayMs: 50,
  factor: 2,
  maxDelayMs: 1_000,
  jitter: true,
});

/** The most busy jitter adds, as a fraction of the wait. */
const JITTER_FRACTION = 0.2;

/** Why a call is being sent again. */
export type RetryReason = 'server_busy' | 'tenant_not_ready' | 'serialization_conflict';

/**
 * The refusal classes that mean "nothing ran; ask again shortly". The one
 * list lives with the refusals (`core/refusal.ts`), so an `OrdoRefusal` can
 * say it is `retryable`. The request helper (`core/door.ts`) retries these,
 * and the sync manager and ordo-react's live hub reconnect on them.
 */
export { RETRY_CLASSES };

/**
 * The refusals a feed reconnects on rather than stopping for good:
 * {@link RETRY_CLASSES}, and `internal` (a fault the door may recover from).
 * Every other refusal is a standing answer.
 */
export const RECONNECT_CLASSES: ReadonlySet<string> = new Set<string>(['internal', ...RETRY_CLASSES]);

/** How a feed backs off between reconnects. Every field is optional. */
export interface ReconnectBackoff {
  /** The wait before the second reconnect, in milliseconds. Default 250. */
  readonly firstDelayMs?: number;
  /** No wait is longer than this, in milliseconds. Default 30000. */
  readonly maxDelayMs?: number;
}

/**
 * The wait before reconnect number `attempts` (0 for the first try, which
 * does not wait): `firstDelayMs` doubling to `maxDelayMs`, and never less
 * than a `Retry-After` the door sent (held to `maxDelayMs`). The one backoff
 * of the SDK's sync manager, its tenant watch and ordo-react's live hub.
 */
export function reconnectDelayMs(
  attempts: number,
  retryAfter: number | null = null,
  backoff: ReconnectBackoff = {},
): number {
  const first = backoff.firstDelayMs ?? 250;
  const max = backoff.maxDelayMs ?? 30_000;
  const computed = attempts <= 0 ? 0 : Math.min(max, first * 2 ** (attempts - 1));
  const asked = retryAfter === null ? 0 : Math.min(max, Math.max(0, retryAfter));
  return Math.max(computed, asked);
}

function bad(name: string, want: string, got: unknown): never {
  throw new TypeError(`${name} must be ${want}; got ${String(got)}`);
}

/**
 * Check one retry option and its shorthand (`busyRetry` and `busyRetries`,
 * or `conflictRetry` and `conflictRetries`), and fill in the defaults.
 */
function retryOf<T extends Required<RetryOptions>>(
  name: string,
  shorthandName: string,
  defaults: T,
  given: RetryOptions | undefined,
  shorthand: number | undefined,
): T {
  if (given !== undefined && (given === null || typeof given !== 'object')) {
    bad(name, 'an object', given);
  }
  const known = new Set(Object.keys(defaults));
  for (const key of Object.keys(given ?? {})) {
    if (!known.has(key)) bad(name, `one of ${[...known].join(', ')} (got "${key}")`, key);
  }
  if (shorthand !== undefined && given?.retries !== undefined && shorthand !== given.retries) {
    bad(shorthandName, `the same as ${name}.retries when both are given (${given.retries})`, shorthand);
  }
  const r: { -readonly [K in keyof T]: T[K] } = { ...defaults, ...stripUndefined(given ?? {}) };
  if (shorthand !== undefined) r.retries = shorthand;
  if (!Number.isInteger(r.retries) || r.retries < 0) {
    bad(given?.retries === undefined ? shorthandName : `${name}.retries`, 'a whole number, 0 or more', r.retries);
  }
  if (typeof r.firstDelayMs !== 'number' || !Number.isFinite(r.firstDelayMs) || r.firstDelayMs < 0) {
    bad(`${name}.firstDelayMs`, 'a number of milliseconds, 0 or more', r.firstDelayMs);
  }
  if (typeof r.factor !== 'number' || !Number.isFinite(r.factor) || r.factor < 1) {
    bad(`${name}.factor`, 'a number, 1 or more', r.factor);
  }
  if (typeof r.maxDelayMs !== 'number' || !Number.isFinite(r.maxDelayMs) || r.maxDelayMs < 0) {
    bad(`${name}.maxDelayMs`, 'a number of milliseconds, 0 or more', r.maxDelayMs);
  }
  if (typeof r.jitter !== 'boolean') bad(`${name}.jitter`, 'true or false', r.jitter);
  return Object.freeze(r);
}

/**
 * Check the two client options that set busy retrying, `busyRetry` and its
 * shorthand `busyRetries` (the same as `busyRetry.retries`), and fill in the
 * defaults. Throws `TypeError` on a value that cannot work. Giving both with
 * different retry counts is also a `TypeError`.
 */
export function busyRetryOf(
  busyRetry: BusyRetryOptions | undefined,
  busyRetries?: number,
): ResolvedBusyRetry {
  return retryOf('busyRetry', 'busyRetries', DEFAULT_BUSY_RETRY, busyRetry, busyRetries);
}

/**
 * Check `conflictRetry` and its shorthand `conflictRetries` the same way,
 * against {@link DEFAULT_CONFLICT_RETRY}.
 */
export function conflictRetryOf(
  conflictRetry: ConflictRetryOptions | undefined,
  conflictRetries?: number,
): ResolvedConflictRetry {
  return retryOf('conflictRetry', 'conflictRetries', DEFAULT_CONFLICT_RETRY, conflictRetry, conflictRetries);
}

function stripUndefined(o: RetryOptions): RetryOptions {
  return Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined));
}

/**
 * The retryable class of this response, when it is a `503 server_busy`, a
 * `503 tenant_not_ready` or a
 * `409 serialization_conflict`. Leaves the body unread.
 */
export async function retryableClass(response: Response): Promise<RetryReason | null> {
  if (response.status !== 503 && response.status !== 409) return null;
  try {
    const body = JSON.parse(await response.clone().text()) as {
      code?: unknown;
      error?: { code?: unknown };
    } | null;
    const code = body?.error?.code ?? body?.code;
    if (response.status === 503 && code === 'server_busy') return 'server_busy';
    if (response.status === 503 && code === 'tenant_not_ready') return 'tenant_not_ready';
    if (response.status === 409 && code === 'serialization_conflict') return 'serialization_conflict';
    return null;
  } catch {
    return null;
  }
}

/** The `Retry-After` of a response in milliseconds, or `null` when absent or unusable. */
export function retryAfterMs(response: Response): number | null {
  const raw = response.headers.get('Retry-After');
  if (raw === null || !/^\s*\d+(\.\d+)?\s*$/.test(raw)) return null;
  return Number(raw) * 1000;
}

/** The wait before retry `attempt` (from 0), before jitter. */
function baseDelayMs(retry: Required<RetryOptions>, attempt: number, retryAfter: number | null): number {
  const computed = Math.min(retry.maxDelayMs, retry.firstDelayMs * retry.factor ** attempt);
  const asked = retryAfter === null ? 0 : Math.min(retry.maxDelayMs, Math.max(0, retryAfter));
  return Math.max(computed, asked);
}

/**
 * How long to wait before busy retry number `attempt` (from 0), in
 * milliseconds. `random` is a source of numbers in [0, 1), for tests.
 */
export function busyDelayMs(
  retry: ResolvedBusyRetry,
  attempt: number,
  retryAfter: number | null,
  random: () => number = Math.random,
): number {
  const base = baseDelayMs(retry, attempt, retryAfter);
  return retry.jitter ? base * (1 + JITTER_FRACTION * random()) : base;
}

/**
 * How long to wait before conflict retry number `attempt` (from 0), in
 * milliseconds: with jitter, a random time from half the wait to all of it.
 */
export function conflictDelayMs(
  retry: ResolvedConflictRetry,
  attempt: number,
  retryAfter: number | null,
  random: () => number = Math.random,
): number {
  const base = baseDelayMs(retry, attempt, retryAfter);
  return retry.jitter ? base * (0.5 + 0.5 * random()) : base;
}

/** Told before each wait: the retry's number for its reason (from 1), the wait, and the reason. */
export type OnRetry = (attempt: number, delayMs: number, reason: RetryReason) => void;

/**
 * The one retry loop is in the request helper (`core/door.ts`); it is
 * re-exported here, where it always was.
 */
export { fetchRetrying } from './door.js';
