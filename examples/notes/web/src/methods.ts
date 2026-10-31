/**
 * Which ways in this page offers: asked of the door at start.
 *
 * The door lists its own methods (`GET /auth/methods`), so the page needs no
 * build word to match the deployment's settings. If the door cannot answer
 * (it is down, or an older one without the route), the page falls back to
 * `VITE_ORDO_SIGN_IN` and `VITE_TURNSTILE_SITE_KEY` (`config.ts`), and with
 * neither set it offers the passkey alone, as before.
 */

import type { PasskeyEndpoint } from '@hominemai/ordo-sdk/auth/passkey.js';
import { signInMethods as askDoor } from '@hominemai/ordo-sdk/auth/methods.js';
import { SIGN_IN, TURNSTILE_SITE_KEY, type SignInMethods } from './config.js';

/** What the sign-in screen is built from. */
export interface Offer {
  readonly methods: SignInMethods;
  /** The Turnstile site key, or null when the human check is off. */
  readonly siteKey: string | null;
  /** Where this came from: the door, or the build variables because the door did not answer. */
  readonly source: 'door' | 'build';
}

/** What the build variables say, for a door that cannot be asked. */
export const FALLBACK: Offer = { methods: SIGN_IN, siteKey: TURNSTILE_SITE_KEY, source: 'build' };

/** Ask the door. Never rejects: an unreachable door gives `fallback`. */
export async function loadOffer(
  endpoint: PasskeyEndpoint,
  fallback: Offer = FALLBACK,
  tenant?: string,
): Promise<Offer> {
  try {
    const got = await askDoor(endpoint, tenant === undefined ? {} : { tenant });
    return {
      methods: {
        passkey: got.passkey,
        email: got.email,
        guest: got.guest,
        connections: got.connections.map((c) => ({ name: c.name, label: c.label })),
      },
      // A door with the check on and no public key set still works if the
      // build has one.
      siteKey: got.humanCheck === null ? null : (got.humanCheck.siteKey ?? fallback.siteKey),
      source: 'door',
    };
  } catch {
    return fallback;
  }
}
