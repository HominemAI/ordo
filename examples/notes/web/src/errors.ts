import { DOOR } from './config.js';
import type { Refusal } from '@hominemai/ordo-react';
import { OrdoNetworkError } from '@hominemai/ordo-sdk/core/network.js';
import { NoTenantError, PasskeyError, SecondStepNeeded, SessionEnded } from './session.js';

/** Which way in failed, so a refusal can be named in its own words. */
export type Way = 'passkey' | 'email' | 'guest' | 'outside' | 'claim';

/** What to tell a person, in the words of this page and not the wire's. */
export function friendly(e: unknown, door: string = DOOR, way: Way = 'passkey'): string {
  // The SDK says which of three it was and what fixes it, in plain words. A
  // slow door is nothing a person can fix, so that one is said short.
  if (e instanceof OrdoNetworkError) {
    return e.kind === 'timeout' ? `The deployment at ${door} took too long to answer. Try again in a moment.` : e.message;
  }
  if (e instanceof NoTenantError || e instanceof SessionEnded || e instanceof SecondStepNeeded) return e.message;
  if (e instanceof PasskeyError) {
    switch (e.failure) {
      case 'cancelled':
        return 'The passkey prompt was closed. Try again when you are ready.';
      case 'unsupported':
        return 'This browser cannot use passkeys. Try a current version of Chrome, Safari, Edge or Firefox.';
      case 'unavailable':
        return `The deployment at ${door} cannot answer that right now. Try again in a moment.`;
      case 'refused':
        return refusedText(way);
      case 'malformed':
        return 'The deployment answered something this page could not read.';
      case 'reauthentication_required':
        return 'This change needs you to sign in again first.';
      case 'guests_off':
        return `Guest sign-in is switched off on this deployment. The deployment owner turns it on with ${e.setting ?? 'ORDO_AUTH_GUEST'}=${e.value ?? 'on'}.`;
      case 'linking_off':
        return `Adding a sign-in is switched off on this deployment. The deployment owner turns it on with ${e.setting ?? 'ORDO_AUTH_LINKING'}=${e.value ?? 'on'}.`;
      case 'identifier_taken':
        return 'That sign-in already belongs to another account.';
      case 'tenant_providers_off':
        return 'This sign-in is switched off on this deployment.';
    }
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
    case 'claim':
      return 'The passkey could not be added. Start again.';
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
  switch (r.class) {
    case 'access_denied':
      return 'The deployment did not allow that.';
    case 'server_busy':
      return 'The deployment is busy. Try again in a moment.';
    case 'space_paused':
      return 'Your notes are paused for now. Nothing is lost. They work again when the deployment owner resumes them.';
    case 'space_unavailable':
      return 'Your notes cannot be reached right now. Try again in a moment.';
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
