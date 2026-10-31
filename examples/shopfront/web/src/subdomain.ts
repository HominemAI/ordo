/**
 * The shop and the console sign in the same way: the login page signs the
 * person in and hands a one-time code back (`docs/howto-sign-in.md`, section
 * 11). This file is that, once, for both pages.
 */

import {
  finishSubdomainSignIn,
  startSubdomainSignIn,
  type HandoffStorage,
} from '@hominemai/ordo-sdk/auth/handoff.js';
import { useCallback, useEffect, useState } from 'react';
import { DOOR, ORIGINS, SPACE } from './config.js';
import { friendly } from './errors.js';
import { Session, sessionStore, type Store } from './session.js';

/**
 * The login page address to send the person to. It carries `return_to` (this
 * page) and `binder_hash`; the binder stays in `storage`.
 */
export function loginAddress(coreUrl: string, returnTo: string, storage: HandoffStorage): Promise<string> {
  return startSubdomainSignIn({ coreUrl, returnTo, storage });
}

/** The options one page's session is built with. */
export function sessionOptions(key: string, store: Store | undefined = sessionStore()) {
  return { key, baseUrl: DOOR, space: SPACE, store } as const;
}

/**
 * React runs an effect twice in development. A hand-off code works once, so
 * the redeem runs once per page load and both runs share its answer.
 */
let redeeming: Promise<Session | null> | null = null;

function redeemOnce(key: string): Promise<Session | null> {
  redeeming ??= finishSubdomainSignIn({ baseUrl: DOOR }).then((done) =>
    done === null ? null : Session.from(sessionOptions(key), done),
  );
  return redeeming;
}

export interface Subdomain {
  /** The signed-in session, or null. */
  readonly session: Session | null;
  /** True until the first look at storage and the address is done. */
  readonly starting: boolean;
  readonly error: string | null;
  readonly busy: boolean;
  /** Send the person to the login page. */
  readonly signIn: () => void;
  /** Forget the session in this tab. */
  readonly forget: () => void;
}

/** A page's session: restored from this tab, or redeemed from a hand-off code in the address. */
export function useSubdomain(key: string): Subdomain {
  const [session, setSession] = useState<Session | null>(null);
  const [starting, setStarting] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    void (async () => {
      try {
        const redeemed = await redeemOnce(key);
        if (live) setSession(redeemed ?? Session.restore(sessionOptions(key)));
      } catch (e) {
        if (live) {
          setError(friendly(e, window.location.origin, DOOR, 'handoff'));
          setSession(Session.restore(sessionOptions(key)));
        }
      } finally {
        redeeming = null;
        if (live) setStarting(false);
      }
    })();
    return () => {
      live = false;
    };
  }, [key]);

  // When the session ends (renewal refused, forgotten), the sign-in button comes back.
  useEffect(() => session?.onEnd(() => setSession(null)), [session]);

  const signIn = useCallback(() => {
    setBusy(true);
    setError(null);
    loginAddress(ORIGINS.login, window.location.href, window.sessionStorage)
      .then((address) => window.location.assign(address))
      .catch((e: unknown) => {
        setError(friendly(e, window.location.origin, DOOR, 'handoff'));
        setBusy(false);
      });
  }, []);

  const forget = useCallback(() => session?.forget(), [session]);

  return { session, starting, error, busy, signIn, forget };
}
