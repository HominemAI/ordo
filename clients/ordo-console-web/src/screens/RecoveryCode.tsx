import { useId, useState } from 'react';
import { post } from '../api';
import { useBusy } from './useBusy';

/** A recovery code signs in once; a new passkey must follow. */
export function RecoveryCode(props: { onRedeemed: () => void; onBack: () => void }) {
  const [code, setCode] = useState('');
  const { busy, error, run } = useBusy();
  const id = useId();
  return (
    <section className="card">
      <h1>Use a recovery code</h1>
      <p>Each code works once. After it, you add a new passkey and get a new set of codes.</p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void run(async () => {
            await post('recovery', { code: code.trim() });
            props.onRedeemed();
          });
        }}
      >
        <label htmlFor={id}>Recovery code</label>
        <input
          id={id}
          name="recovery-code"
          className="mono"
          autoComplete="off"
          autoCapitalize="characters"
          spellCheck={false}
          required
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
        {error && <p role="alert" className="error">{error}</p>}
        <div className="actions">
          <button type="submit" className="primary" disabled={busy}>
            {busy ? 'Checking…' : 'Use this code'}
          </button>
          <button type="button" className="link" onClick={props.onBack}>
            Back to sign in
          </button>
        </div>
      </form>
    </section>
  );
}
