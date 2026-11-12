import { useCallback, useEffect, useId, useState } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import { exposeTouch, tenantTouches, type Exposed, type TenantTouches as Touches, type TouchSituation } from '../deploys';

const WAYS: { value: Exposed; label: string }[] = [
  { value: 'none', label: 'None: tenants follow the level' },
  { value: 'raise', label: 'Raise: a tenant may turn it on' },
  { value: 'lower', label: 'Lower: a tenant may turn it off' },
  { value: 'both', label: 'Both ways' },
];

function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/**
 * Which passkey touches tenants may choose for their own space, and which
 * way (owner, 2026-11-05: "The dev exposes the options"). Nothing is exposed
 * by default. Each change takes a reason and is kept with who made it. The
 * door refuses an act on the deployment by name, and that refusal is shown
 * as given.
 */
export function TenantTouches(props: {
  atDeployment: <T>(f: () => Promise<T>) => Promise<T>;
  onSignedOut: () => void;
}) {
  const [now, setNow] = useState<Touches | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [editing, setEditing] = useState<TouchSituation | null>(null);
  const [way, setWay] = useState<Exposed>('none');
  const [reason, setReason] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const ids = useId();

  const signedOut = (e: unknown) => e instanceof ConsoleCallError && SIGNED_OUT.has(e.code);

  const load = useCallback(async () => {
    try {
      setNow(await tenantTouches());
      setLoadError(null);
    } catch (e) {
      if (signedOut(e)) return props.onSignedOut();
      setLoadError(explain(e));
    }
  }, [props]);

  useEffect(() => {
    void load();
  }, []);

  const open = (s: TouchSituation) => {
    setEditing(s);
    setWay(s.exposed);
    setReason('');
    setError(null);
    setDone(null);
  };

  const save = async (s: TouchSituation) => {
    setBusy(true);
    setError(null);
    try {
      const got = await props.atDeployment(() => exposeTouch(s.situation, way, reason.trim()));
      setDone(
        got.changed ? `${s.label}: tenants now ${got.exposed}.` : `${s.label} was already exposed ${got.exposed}.`,
      );
      setEditing(null);
      await load();
    } catch (e) {
      if (signedOut(e)) return props.onSignedOut();
      setError(explain(e));
    } finally {
      setBusy(false);
    }
  };

  const errorId = `${ids}-error`;

  return (
    <div role="region" aria-labelledby={`${ids}-title`}>
      <h2 id={`${ids}-title`}>Touches tenants may choose</h2>
      <p className="muted small">
        A tenant's owner may turn an exposed touch on or off for their own space. Nothing is exposed until you
        expose it. Acts on the deployment are never a tenant's. A system is never asked.
      </p>
      {loadError && <p className="muted small">The touches did not load: {loadError}</p>}
      {now && (
        <table className="kv">
          <caption className="muted small"><code className="code">ORDO_SECURITY_LEVEL={now.level}</code></caption>
          <thead>
            <tr>
              <th scope="col">Situation</th>
              <th scope="col">The level</th>
              <th scope="col">Tenants</th>
            </tr>
          </thead>
          <tbody>
            {now.situations.map((s) => (
              <tr key={s.situation}>
                <th scope="row">{s.label}</th>
                <td>{s.levelAsks ? 'asks a touch' : 'no touch'}</td>
                <td>
                  {s.choosable ? (
                    <>
                      <span className="mono">{s.exposed}</span>{' '}
                      {editing?.situation !== s.situation && (
                        <button
                          type="button"
                          className="link"
                          disabled={busy}
                          aria-label={`Change what tenants may choose for ${s.label}`}
                          onClick={() => open(s)}
                        >
                          Change
                        </button>
                      )}
                    </>
                  ) : (
                    <span className="muted small">never a tenant's</span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {editing && (
        <form
          className="confirm"
          aria-label={`Change what tenants may choose for ${editing.label}`}
          onSubmit={(e) => {
            e.preventDefault();
            void save(editing);
          }}
        >
          <label htmlFor={`${ids}-way`}>{editing.label}</label>
          <select
            id={`${ids}-way`}
            name="exposed"
            value={way}
            aria-describedby={error ? errorId : undefined}
            onChange={(e) => {
              setWay(e.target.value as Exposed);
              setError(null);
            }}
          >
            {WAYS.map((w) => (
              <option key={w.value} value={w.value}>
                {w.label}
              </option>
            ))}
          </select>
          <label htmlFor={`${ids}-reason`}>Reason</label>
          <input
            id={`${ids}-reason`}
            name="reason"
            required
            maxLength={500}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
          <div className="actions">
            <button type="submit" className="primary" disabled={busy}>
              Save
            </button>
            <button type="button" className="link" disabled={busy} onClick={() => setEditing(null)}>
              Cancel
            </button>
          </div>
        </form>
      )}

      <div aria-live="polite">
        {error && (
          <p id={errorId} className="error">
            {error}
          </p>
        )}
        {done && <p className="ok">{done}</p>}
      </div>

      {now?.history && now.history.length > 0 && (
        <details className="logs">
          <summary>Changes ({now.history.length})</summary>
          <ol className="kv" aria-label="Changes to what tenants may choose">
            {now.history.map((c, i) => (
              <li key={i}>
                <span className="mono">
                  {c.situation} exposed {c.exposed}
                </span>{' '}
                <span className="muted small">
                  {when(c.at)} by {c.actor}: {c.reason}
                </span>
              </li>
            ))}
          </ol>
        </details>
      )}
    </div>
  );
}
