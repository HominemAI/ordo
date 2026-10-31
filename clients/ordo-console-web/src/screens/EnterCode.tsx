import { useId, useState } from 'react';
import type { Enrolled } from '../api';
import { passkeysAvailable, register } from '../webauthn';
import { useBusy } from './useBusy';

/**
 * A one-time code, then a new passkey: first-run setup, an invite to be the
 * second owner, or a code from `ordo console recover`. The console tells
 * which from the code.
 */
export function EnterCode(props: {
  heading: string;
  intro: string;
  button: string;
  onDone: (r: Enrolled) => void;
  onBack?: () => void;
}) {
  const [code, setCode] = useState('');
  const [label, setLabel] = useState('');
  const { busy, error, run } = useBusy();
  const codeId = useId();
  const labelId = useId();
  const can = passkeysAvailable();

  return (
    <section className="card">
      <h1>{props.heading}</h1>
      <p>{props.intro}</p>
      {!can && <p role="alert" className="error">This browser cannot use passkeys. Open the console in a current browser.</p>}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void run(async () => {
            const r = await register<Enrolled>('setup/begin', 'setup/finish', {
              code: code.trim(),
              ...(label.trim() ? { label: label.trim() } : {}),
            });
            props.onDone(r);
          });
        }}
      >
        <label htmlFor={codeId}>Code</label>
        <input
          id={codeId}
          name="code"
          className="mono"
          autoComplete="one-time-code"
          autoCapitalize="characters"
          spellCheck={false}
          required
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
        <label htmlFor={labelId}>Name for this passkey <span className="muted">(optional)</span></label>
        <input
          id={labelId}
          name="label"
          maxLength={64}
          placeholder="My laptop"
          value={label}
          onChange={(e) => setLabel(e.target.value)}
        />
        {error && <p role="alert" className="error">{error}</p>}
        <div className="actions">
          <button type="submit" className="primary" disabled={busy || !can}>
            {busy ? 'Waiting for your passkey…' : props.button}
          </button>
          {props.onBack && (
            <button type="button" className="link" onClick={props.onBack}>
              Back to sign in
            </button>
          )}
        </div>
      </form>
    </section>
  );
}
