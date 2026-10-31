import { useCallback, useEffect, useState } from 'react';
import { ConsoleCallError, explain, get, handle, post, SIGNED_OUT, type Enrolled, type Session, type Status } from './api';
import { goSignIn, identityMode, setIdentity, signOut, token } from './identity';
import { Codes } from './screens/Codes';
import { EnterCode } from './screens/EnterCode';
import { Home } from './screens/Home';
import { NewPasskey } from './screens/NewPasskey';
import { RecoveryCode } from './screens/RecoveryCode';
import { SignIn } from './screens/SignIn';

/**
 * Which screen shows. Signed out, the screens follow one another in state;
 * signed in, Home routes by the address (`/d/<handle>/owners`).
 */
export type View =
  | { kind: 'loading' }
  | { kind: 'failed'; message: string }
  | { kind: 'setup' }
  | { kind: 'signin'; note?: string }
  | { kind: 'identity-signin'; note?: string }
  | { kind: 'code' }
  | { kind: 'recovery' }
  | { kind: 'new-passkey' }
  | { kind: 'codes'; codes: string[]; heading: string }
  | { kind: 'home'; session: Session };

export function App() {
  const [view, setView] = useState<View>({ kind: 'loading' });

  /** Find out where the person stands: signed in, or which way in. */
  const start = useCallback(async (note?: string) => {
    let status: Status;
    try {
      status = await get<Status>('status');
    } catch (e) {
      setView({ kind: 'failed', message: explain(e) });
      return;
    }
    setIdentity(status.identity);
    if (status.identity) {
      // People sign in at the identity service. No session there: go and
      // sign in, and come back to this address.
      try {
        await token();
      } catch (e) {
        if (e instanceof ConsoleCallError && SIGNED_OUT.has(e.code)) {
          if (note) setView({ kind: 'identity-signin', note });
          else goSignIn();
          return;
        }
        setView({ kind: 'failed', message: explain(e) });
        return;
      }
    }
    try {
      const session = await get<Session>('session');
      setView(session.mustEnrol ? { kind: 'new-passkey' } : { kind: 'home', session });
      return;
    } catch (e) {
      if (!(e instanceof ConsoleCallError && SIGNED_OUT.has(e.code))) {
        setView({ kind: 'failed', message: explain(e) });
        return;
      }
    }
    if (status.identity) {
      setView({ kind: 'identity-signin', note: 'You are signed out. Sign in again.' });
      return;
    }
    setView(status.hasOwner ? { kind: 'signin', ...(note ? { note } : {}) } : { kind: 'setup' });
  }, []);

  useEffect(() => {
    void start();
  }, [start]);

  /** A call found the session gone. */
  const signedOut = useCallback(() => {
    const note = 'You are signed out. Sign in again.';
    setView(identityMode() ? { kind: 'identity-signin', note } : { kind: 'signin', note });
  }, []);

  const enrolled = (r: Enrolled) => {
    const heading =
      r.kind === 'first-run'
        ? 'You are the console owner'
        : r.kind === 'invite'
          ? 'You are now a console owner'
          : 'Your new passkey is ready';
    setView({ kind: 'codes', codes: r.recoveryCodes, heading });
  };

  let screen;
  switch (view.kind) {
    case 'loading':
      screen = <p className="muted" aria-live="polite">Loading…</p>;
      break;
    case 'failed':
      screen = (
        <section className="card">
          <h1>The console did not answer</h1>
          <p role="alert" className="error">{view.message}</p>
          <button type="button" onClick={() => void start()}>Try again</button>
        </section>
      );
      break;
    case 'setup':
      screen = (
        <EnterCode
          heading="Set up the console"
          intro="Enter the setup code. The console printed it to its log, and wrote it to the file setup-code in its data folder."
          button="Continue with a passkey"
          onDone={enrolled}
        />
      );
      break;
    case 'code':
      screen = (
        <EnterCode
          heading="Join with a code"
          intro="Enter the invite code an owner gave you, or a code from ordo console recover."
          button="Continue with a passkey"
          onDone={enrolled}
          onBack={() => setView({ kind: 'signin' })}
        />
      );
      break;
    case 'identity-signin':
      screen = (
        <section className="card">
          <h1>Sign in</h1>
          {view.note && <p role="status">{view.note}</p>}
          <p>You sign in at the deployment's identity service with your passkey. It brings you back here.</p>
          <div className="actions">
            <button type="button" className="primary" onClick={goSignIn}>
              Sign in
            </button>
          </div>
        </section>
      );
      break;
    case 'signin':
      screen = (
        <SignIn
          {...(view.note ? { note: view.note } : {})}
          onSignedIn={() => void start()}
          onRecovery={() => setView({ kind: 'recovery' })}
          onCode={() => setView({ kind: 'code' })}
        />
      );
      break;
    case 'recovery':
      screen = (
        <RecoveryCode onRedeemed={() => setView({ kind: 'new-passkey' })} onBack={() => setView({ kind: 'signin' })} />
      );
      break;
    case 'new-passkey':
      screen = (
        <NewPasskey
          onDone={(codes) => setView({ kind: 'codes', codes, heading: 'Your new passkey is ready' })}
          onSignedOut={signedOut}
        />
      );
      break;
    case 'codes':
      screen = <Codes heading={view.heading} codes={view.codes} onContinue={() => void start()} />;
      break;
    case 'home':
      screen = (
        <Home
          session={view.session}
          onSignedOut={signedOut}
          onSignOut={async () => {
            if (identityMode()) {
              await signOut();
              setView({ kind: 'identity-signin', note: 'You are signed out.' });
              return;
            }
            try {
              await post('signout');
            } catch {
              // Signed out already, or the console is down: either way the page signs out.
            }
            setView({ kind: 'signin', note: 'You are signed out.' });
          }}
        />
      );
      break;
  }

  return (
    <div className="page">
      {view.kind !== 'home' && (
        <header className="bar">
          <span className="brand">Ordo console</span>
          <span className="muted handle">{handle}</span>
        </header>
      )}
      {view.kind === 'home' ? screen : <main className="narrow">{screen}</main>}
    </div>
  );
}
