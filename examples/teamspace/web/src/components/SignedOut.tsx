import type { EmailCodeSent } from '@hominemai/ordo-sdk/auth/email.js';
import { useState, type FormEvent } from 'react';
import type { SignInMethods } from '../config.js';

interface Props {
  /** The ways in this deployment serves (`config.ts`). */
  readonly methods: SignInMethods;
  /** True on an invitation link: the words change, the ways in do not. */
  readonly invited: boolean;
  readonly busy: boolean;
  readonly error: string | null;
  /** The form and the passkey button: make an account. */
  readonly onSignUp: (display: string) => void;
  /** The passkey button alone, for someone who has an account. */
  readonly onSignIn: () => void;
  /** Mail a code. Resolves to what the second half needs, or null when it failed (the error is shown). */
  readonly onEmailBegin: (address: string) => Promise<EmailCodeSent | null>;
  /** The code the person typed. */
  readonly onEmailCode: (sent: EmailCodeSent, code: string) => void;
  /** Start or resume a guest. */
  readonly onGuest: () => void;
  /** True when this device holds a guest secret. */
  readonly guestHere: boolean;
  /** Go to an outside connection. */
  readonly onOutside: (connection: string) => void;
}

/**
 * Every way in the deployment serves, passkey first. On an invitation link the
 * same buttons carry the invitation, so the person lands in the team that
 * invited them.
 */
export function SignedOut(props: Props) {
  const { methods, invited, busy, error, onSignUp, onSignIn } = props;
  const [display, setDisplay] = useState('');

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (display.trim() !== '') onSignUp(display.trim());
  };

  const others = methods.email || methods.guest || methods.connections.length > 0;

  return (
    <section className="card" aria-labelledby="sign-in-title">
      {invited ? (
        <>
          <h2 id="sign-in-title">You were invited to a team</h2>
          <p className="hint">Sign up to join it. If you already have an account, sign in and you join it too.</p>
        </>
      ) : (
        <>
          <h2 id="sign-in-title">Sign up or sign in</h2>
          <p className="hint">Sign up to start a team of your own. There is no password.</p>
        </>
      )}
      {methods.passkey && (
        <form onSubmit={submit}>
          <label htmlFor="display">Your name</label>
          <input
            id="display"
            value={display}
            autoComplete="name"
            onChange={(e) => setDisplay(e.target.value)}
            placeholder="What should we call you?"
            disabled={busy}
          />
          <div className="row">
            <button type="submit" className="primary" disabled={busy || display.trim() === ''}>
              Sign up with a passkey
            </button>
            <button type="button" onClick={onSignIn} disabled={busy}>
              {invited ? 'I already have an account: sign in' : 'Sign in with a passkey'}
            </button>
          </div>
        </form>
      )}

      {others && methods.passkey && <p className="or">or</p>}

      {methods.email && <EmailForm busy={busy} onBegin={props.onEmailBegin} onCode={props.onEmailCode} />}

      {methods.connections.length > 0 && (
        <div className="row">
          {methods.connections.map((c) => (
            <button key={c.name} type="button" disabled={busy} onClick={() => props.onOutside(c.name)}>
              Continue with {c.label}
            </button>
          ))}
        </div>
      )}

      {methods.guest && (
        <div className="row">
          <button type="button" disabled={busy} onClick={props.onGuest}>
            {props.guestHere ? 'Continue as your guest account' : 'Try it as a guest'}
          </button>
        </div>
      )}

      {busy && <p className="hint">Working on it.</p>}
      {error !== null && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </section>
  );
}

interface EmailProps {
  readonly busy: boolean;
  readonly onBegin: (address: string) => Promise<EmailCodeSent | null>;
  readonly onCode: (sent: EmailCodeSent, code: string) => void;
}

/** An address, then the code that was mailed to it. */
export function EmailForm({ busy, onBegin, onCode }: EmailProps) {
  const [address, setAddress] = useState('');
  const [code, setCode] = useState('');
  const [sent, setSent] = useState<EmailCodeSent | null>(null);

  const send = async (e?: FormEvent) => {
    e?.preventDefault();
    if (address.trim() === '') return;
    const s = await onBegin(address.trim());
    if (s !== null) {
      setSent(s);
      setCode('');
    }
  };

  if (sent === null) {
    return (
      <form onSubmit={(e) => void send(e)}>
        <label htmlFor="email">Email</label>
        <input
          id="email"
          type="email"
          autoComplete="email"
          value={address}
          onChange={(e) => setAddress(e.target.value)}
          placeholder="you@example.com"
          disabled={busy}
        />
        <div className="row">
          <button type="submit" disabled={busy || address.trim() === ''}>
            Email me a code
          </button>
        </div>
      </form>
    );
  }

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (code.trim() !== '') onCode(sent, code.trim());
      }}
    >
      <p className="hint">If {address.trim()} can sign in here, a code is on its way.</p>
      <label htmlFor="email-code">Code</label>
      <input
        id="email-code"
        inputMode="numeric"
        autoComplete="one-time-code"
        value={code}
        onChange={(e) => setCode(e.target.value)}
        disabled={busy}
      />
      <div className="row">
        <button type="submit" className="primary" disabled={busy || code.trim() === ''}>
          Sign in
        </button>
        <button type="button" disabled={busy} onClick={() => void send()}>
          Send a new code
        </button>
        <button type="button" disabled={busy} onClick={() => setSent(null)}>
          Use another address
        </button>
      </div>
    </form>
  );
}
