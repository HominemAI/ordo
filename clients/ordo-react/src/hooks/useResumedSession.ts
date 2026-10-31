/**
 * `useResumedSession`: start signed in when the browser still holds a
 * sign-in (owner ruling 2026-10-21).
 *
 * The page passes the SDK's `resumeSession`, bound to its door:
 *
 * ```tsx
 * const resumed = useResumedSession(() => resumeSession({ baseUrl }));
 * if (resumed.phase === 'checking') return <Splash />;
 * if (resumed.session === null) return <SignIn onDone={resumed.set} />;
 * return <OrdoProvider client={clientFor(resumed.session)}>...</OrdoProvider>;
 * ```
 *
 * The provider then starts with a client that is already signed in, and the
 * sign-in screen shows only when there is nothing to resume. The call runs
 * once per mount (and again on `retry`). It is passed in, so any resume
 * works, and any value it answers is kept as the session.
 *
 * Most pages want {@link useOrdoAuth} instead: it runs this over the SDK's
 * `createOrdoAuth`, which also renews the token and signs out.
 */

import { useCallback, useEffect, useRef, useState } from 'react';

/** Where resuming stands. */
export type ResumePhase =
  /** The door has not answered yet. Show nothing, or a splash. */
  | 'checking'
  /** A session: resumed, or set by the page after a sign-in. */
  | 'signed-in'
  /** Nothing to resume. Show the sign-in screen. */
  | 'signed-out'
  /** The door could not be asked. The person may still be signed in: `retry()`. */
  | 'failed';

export interface ResumedSession<S> {
  readonly phase: ResumePhase;
  /** The session, or `null`. */
  readonly session: S | null;
  /** What went wrong, when `phase` is `failed`. */
  readonly error: unknown;
  /** Ask the door again. */
  readonly retry: () => void;
  /** Put a session in place after a sign-in, or `null` after a sign-out. */
  readonly set: (session: S | null) => void;
}

export function useResumedSession<S>(resume: () => Promise<S | null>): ResumedSession<S> {
  const [phase, setPhase] = useState<ResumePhase>('checking');
  const [session, setSession] = useState<S | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [round, setRound] = useState(0);
  // The call is read once per round, so a page that passes a new arrow on
  // every render does not ask the door on every render.
  const latest = useRef(resume);
  latest.current = resume;
  // A sign-in the page set while the door was still answering wins.
  const settled = useRef(false);

  useEffect(() => {
    let live = true;
    settled.current = false;
    setPhase('checking');
    latest
      .current()
      .then((s) => {
        if (!live || settled.current) return;
        setSession(s);
        setError(null);
        setPhase(s === null ? 'signed-out' : 'signed-in');
      })
      .catch((e: unknown) => {
        if (!live || settled.current) return;
        setError(e);
        setPhase('failed');
      });
    return () => {
      live = false;
    };
  }, [round]);

  const retry = useCallback(() => setRound((r) => r + 1), []);
  const set = useCallback((s: S | null) => {
    settled.current = true;
    setSession(s);
    setError(null);
    setPhase(s === null ? 'signed-out' : 'signed-in');
  }, []);

  return { phase, session, error, retry, set };
}
