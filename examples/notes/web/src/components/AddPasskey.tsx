import type { EmailCodeSent } from '@hominemai/ordo-sdk/auth/email.js';
import { useEffect, useState } from 'react';
import { returnUrl, type Connection } from '../config.js';
import { friendly } from '../errors.js';
import type { Session } from '../session.js';
import type { HumanCheck } from './HumanCheck.js';

interface Props {
  readonly session: Session;
  readonly human: HumanCheck;
  /** The outside connections the door serves, for the provider's name on a button. */
  readonly connections: readonly Connection[];
}

/**
 * For an account with no passkey: add one, by linking it to the account
 * (a claim). The account keeps its id, its tenant and its notes.
 *
 * A claim needs a fresh proof of a sign-in the account already holds. For a
 * guest that is the secret on this device, with nothing to type. For an
 * emailed-code account it is a new code to the same address. For an account
 * that came in through an outside connection it is another trip to the
 * provider and back: the first press leaves, and on return a second press
 * adds the passkey. Shown only when the account holds no passkey.
 */
export function AddPasskey({ session, human, connections }: Props) {
  const [needed, setNeeded] = useState(false);
  const [sent, setSent] = useState<EmailCodeSent | null>(null);
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  useEffect(() => {
    let alive = true;
    session.hasPasskey().then(
      (has) => alive && setNeeded(!has),
      // Unknown: say nothing rather than nag.
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, [session]);

  if (done) {
    return (
      <p className="banner" role="status">
        Passkey added. Use it to sign in from now on.
      </p>
    );
  }
  if (!needed) return null;

  const run = async (act: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await act();
    } catch (e) {
      setError(friendly(e, undefined, 'claim'));
    } finally {
      setBusy(false);
      human.reset();
    }
  };

  const proof = session.claimProof;
  const connection = session.outsideConnection;
  const provider = connections.find((c) => c.name === connection)?.label ?? connection ?? '';
  return (
    <section className="card" aria-labelledby="add-passkey-title">
      <h2 id="add-passkey-title">Add a passkey</h2>
      <p className="hint">
        A passkey gets you back in from any device, and sharing a note asks for one. Your notes stay as they are.
      </p>
      {proof === 'guest' && (
        <div className="row">
          <button
            className="primary"
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await session.addPasskey();
                setDone(true);
              })
            }
          >
            Add a passkey
          </button>
        </div>
      )}
      {proof === 'email' && sent === null && (
        <>
          <p className="hint">First we email a code to {session.address} to check it is you.</p>
          <div className="row">
            <button
              className="primary"
              disabled={busy || !human.ready}
              onClick={() => void run(async () => setSent(await session.sendClaimCode(human.token)))}
            >
              Email me a code
            </button>
          </div>
          {human.widget}
        </>
      )}
      {proof === 'email' && sent !== null && (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void run(async () => {
              await session.addPasskey(sent, code.trim());
              setDone(true);
            });
          }}
        >
          <label htmlFor="claim-code">Code</label>
          <input
            id="claim-code"
            inputMode="numeric"
            autoComplete="one-time-code"
            value={code}
            onChange={(e) => setCode(e.target.value)}
            disabled={busy}
          />
          <div className="row">
            <button type="submit" className="primary" disabled={busy || code.trim() === ''}>
              Add a passkey
            </button>
            <button type="button" disabled={busy} onClick={() => setSent(null)}>
              Send a new code
            </button>
          </div>
        </form>
      )}
      {proof === 'outside' && !session.outsideProved && (
        <>
          <p className="hint">
            First {provider} checks it is you. You leave this page and come straight back.
          </p>
          <div className="row">
            <button
              className="primary"
              disabled={busy || !human.ready}
              onClick={() =>
                void run(async () => {
                  window.location.assign(
                    await session.beginOutsideClaim(returnUrl(window.location), human.token, window.location.hash),
                  );
                })
              }
            >
              Continue with {provider}
            </button>
          </div>
          {human.widget}
        </>
      )}
      {proof === 'outside' && session.outsideProved && (
        <>
          <p className="hint">{provider} checked it is you. Now your browser makes the passkey.</p>
          <div className="row">
            <button
              className="primary"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await session.addPasskey();
                  setDone(true);
                })
              }
            >
              Add a passkey
            </button>
          </div>
        </>
      )}
      {proof === null && (
        <p className="hint">
          This page adds a passkey to a guest account, an emailed-code account or one that came in through an outside
          connection. You signed in with {wayIn(session.via)}.
        </p>
      )}
      {error !== null && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </section>
  );
}

/** How a person came in, in words: never the door's own name for it. */
export function wayIn(via: string): string {
  if (via === 'passkey') return 'a passkey';
  if (via === 'email-otp') return 'an emailed code';
  if (via === 'guest') return 'a guest start';
  const m = /^(?:oauth2|oidc):(.+)$/.exec(via);
  if (m) return `your ${m[1]!.charAt(0).toUpperCase()}${m[1]!.slice(1)} account`;
  return via;
}
