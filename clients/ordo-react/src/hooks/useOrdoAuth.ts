/**
 * `useOrdoAuth`: the page's sign-in state, over the SDK's session object
 * (`createOrdoAuth`).
 *
 * ```tsx
 * const auth = createOrdoAuth({ baseUrl: DOOR }); // once, at module level
 *
 * function App() {
 *   const { phase, session, retry } = useOrdoAuth(auth);
 *   if (phase === 'checking') return <Splash />;
 *   if (phase === 'failed') return <button onClick={retry}>Try again</button>;
 *   if (session === null) return <SignIn auth={auth} />; // auth.completeEmail(...)
 *   return <TenantGate baseUrl={DOOR} token={auth.token}>{(space) => ...}</TenantGate>;
 * }
 * ```
 *
 * It resumes once per mount and follows every change the object makes: a
 * sign-in, a renewal, a sign-out, a session that ended. The page never sets
 * the session itself.
 */

import { useEffect } from 'react';
import type { OrdoAuth } from '@hominemai/ordo-sdk/auth/session.js';
import type { PasskeySession } from '@hominemai/ordo-sdk/auth/passkey.js';
import { useResumedSession, type ResumePhase } from './useResumedSession.js';

export interface OrdoAuthState {
  /** `checking`, `signed-in`, `signed-out`, or `failed` (the door could not be asked: `retry`). */
  readonly phase: ResumePhase;
  readonly session: PasskeySession | null;
  /** What went wrong, when `phase` is `failed`. */
  readonly error: unknown;
  /** Ask the door again after `failed`. */
  readonly retry: () => void;
  /** The object, for its verbs: `completeEmail`, `signOut`, `token`, `spaces`. */
  readonly auth: OrdoAuth;
}

export function useOrdoAuth(auth: OrdoAuth): OrdoAuthState {
  const resumed = useResumedSession(() => auth.resume());
  const { set } = resumed;
  useEffect(() => auth.subscribe(set), [auth, set]);
  return { phase: resumed.phase, session: resumed.session, error: resumed.error, retry: resumed.retry, auth };
}
