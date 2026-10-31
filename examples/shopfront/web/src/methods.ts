/**
 * Which ways in the login page offers: asked of the door at start.
 *
 * The door lists its own methods (`GET /auth/methods`), so the page needs no
 * build word to match the deployment's settings. If the door cannot answer
 * (it is down, or an older one without the route), the page falls back to
 * `VITE_ORDO_SIGN_IN` (`config.ts`), and with it unset offers the passkey
 * alone, as before. This page offers `passkey` and `email` only.
 */

import type { PasskeyEndpoint } from '@hominemai/ordo-sdk/auth/passkey.js';
import { signInMethods as askDoor } from '@hominemai/ordo-sdk/auth/methods.js';
import { SIGN_IN, type SignInMethods } from './config.js';

/** Ask the door. Never rejects: an unreachable door gives `fallback`. */
export async function loadMethods(
  endpoint: PasskeyEndpoint,
  fallback: SignInMethods = SIGN_IN,
): Promise<SignInMethods> {
  try {
    const got = await askDoor(endpoint);
    return { passkey: got.passkey, email: got.email };
  } catch {
    return fallback;
  }
}
