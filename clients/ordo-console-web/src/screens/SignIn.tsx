import { passkeysAvailable, assert } from '../webauthn';
import { useBusy } from './useBusy';

export function SignIn(props: {
  note?: string;
  onSignedIn: () => void;
  onRecovery: () => void;
  onCode: () => void;
}) {
  const { busy, error, run } = useBusy();
  const can = passkeysAvailable();
  return (
    <section className="card">
      <h1>Sign in</h1>
      {props.note && <p className="note" role="status">{props.note}</p>}
      <p>Use the passkey you set up for this console.</p>
      {!can && <p role="alert" className="error">This browser cannot use passkeys. Use a recovery code, or a current browser.</p>}
      {error && <p role="alert" className="error">{error}</p>}
      <div className="actions">
        <button
          type="button"
          className="primary"
          disabled={busy || !can}
          onClick={() =>
            void run(async () => {
              await assert('signin/begin', 'signin/finish');
              props.onSignedIn();
            })
          }
        >
          {busy ? 'Waiting for your passkey…' : 'Sign in with a passkey'}
        </button>
      </div>
      <hr />
      <ul className="ways">
        <li>
          <button type="button" className="link" onClick={props.onRecovery}>
            Lost your passkey? Use a recovery code
          </button>
        </li>
        <li>
          <button type="button" className="link" onClick={props.onCode}>
            Have an invite code?
          </button>
        </li>
      </ul>
    </section>
  );
}
