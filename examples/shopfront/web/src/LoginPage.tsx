import { issueHandoff, readHandoffRequest } from '@hominemai/ordo-sdk/auth/handoff.js';
import {
  beginEmailSignIn,
  completeEmailSignIn,
  type EmailCodeSent,
} from '@hominemai/ordo-sdk/auth/email.js';
import { registerPasskey, signInWithPasskey, type PasskeySession } from '@hominemai/ordo-sdk/auth/passkey.js';
import { useEffect, useMemo, useRef, useState } from 'react';
import { SignInForm } from './components/SignInForm.js';
import { DOOR, ORIGINS, SPACE, type SignInMethods } from './config.js';
import { friendly, type Way } from './errors.js';
import { useLoad } from './hooks.js';
import { loadMethods } from './methods.js';
import { allowedReturn } from './pages.js';
import { Session, sessionStore } from './session.js';
import { me } from './generated/ops.js';

const KEY = 'ordo-shopfront.login';
const DEVICE_KEY = 'ordo-shopfront.device';

const options = () => ({ key: KEY, baseUrl: DOOR, space: SPACE, store: sessionStore() });

/** The email device marker this browser holds, if any. */
function deviceMarker(): string | undefined {
  try {
    return localStorage.getItem(DEVICE_KEY) ?? undefined;
  } catch {
    return undefined;
  }
}

function keepDeviceMarker(marker: string | undefined): void {
  try {
    if (marker) localStorage.setItem(DEVICE_KEY, marker);
  } catch {
    // Nothing kept.
  }
}

/** The login page: the core site. It signs people in and hands them back to the shop or the console. */
export function LoginPage() {
  const [session, setSession] = useState<Session | null>(() => Session.restore(options()));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The ways in come from the door. Until they arrive the form waits.
  const [methods, setMethods] = useState<SignInMethods | null>(null);
  useEffect(() => {
    let alive = true;
    void loadMethods({ baseUrl: DOOR }).then((m) => alive && setMethods(m));
    return () => {
      alive = false;
    };
  }, []);
  const origin = window.location.origin;
  const request = useMemo(() => readHandoffRequest(window.location.href), []);
  const refusedReturn = request !== null && !allowedReturn(request.returnTo, ORIGINS);
  const refused = session?.admission?.outcome === 'refused';
  const handing = useRef(false);

  useEffect(() => session?.onEnd(() => setSession(null)), [session]);

  // Signed in, and asked for by the shop or the console: pass the person back.
  useEffect(() => {
    if (session === null || request === null || refusedReturn || refused || handing.current) return;
    handing.current = true;
    void (async () => {
      try {
        const issued = await issueHandoff({
          baseUrl: DOOR,
          token: await session.token(),
          returnTo: request.returnTo,
          binderHash: request.binderHash,
        });
        window.location.replace(issued.redirect);
      } catch (e) {
        handing.current = false;
        setError(friendly(e, origin, DOOR, 'handoff'));
      }
    })();
  }, [session, request, refusedReturn, refused, origin]);

  const enter = async (make: () => Promise<PasskeySession>, way: Way) => {
    setBusy(true);
    setError(null);
    try {
      const done = await make();
      keepDeviceMarker(done.device);
      setSession(Session.from(options(), done));
    } catch (e) {
      setError(friendly(e, origin, DOOR, way));
    } finally {
      setBusy(false);
    }
  };

  const endpoint = { baseUrl: DOOR };
  const signUp = (display: string) => void enter(() => registerPasskey(endpoint, { display, space: SPACE }), 'passkey');
  const signIn = () => void enter(() => signInWithPasskey(endpoint, { space: SPACE }), 'passkey');
  const emailBegin = async (address: string): Promise<EmailCodeSent | null> => {
    setBusy(true);
    setError(null);
    try {
      const device = deviceMarker();
      return await beginEmailSignIn(endpoint, { address, space: SPACE, ...(device ? { device } : {}) });
    } catch (e) {
      setError(friendly(e, origin, DOOR, 'email'));
      return null;
    } finally {
      setBusy(false);
    }
  };
  const emailCode = (sent: EmailCodeSent, code: string) =>
    void enter(async () => {
      const r = await completeEmailSignIn(endpoint, sent, code, { space: SPACE });
      if (r.kind === 'second-step') {
        throw new Error('This deployment asks for a second step, which this page does not offer.');
      }
      return r.session;
    }, 'email');

  const signOut = async () => {
    if (session === null) return;
    setBusy(true);
    try {
      await session.signOut();
    } catch (e) {
      setError(friendly(e, origin));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main>
      <header>
        <h1>Shopfront sign-in</h1>
      </header>
      {refusedReturn && (
        <p role="alert" className="error">
          This page cannot send you to {request?.returnTo}. It only returns to the shop and the staff console.
        </p>
      )}
      {session === null ? (
        <>
          {request !== null && !refusedReturn && <p className="hint">Sign in to go back to {new URL(request.returnTo).origin}.</p>}
          {methods === null ? (
            <p className="hint">Checking how you can sign in...</p>
          ) : (
            <SignInForm
              methods={methods}
              busy={busy}
              error={error}
              onSignUp={signUp}
              onSignIn={signIn}
              onEmailBegin={emailBegin}
              onEmailCode={emailCode}
            />
          )}
        </>
      ) : refused ? (
        <section className="card">
          <p role="alert" className="error">
            You signed in, but the shop did not let you in. Ask the app developer to check the shop's way in.
          </p>
          <button type="button" onClick={() => void signOut()} disabled={busy}>
            Sign out
          </button>
        </section>
      ) : request !== null && !refusedReturn ? (
        <section className="card" aria-live="polite">
          <p>Signed in. Taking you back.</p>
          {error !== null && (
            <p role="alert" className="error">
              {error}
            </p>
          )}
        </section>
      ) : (
        <Signed session={session} busy={busy} error={error} onSignOut={() => void signOut()} />
      )}
    </main>
  );
}

function Signed(props: { session: Session; busy: boolean; error: string | null; onSignOut: () => void }) {
  const { session, busy, error, onSignOut } = props;
  const who = useLoad(() => me(session.client()).fetch().then((p) => p.rows[0] ?? null), [session]);
  const row = who.data;
  return (
    <section className="card" aria-labelledby="signed-in-title">
      <h2 id="signed-in-title">You are signed in</h2>
      {row ? (
        <p>
          {row.displayName} <span className="hint">member {row.id}</span>
        </p>
      ) : (
        who.error === null && <p className="hint">Looking you up.</p>
      )}
      {who.error !== null && (
        <p role="alert" className="error">
          {who.error}
        </p>
      )}
      <ul className="plain">
        <li>
          <a href={ORIGINS.shop}>Go to the shop</a>
        </li>
        <li>
          <a href={ORIGINS.admin}>Go to the staff console</a>
        </li>
      </ul>
      <div className="row">
        <button type="button" onClick={onSignOut} disabled={busy}>
          Sign out
        </button>
      </div>
      {error !== null && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </section>
  );
}
