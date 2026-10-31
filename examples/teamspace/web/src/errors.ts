import { DOOR, originHint } from './config.js';
import type { Refusal } from '@hominemai/ordo-react';
import { NoTeamError, PasskeyError, SecondStepNeeded, SessionEnded } from './session.js';

/** Which way in failed, so a refusal can be named in its own words. */
export type Way = 'passkey' | 'email' | 'guest' | 'outside';

const unreachable = (origin: string, door: string) =>
  `This page cannot reach the deployment at ${door}. If it is running, the deployment owner has to set ` +
  `${originHint(origin)} on it, then restart it.`;

/** What to tell a person, in the words of this page and not the wire's. */
export function friendly(e: unknown, origin: string, door: string = DOOR, way: Way = 'passkey'): string {
  if (e instanceof NoTeamError || e instanceof SessionEnded || e instanceof SecondStepNeeded) return e.message;
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
      case 'guests_off':
        return 'Guest sign-in is switched off on this deployment.';
      case 'identifier_taken':
        return 'That sign-in already belongs to another account.';
      default:
        return 'That sign-in did not work. Try again.';
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
    case 'guest':
      return 'A guest account could not be started. Try again in a while.';
    case 'outside':
      return 'That sign-in was not accepted. Try again.';
  }
}

/** What to say when an outside provider sent the person back with `?error=`. */
export function returnedErrorText(error: string): string {
  if (error === 'identity_provider_unavailable') {
    return 'The sign-in provider could not be reached. Try again later.';
  }
  return 'That sign-in did not work. Try again.';
}

/** What to tell a person about a refusal a hook reports, chosen by its class. */
export function refusalText(r: Refusal): string {
  const cls: string = r.class;
  switch (cls) {
    case 'cas_miss':
      return 'Changed by someone else, reloaded.';
    case 'access_denied':
      return 'You are not allowed to do that.';
    case 'tenant_refused':
      return `The team refused that: ${r.message}`;
    case 'server_busy':
      return 'The deployment is busy. Try again in a moment.';
    case 'space_paused':
      return 'This team is paused for now. Nothing is lost. It works again when the deployment owner resumes it.';
    case 'space_unavailable':
      return 'This team cannot be reached right now. Try again in a moment.';
    case 'quota_exceeded':
      return 'The deployment has reached a limit. Try again later.';
    case 'bound_violation':
      return 'That is too long or too large to save.';
    case 'invalid_param':
    case 'missing_param':
      return `That was not accepted: ${r.message}`;
    default:
      return r.message;
  }
}
