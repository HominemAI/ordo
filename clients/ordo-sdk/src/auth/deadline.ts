/**
 * A deadline for one call to the door. A sign-in call that the door never
 * answers would otherwise hang the page forever.
 */

/** How long a sign-in call waits for the door, in milliseconds. */
export const DEFAULT_CALL_TIMEOUT_MS = 15_000;

/** The door did not answer within the deadline. Each module maps it to its own `timeout`. */
export class CallTimeout extends Error {
  constructor(readonly timeoutMs: number) {
    super(`the door did not answer in time (${timeoutMs} ms)`);
    this.name = 'CallTimeout';
  }
}

/**
 * `fetch` with a deadline. It lives with the one request helper
 * (`core/door.ts`) and is re-exported here, where it always was.
 */
export { fetchWithDeadline } from '../core/door.js';

/** The message for a failed call: a missed deadline says so. */
export function unreachableMessage(e: unknown): string {
  return e instanceof CallTimeout ? 'the door did not answer in time' : 'the door could not be reached';
}
