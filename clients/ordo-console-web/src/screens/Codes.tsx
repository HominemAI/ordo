import { useEffect, useId, useState } from 'react';
import { handle } from '../api';

/**
 * The recovery codes, shown once. The person must say they saved them
 * before going on; the console keeps only their hashes.
 */
export function Codes(props: { heading: string; codes: string[]; onContinue: () => void }) {
  const [saved, setSaved] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [href, setHref] = useState<string | null>(null);
  const checkId = useId();
  const text = `Ordo console (${handle}) recovery codes. Each works once.\n\n${props.codes.join('\n')}\n`;

  useEffect(() => {
    const url = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
    setHref(url);
    return () => URL.revokeObjectURL(url);
  }, [text]);

  return (
    <section className="card">
      <h1>{props.heading}</h1>
      <h2>Save your recovery codes</h2>
      <p>
        If you lose your passkey, one of these codes lets you in once. <strong>They will not be shown again.</strong>{' '}
        Keep them somewhere safe, away from this device.
      </p>
      <ol className="codes mono" aria-label="Recovery codes">
        {props.codes.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ol>
      <div className="actions">
        <button
          type="button"
          onClick={() => {
            navigator.clipboard.writeText(text).then(
              () => setCopied('Copied.'),
              () => setCopied('Copy did not work here. Download them, or write them down.'),
            );
          }}
        >
          Copy all
        </button>
        {href && (
          <a className="button" href={href} download={`ordo-console-${handle}-recovery-codes.txt`}>
            Download
          </a>
        )}
        {copied && <span className="muted" role="status">{copied}</span>}
      </div>
      <p className="check">
        <input id={checkId} type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} />
        <label htmlFor={checkId}>I saved these codes</label>
      </p>
      <div className="actions">
        <button type="button" className="primary" disabled={!saved} onClick={props.onContinue}>
          Continue
        </button>
      </div>
    </section>
  );
}
