import type { EmailCodeSent } from '@hominemai/ordo-sdk/auth/email.js';
import { useState, type FormEvent } from 'react';
import type { SignInMethods } from '../config.js';
import type { HumanCheck } from './HumanCheck.js';

interface Props {
  /** The ways in this deployment serves (`config.ts`). */
  readonly methods: SignInMethods;
  /** The Turnstile widget and its token, or an always-ready check when it is off. */
  readonly human: HumanCheck;
  readonly busy: boolean;
  readonly error: string | null;
  /** The form and the passkey button: make an account. */
  readonly onSignUp: (display: string) => void;
  /** The passkey button alone, for someone who has an account. */
  readonly onSignIn: () => void;
  /** Mail a code. Resolves to what the second half needs, or null when it failed (the error is shown). */
  readonly onEmailBegin: (address: string) => Promise<EmailCodeSent | null>;
  /** The code the person typed. */
  readonly onEmailCode: (sent: EmailCodeSent, code: string, address: string) => void;
  /** Start or resume a guest. */
  readonly onGuest: () => void;
  /** True when this device holds a guest secret. */
  readonly guestHere: boolean;
  /** Go to an outside connection. */
  readonly onOutside: (connection: string) => void;
  /** Development only: a token and space printed by `ordo dev`. */
  readonly onToken?: ((token: string, space: string) => void) | undefined;
}

/**
 * Every way in the deployment serves. A passkey comes first: it is the
 * default, and the only one that proves presence for sharing.
 */
export function SignedOut(props: Props) {
  const { methods, human, busy, error, onSignUp, onSignIn, onToken } = props;
  const [display, setDisplay] = useState('');
  const [token, setToken] = useState('');
  const [space, setSpace] = useState('dev');
  const blocked = busy || !human.ready;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (display.trim() !== '') onSignUp(display.trim());
  };

  const others = methods.email || methods.guest || methods.connections.length > 0;

  return (
    <section className="card" aria-labelledby="sign-in-title">
      <h2 id="sign-in-title">Sign up or sign in</h2>
      <p className="hint">Your notes are kept for you alone. There is no password.</p>
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
            <button type="submit" className="primary" disabled={blocked || display.trim() === ''}>
              Sign up with a passkey
            </button>
            <button type="button" onClick={onSignIn} disabled={blocked}>
              Sign in with a passkey
            </button>
          </div>
        </form>
      )}

      {others && methods.passkey && <p className="or">or</p>}

      {methods.email && (
        <EmailForm busy={busy} blocked={blocked} onBegin={props.onEmailBegin} onCode={props.onEmailCode} />
      )}

      {methods.connections.length > 0 && (
        <div className="row">
          {methods.connections.map((c) => (
            <button key={c.name} type="button" disabled={blocked} onClick={() => props.onOutside(c.name)}>
              Continue with {c.label}
            </button>
          ))}
        </div>
      )}

      {methods.guest && (
        <div>
          <div className="row">
            <button type="button" disabled={blocked} onClick={props.onGuest}>
              {props.guestHere ? 'Continue as your guest account' : 'Try it as a guest'}
            </button>
          </div>
          <p className="hint">
            A guest account lives on this device. Add a passkey once you are in, so you can get back from anywhere.
          </p>
        </div>
      )}

      {human.widget}

      {onToken !== undefined && import.meta.env.DEV && (
        <details>
          <summary>Use a development token</summary>
          <p className="hint">
            For <code>ordo dev</code>, which has no sign-in. Paste the token it printed and the space it serves.
          </p>
          <label htmlFor="dev-token">Token</label>
          <input id="dev-token" value={token} onChange={(e) => setToken(e.target.value)} />
          <label htmlFor="dev-space">Space</label>
          <input id="dev-space" value={space} onChange={(e) => setSpace(e.target.value)} />
          <div className="row">
            <button type="button" disabled={busy || token.trim() === ''} onClick={() => onToken(token.trim(), space.trim())}>
              Use this token
            </button>
          </div>
        </details>
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
  readonly blocked: boolean;
  readonly onBegin: (address: string) => Promise<EmailCodeSent | null>;
  readonly onCode: (sent: EmailCodeSent, code: string, address: string) => void;
}

/** An address, then the code that was mailed to it. */
export function EmailForm({ busy, blocked, onBegin, onCode }: EmailProps) {
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
          <button type="submit" disabled={blocked || address.trim() === ''}>
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
        if (code.trim() !== '') onCode(sent, code.trim(), address.trim());
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
        <button type="button" disabled={blocked} onClick={() => void send()}>
          Send a new code
        </button>
        <button type="button" disabled={busy} onClick={() => setSent(null)}>
          Use another address
        </button>
      </div>
    </form>
  );
}
