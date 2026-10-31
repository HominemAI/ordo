import { useCallback, useEffect, useRef, useState } from 'react';
import {
  ConsoleCallError,
  explain,
  get,
  post,
  SIGNED_OUT,
  type Applied,
  type Diff,
  type DoorOnly,
  type Patch,
  type Revision,
  type Revisions,
  type Runtime as RuntimeView,
  type RuntimeStatus,
} from '../api';
import { withTouch } from '../touch';
import { DoorOnlyRuntime } from './DoorOnlyRuntime';
import { LogLines, STATE_NAME, StatusFacts, when } from './runtimeParts';

/** How often the screen reads the supervisor: idle, and while a change comes up. */
const IDLE_MS = 3000;
const WAITING_MS = 1000;

/** What the last action ended in. */
type Outcome =
  | { kind: 'applied'; applied: Applied; from?: number }
  | { kind: 'restarted'; status: RuntimeStatus }
  | { kind: 'refused'; revision: number | null; message: string }
  | { kind: 'rolled-back'; revision: number | null; to: number | null; message: string }
  | { kind: 'error'; message: string };

interface Pending {
  what: string;
  started: number;
}

interface Row {
  name: string;
  value: string;
}

const OUTCOME_WORDS: Record<NonNullable<Revision['outcome']>, string> = {
  good: 'Came up',
  refused: 'Refused by the settings check',
  failed: 'Did not come up',
  crashed: 'Kept stopping',
};

function who(actor: string): string {
  if (actor.startsWith('owner:')) return 'A console owner';
  if (actor === 'boot' || actor === 'console') return 'The console, at start';
  return actor;
}

/**
 * The Runtime screen (docs/plan-console.md 4.2): the deployment's settings
 * and secrets, the revisions they made, and the changes that start a new
 * process with them.
 */
