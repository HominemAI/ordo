import { ConsoleCallError, SIGNED_OUT } from '../api';
import { passkeysAvailable, register } from '../webauthn';
import { useBusy } from './useBusy';

/** After a recovery code: a new passkey is required before anything else. */
export function NewPasskey(props: { onDone: (codes: string[]) => void; onSignedOut: () => void }) {
  const { busy, error, run } = useBusy();
  const can = passkeysAvailable();
  return (
    <section className="card">
      <h1>Add a new passkey</h1>
      <p>You signed in with a recovery code. Add a new passkey now. Your old recovery codes stop working and you get new ones.</p>
      {error && <p role="alert" className="error">{error}</p>}
      <div className="actions">
        <button
          type="button"
          className="primary"
          disabled={busy || !can}
          onClick={() =>
            void run(async () => {
              try {
                const r = await register<{ recoveryCodes?: string[] }>('passkeys/begin', 'passkeys/finish');
                props.onDone(r.recoveryCodes ?? []);
              } catch (e) {
                if (e instanceof ConsoleCallError && SIGNED_OUT.has(e.code)) {
                  props.onSignedOut();
                  return;
                }
                throw e;
              }
            })
          }
        >
          {busy ? 'Waiting for your passkey…' : 'Add a passkey'}
        </button>
      </div>
    </section>
  );
}
