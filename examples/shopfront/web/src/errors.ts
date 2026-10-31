import { OrdoRefusal } from '@hominemai/ordo-sdk/core/refusal.js';
import { PasskeyError } from '@hominemai/ordo-sdk/auth/passkey.js';
import { DOOR, originHint } from './config.js';
import { SessionEnded } from './session.js';

/** Which way in failed, so a refusal can be named in its own words. */
export type Way = 'passkey' | 'email' | 'handoff';

const unreachable = (origin: string, door: string) =>
  `This page cannot reach the deployment at ${door}. If it is running, the app developer has to set ` +
  `${originHint(origin)} on it, then restart it.`;

/** What to tell a person about a refusal from an op, in the shop's words. */
export function refusalText(e: OrdoRefusal): string {
  switch (e.class) {
    case 'access_denied':
      return 'The shop did not allow that.';
    case 'access_guard':
      return 'Only the shop staff can do that.';
    case 'not_found':
      return 'That was not found, or you may not see it.';
    case 'serialization_conflict':
    case 'idempotency_in_flight':
      return 'Someone else changed this at the same moment. Reload and try again.';
    case 'server_busy':
      return 'The shop is busy. Try again in a moment.';
    case 'invalid_param':
    case 'missing_param':
      return `That was not accepted: ${e.detail.message}`;
    default:
      return e.detail.message;
  }
}

/** What to tell a person, in the words of this page and not the wire's. */
export function friendly(e: unknown, origin: string, door: string = DOOR, way: Way = 'passkey'): string {
  if (e instanceof SessionEnded) return e.message;
  if (e instanceof OrdoRefusal) return refusalText(e);
  if (e instanceof PasskeyError) {
    switch (e.failure) {
      case 'cancelled':
        return 'The passkey prompt was closed. Try again when you are ready.';
      case 'unsupported':
        return 'This browser cannot use passkeys. Try a current version of Chrome, Safari, Edge or Firefox.';
      case 'unavailable':
        return unreachable(origin, door);
      case 'refused':
        return refusedText(way);
      case 'malformed':
        return 'The deployment answered something this page could not read.';
      case 'reauthentication_required':
        return 'This change needs you to sign in again first.';
      default:
        return e.message;
    }
  }
  if (e instanceof TypeError) {
    // A browser reports a blocked cross-origin call as a failed fetch and says nothing else.
    return unreachable(origin, door);
  }
  return e instanceof Error ? e.message : 'Something went wrong.';
}

function refusedText(way: Way): string {
  switch (way) {
    case 'passkey':
      return 'That passkey was not accepted. If you have no account yet, type your name and use the sign up button.';
    case 'email':
      return 'That code was not accepted. It may be wrong, used or too old. Ask for a new code.';
    case 'handoff':
      return 'The sign-in could not be passed on. Start again from the shop.';
  }
}