export function Runtime(props: { onSignedOut: () => void }) {
  const [data, setData] = useState<RuntimeView | DoorOnly | null>(null);
  const [revs, setRevs] = useState<Revisions | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  const [rows, setRows] = useState<Row[]>([{ name: '', value: '' }]);
  const [unset, setUnset] = useState<string[]>([]);
  const [secretName, setSecretName] = useState('');
  const [secretHow, setSecretHow] = useState<'value' | 'file'>('value');
  const [secretValue, setSecretValue] = useState('');
  const [secretFile, setSecretFile] = useState('');
  const [unsetSecrets, setUnsetSecrets] = useState<string[]>([]);
  const [reason, setReason] = useState('');
  const [formError, setFormError] = useState<string | null>(null);

  const [rollbackTo, setRollbackTo] = useState<number | null>(null);
  const [rollbackReason, setRollbackReason] = useState('');
  const [confirmRestart, setConfirmRestart] = useState(false);

  const signedOut = useRef(props.onSignedOut);
  signedOut.current = props.onSignedOut;
  const waiting = useRef(false);
  waiting.current = pending !== null;

  const refresh = useCallback(async (withRevisions: boolean) => {
    try {
      const [r, list] = await Promise.all([
        get<RuntimeView | DoorOnly>('runtime'),
        withRevisions ? get<Revisions>('runtime/revisions').catch(() => null) : Promise.resolve(null),
      ]);
      setData(r);
      if (list) setRevs(list);
      setLoadError(null);
    } catch (e) {
      if (e instanceof ConsoleCallError && SIGNED_OUT.has(e.code)) {
        signedOut.current();
        return;
      }
      setLoadError(explain(e));
    }
  }, []);

  // Read the supervisor on open, then every few seconds; every second while
  // a change comes up, so the screen shows what it is doing.
  useEffect(() => {
    let live = true;
    let timer: number | undefined;
    const tick = async () => {
      await refresh(!waiting.current);
      if (live) timer = window.setTimeout(() => void tick(), waiting.current ? WAITING_MS : IDLE_MS);
    };
    void tick();
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [refresh]);

  useEffect(() => {
    if (!pending) return;
    setElapsed(0);
    const t = window.setInterval(() => setElapsed(Math.round((Date.now() - pending.started) / 1000)), 1000);
    return () => window.clearInterval(t);
  }, [pending]);

  /** Run a write that brings a revision up, and say how it ended. */
  const run = async (what: string, f: () => Promise<Outcome>): Promise<boolean> => {
    setOutcome(null);
    setPending({ what, started: Date.now() });
    let ok = false;
    try {
      setOutcome(await withTouch(f));
      ok = true;
    } catch (e) {
      if (e instanceof ConsoleCallError) {
        if (SIGNED_OUT.has(e.code)) {
          signedOut.current();
          return false;
        }
        if (e.code === 'settings_refused') {
          setOutcome({ kind: 'refused', revision: e.extra.revision ?? null, message: e.message });
        } else if (e.code === 'apply_rolled_back') {
          setOutcome({
            kind: 'rolled-back',
            revision: e.extra.revision ?? null,
            to: e.extra.rolledBackTo ?? null,
            message: e.message,
          });
        } else {
          setOutcome({ kind: 'error', message: explain(e) });
        }
      } else {
        setOutcome({ kind: 'error', message: explain(e) });
      }
    } finally {
      setPending(null);
      void refresh(true);
    }
    return ok;
  };

  if (data === null) {
    return (
      <section className="card">
        <h1>Runtime</h1>
        {loadError ? (
          <p role="alert" className="error">
            {loadError}
          </p>
        ) : (
          <p className="muted">Loading…</p>
        )}
      </section>
    );
  }

  if (data.mode === 'door-only') {
    return <DoorOnlyRuntime data={data} />;
  }

  const settings = data.current.settings;
  const secrets = data.current.secrets;
  const secretNames = new Set([...(data.secretSettings ?? []), ...secrets.map((s) => s.name)]);
  const busy = pending !== null;

  // The change as the form holds it.
  const set: Record<string, string> = {};
  for (const r of rows) {
    const name = r.name.trim();
    if (name && settings[name] !== r.value) set[name] = r.value;
  }
  const secretPatch: Patch['secrets'] = {};
  const sName = secretName.trim();
  if (sName) {
    if (secretHow === 'value' && secretValue) secretPatch[sName] = { value: secretValue };
    if (secretHow === 'file' && secretFile.trim()) secretPatch[sName] = { file: secretFile.trim() };
  }
  for (const n of unsetSecrets) secretPatch[n] = null;
  const patch: Patch = { set, unset, secrets: secretPatch, reason: reason.trim() };
  const empty = Object.keys(set).length === 0 && unset.length === 0 && Object.keys(secretPatch).length === 0;

  const preview: string[] = [
    ...Object.entries(set).map(([k, v]) =>
      k in settings ? `Change ${k} from "${settings[k]}" to "${v}"` : `Add ${k} = "${v}"`,
    ),
    ...unset.map((k) => `Remove ${k}`),
    ...Object.entries(secretPatch).map(([k, v]) =>
      v === null ? `Remove secret ${k}` : 'file' in v ? `Read secret ${k} from ${v.file}` : `Set secret ${k}`,
    ),
  ];

  const resetForm = () => {
    setRows([{ name: '', value: '' }]);
    setUnset([]);
    setSecretName('');
    setSecretValue('');
    setSecretFile('');
    setUnsetSecrets([]);
    setReason('');
  };

  const apply = async () => {
    setFormError(null);
    const asSecret = Object.keys(set).find((k) => secretNames.has(k));
    if (asSecret) {
      setFormError(`${asSecret} is a secret. Set it under "Set a secret", so its value is never shown.`);
      return;
    }
    const ok = await run('Applying the change', async () => ({
      kind: 'applied',
      applied: await post<Applied>('runtime/apply', patch),
    }));
    if (ok) resetForm();
  };

  const rollback = async (to: number) => {
    const ok = await run(`Rolling back to revision ${to}`, async () => ({
      kind: 'applied',
      from: to,
      applied: await post<Applied>('runtime/rollback', { revision: to, reason: rollbackReason.trim() }),
    }));
    if (ok) {
      setRollbackTo(null);
      setRollbackReason('');
    }
  };

  const restart = async () => {
    setConfirmRestart(false);
    await run('Restarting', async () => ({
      kind: 'restarted',
      status: (await post<{ status: RuntimeStatus }>('runtime/restart')).status,
    }));
  };

  const s = data.status;

  return (
    <section className="card runtime">
      <h1>Runtime</h1>

      <StatusFacts status={s} />
      <p>
        The deployment runs revision <strong>{data.current.revision}</strong> of its settings.
      </p>
      <div className="actions">
        {!confirmRestart ? (
          <button type="button" disabled={busy} onClick={() => setConfirmRestart(true)}>
            Restart
          </button>
        ) : (
          <div className="confirm" role="group" aria-label="Confirm restart">
            <p>Restart the deployment with the same settings? It stops for a moment. This needs a touch of your passkey.</p>
            <div className="actions">
              <button type="button" className="danger" disabled={busy} onClick={() => void restart()}>
                Yes, restart
              </button>
              <button type="button" className="link" onClick={() => setConfirmRestart(false)}>
                Cancel
              </button>
            </div>
          </div>
        )}
      </div>

      <div role="status" className="progress">
        {pending && (
          <p>
            {pending.what}. This can take up to about 150 seconds.{' '}
            {s.applying !== null ? `Bringing up revision ${s.applying}: ` : 'The deployment is '}
            {STATE_NAME[s.state].toLowerCase()}.
          </p>
        )}
        {outcome?.kind === 'applied' && (
          <div className="ok">
            <p>
              {outcome.from !== undefined && outcome.from !== outcome.applied.revision
                ? `Rolled back. Revision ${outcome.applied.revision} runs the settings of revision ${outcome.from}, and it is ready.`
                : `${outcome.from !== undefined ? 'Rolled back. ' : ''}Revision ${outcome.applied.revision} is live and ready.`}
            </p>
            <DiffList diff={outcome.applied.diff} />
          </div>
        )}
        {outcome?.kind === 'restarted' && (
          <p className="ok">Restarted. Revision {outcome.status.liveRevision} runs again.</p>
        )}
      </div>
      {pending && (
        <p className="muted small" aria-live="off">
          {elapsed} seconds so far.
        </p>
      )}
      {outcome?.kind === 'refused' && (
        <div role="alert" className="error">
          <p>
            {outcome.revision !== null ? `Revision ${outcome.revision} was refused` : 'The change was refused'} by the
            settings check. Nothing running was touched.
          </p>
          <pre className="mono small">{outcome.message}</pre>
        </div>
      )}
      {outcome?.kind === 'rolled-back' && (
        <div role="alert" className="error">
          <p>
            Revision {outcome.revision ?? '?'} did not come up. Revision {outcome.to ?? 'the last good one'} runs again.
          </p>
          <pre className="mono small">{outcome.message}</pre>
        </div>
      )}
      {outcome?.kind === 'error' && (
        <p role="alert" className="error">
          {outcome.message}
        </p>
      )}
      {loadError && (
        <p role="alert" className="error">
          {loadError}
        </p>
      )}

      <LogLines lines={data.logs} />

      <h2>How it runs</h2>
      <p>
        {data.privilege.separateUser
          ? 'The deployment runs as its own unix user, apart from the console.'
          : data.privilege.note ?? 'The deployment runs as the same unix user as the console.'}
      </p>
      {data.containerVariablesIgnored.length > 0 && (
        <p className="note small">
          These container variables are ignored, because the console starts the deployment with its own record:{' '}
          <span className="mono">{data.containerVariablesIgnored.join(', ')}</span>.
        </p>
      )}

      <h2>Settings</h2>
      {Object.keys(settings).length === 0 ? (
        <p className="muted">No settings.</p>
      ) : (
        <ul className="kv" aria-label="Settings">
          {Object.entries(settings).map(([k, v]) => (
            <li key={k}>
              <span className="mono">{k}</span> = <span className="mono value">{v}</span>
            </li>
          ))}
        </ul>
      )}

      <h2>Secrets</h2>
      <p className="muted small">The console never shows a secret's value.</p>
      {secrets.length === 0 ? (
        <p className="muted">No secrets.</p>
      ) : (
        <ul className="kv" aria-label="Secrets">
          {secrets.map((x) => (
            <li key={x.name}>
              <span className="mono">{x.name}</span>{' '}
              <span className="muted small">
                {x.kind === 'managed'
                  ? `kept sealed by the console${x.version !== undefined ? `, version ${x.version}` : ''}`
                  : x.kind === 'held'
                    ? 'kept sealed by the supervisor'
                    : `read from ${x.path ?? 'a file'}`}
                , changed {when(x.changedAt)}
              </span>
            </li>
          ))}
        </ul>
      )}

      <h2>Change the settings</h2>
      <p className="muted small">
        The console checks the new settings first. If the check refuses them, nothing running is touched. If they pass,
        the deployment restarts with them. If it is not ready in time, the last good revision runs again. This needs a
        touch of your passkey.
      </p>
      <form
        className="change"
        onSubmit={(e) => {
          e.preventDefault();
          void apply();
        }}
      >
        <fieldset>
          <legend>Add or change settings</legend>
          {rows.map((r, i) => (
            <div className="pair" key={i}>
              <div>
                <label htmlFor={`set-name-${i}`}>{i === 0 ? 'Setting name' : `Setting name ${i + 1}`}</label>
                <input
                  id={`set-name-${i}`}
                  className="mono"
                  value={r.name}
                  list="setting-names"
                  autoComplete="off"
                  autoCapitalize="characters"
                  autoCorrect="off"
                  spellCheck={false}
                  onChange={(e) => setRows(rows.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)))}
                />
              </div>
              <div>
                <label htmlFor={`set-value-${i}`}>{i === 0 ? 'Value' : `Value ${i + 1}`}</label>
                <input
                  id={`set-value-${i}`}
                  className="mono"
                  value={r.value}
                  autoComplete="off"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  onChange={(e) => setRows(rows.map((x, j) => (j === i ? { ...x, value: e.target.value } : x)))}
                />
              </div>
            </div>
          ))}
          <datalist id="setting-names">
            {Object.keys(settings).map((k) => (
              <option key={k} value={k} />
            ))}
          </datalist>
          <button type="button" className="link" onClick={() => setRows([...rows, { name: '', value: '' }])}>
            Add another setting
          </button>
        </fieldset>

        {Object.keys(settings).length > 0 && (
          <fieldset>
            <legend>Remove settings</legend>
            {Object.keys(settings).map((k) => (
              <div className="check" key={k}>
                <input
                  type="checkbox"
                  id={`unset-${k}`}
                  checked={unset.includes(k)}
                  onChange={(e) => setUnset(e.target.checked ? [...unset, k] : unset.filter((x) => x !== k))}
                />
                <label htmlFor={`unset-${k}`} className="mono">
                  Remove {k}
                </label>
              </div>
            ))}
          </fieldset>
        )}

        <fieldset>
          <legend>Set a secret</legend>
          <label htmlFor="secret-name">Secret name</label>
          <input
            id="secret-name"
            className="mono"
            value={secretName}
            autoComplete="off"
            autoCapitalize="characters"
            autoCorrect="off"
            spellCheck={false}
            onChange={(e) => setSecretName(e.target.value)}
          />
          <div className="check">
            <input
              type="radio"
              id="secret-how-value"
              name="secret-how"
              checked={secretHow === 'value'}
              onChange={() => setSecretHow('value')}
            />
            <label htmlFor="secret-how-value">A value the console keeps sealed</label>
          </div>
          <div className="check">
            <input
              type="radio"
              id="secret-how-file"
              name="secret-how"
              checked={secretHow === 'file'}
              onChange={() => setSecretHow('file')}
            />
            <label htmlFor="secret-how-file">A file already in the container</label>
          </div>
          {secretHow === 'value' ? (
            <>
              <label htmlFor="secret-value">Secret value</label>
              <input
                id="secret-value"
                type="password"
                className="mono"
                value={secretValue}
                autoComplete="off"
                onChange={(e) => setSecretValue(e.target.value)}
              />
              <p className="muted small">Write-only. Once applied, no screen shows it again.</p>
            </>
          ) : (
            <>
              <label htmlFor="secret-file">File path</label>
              <input
                id="secret-file"
                className="mono"
                value={secretFile}
                placeholder="/run/secrets/name"
                autoComplete="off"
                autoCapitalize="off"
                autoCorrect="off"
                spellCheck={false}
                onChange={(e) => setSecretFile(e.target.value)}
              />
            </>
          )}
          {secrets.length > 0 && (
            <>
              <p className="small">Remove a secret:</p>
              {secrets.map((x) => (
                <div className="check" key={x.name}>
                  <input
                    type="checkbox"
                    id={`unset-secret-${x.name}`}
                    checked={unsetSecrets.includes(x.name)}
                    onChange={(e) =>
                      setUnsetSecrets(
                        e.target.checked ? [...unsetSecrets, x.name] : unsetSecrets.filter((y) => y !== x.name),
                      )
                    }
                  />
                  <label htmlFor={`unset-secret-${x.name}`} className="mono">
                    Remove {x.name}
                  </label>
                </div>
              ))}
            </>
          )}
        </fieldset>

        <label htmlFor="reason">Reason, for the history</label>
        <input id="reason" value={reason} autoComplete="off" onChange={(e) => setReason(e.target.value)} />

        {preview.length > 0 && (
          <div className="preview">
            <p className="small">What changes:</p>
            <ul className="small">
              {preview.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          </div>
        )}
        {formError && (
          <p role="alert" className="error">
            {formError}
          </p>
        )}
        <div className="actions">
          <button type="submit" className="primary" disabled={busy || empty}>
            Apply
          </button>
          {empty && <span className="muted small">Nothing to apply yet.</span>}
        </div>
      </form>

      <h2>Revisions</h2>
      <p className="muted small">Each change makes a new revision. Rolling back applies an earlier one again.</p>
      {revs === null ? (
        <p className="muted">Loading…</p>
      ) : (
        <ol className="revisions" aria-label="Revisions">
          {revs.revisions.map((r) => (
            <li key={r.number} data-revision={r.number}>
              <div className="rev-head">
                <strong>Revision {r.number}</strong>
                {r.number === revs.live && <span className="tag">live</span>}
                <span className="muted small">
                  {r.outcome ? OUTCOME_WORDS[r.outcome] : 'Not decided yet'}
                </span>
              </div>
              <div className="muted small">
                {who(r.actor)}, {when(r.createdAt)}
                {r.reason ? `: ${r.reason}` : ''}
              </div>
              {r.detail && <div className="small detail">{r.detail}</div>}
              {r.outcome === 'good' && r.number !== revs.live && rollbackTo !== r.number && (
                <button type="button" disabled={busy} onClick={() => setRollbackTo(r.number)}>
                  Roll back to this
                </button>
              )}
              {rollbackTo === r.number && (
                <div className="confirm" role="group" aria-label={`Confirm roll back to revision ${r.number}`}>
                  <label htmlFor={`rollback-reason-${r.number}`}>Reason, for the history</label>
                  <input
                    id={`rollback-reason-${r.number}`}
                    value={rollbackReason}
                    autoComplete="off"
                    onChange={(e) => setRollbackReason(e.target.value)}
                  />
                  <div className="actions">
                    <button
                      type="button"
                      className="danger"
                      disabled={busy}
                      onClick={() => void rollback(r.number)}
                    >
                      Roll back to revision {r.number}
                    </button>
                    <button type="button" className="link" onClick={() => setRollbackTo(null)}>
                      Cancel
                    </button>
                  </div>
                </div>
              )}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

/** What a revision changed. Secrets by name only. */
function DiffList(props: { diff: Diff }) {
  const d = props.diff;
  const lines = [
    ...Object.entries(d.added).map(([k, v]) => `Added ${k} = "${v}"`),
    ...Object.entries(d.changed).map(([k, [a, b]]) => `Changed ${k} from "${a}" to "${b}"`),
    ...d.removed.map((k) => `Removed ${k}`),
    ...d.secretsChanged.map((k) => `Secret ${k} changed`),
  ];
  if (lines.length === 0) return <p className="small">No settings changed.</p>;
  return (
    <ul className="small" aria-label="What changed">
      {lines.map((l) => (
        <li key={l}>{l}</li>
      ))}
    </ul>
  );
}
