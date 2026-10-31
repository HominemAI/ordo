/**
 * Staying signed in across a reload (owner ruling 2026-10-21).
 *
 * Every sign-in sets a renewal cookie the page cannot read. On load, the
 * page calls {@link resumeSession} once. The browser sends the cookie to
 * `POST /auth/refresh`, and the door answers a fresh token, which the page
 * keeps in memory as it does any other. `null` means there is nothing to
 * resume: show the sign-in screen.
 *
 *     const done = await resumeSession({ baseUrl });
 *     if (done === null) showSignIn();
 *
 * `null` also covers a page and a door on different sites. The browser does
 * not send a `SameSite=Strict` cookie across sites, so such a page asks for
 * a sign-in on every reload, as it did before.
 */

import { PasskeyError, session, type PasskeyEndpoint, type PasskeySession, type Wire } from './passkey.js';
import { authRequest } from './request.js';

/** The lock two tabs of one page share, so they do not spend one cookie at once. */
export const RESUME_LOCK = 'ordo-resume';

/** Why there was nothing to resume, when the door said. */
export type ResumeMiss =
  /** No cookie, or one the door no longer honours. */
  | 'none'
  /** The deployment turned it off (`ORDO_REMEMBER=off`). */
  | 'remember_off';

export interface ResumeOptions {
  /** Told why there was nothing to resume. For a page that wants to say so. */
  readonly onMiss?: (why: ResumeMiss, message?: string) => void;
}

/**
 * Continue the sign-in this browser already holds, or `null` when it holds
 * none. Throws `OrdoNetworkError` when the door cannot be reached, and a
 * `PasskeyError` of kind `unavailable` when it cannot check right now: in
 * both cases the person may still be signed in, so do not show sign-in as if
 * they were not.
 */
export async function resumeSession(
  endpoint: PasskeyEndpoint,
  options: ResumeOptions = {},
): Promise<PasskeySession | null> {
  // Tabs opened together each send the same cookie. Taking turns means the
  // second sends the cookie the first was given. The door also forgives a
  // second use within a few seconds, for browsers without locks.
  const locks = typeof navigator !== 'undefined' ? navigator.locks : undefined;
  if (locks && typeof locks.request === 'function') {
    return locks.request(RESUME_LOCK, () => once(endpoint, options));
  }
  return once(endpoint, options);
}

async function once(endpoint: PasskeyEndpoint, options: ResumeOptions): Promise<PasskeySession | null> {
  const response = await authRequest(endpoint, '/auth/refresh', {
    method: 'POST',
    headers: { Accept: 'application/json' },
    credentials: 'include',
  });
  let json: unknown = null;
  try {
    json = await response.json();
  } catch {
    // An empty body: nothing to read.
  }
  if (response.status === 401 || response.status === 403) {
    const error = json && typeof json === 'object' ? ((json as Wire)['error'] as Wire | undefined) : undefined;
    if (error?.['code'] === 'remember_off') {
      const message = typeof error['message'] === 'string' ? error['message'] : undefined;
      options.onMiss?.('remember_off', message);
    } else {
      options.onMiss?.('none');
    }
    return null;
  }
  if (response.status === 503) {
    throw new PasskeyError('unavailable', 'the door cannot check this sign-in right now');
  }
  if (!response.ok || json === null || typeof json !== 'object') {
    throw new PasskeyError('malformed', 'the door answered a resume this client cannot read');
  }
  return session(json as Wire);
}
