/**
 * Retrying a busy server, and a write that lost a race.
 *
 * The code lives in `@hominemai/ordo-sdk` (`src/core/busy.ts`) and its header
 * there is the description. This package re-exports it (owner ruling,
 * 2026-10-23: ordo-react imports ordo-sdk's code and carries no copy).
 */
export {
  DEFAULT_BUSY_RETRY,
  DEFAULT_CONFLICT_RETRY,
  busyDelayMs,
  busyRetryOf,
  conflictDelayMs,
  conflictRetryOf,
  fetchRetrying,
  retryAfterMs,
  retryableClass,
  type BusyRetryOptions,
  type ConflictRetryOptions,
  type OnRetry,
  type ResolvedBusyRetry,
  type ResolvedConflictRetry,
  type RetryReason,
} from '@hominemai/ordo-sdk/core/busy.js';
