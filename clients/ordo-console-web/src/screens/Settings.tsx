import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { ConsoleCallError, explain, post, SIGNED_OUT, type Applied, type Patch } from '../api';
import {
  loadSettings,
  plain,
  SIGN_IN_GROUPS,
  SOURCE_WORDS,
  type SettingRow,
  type SettingsView,
} from '../settings';
import { withTouch } from '../touch';
import { when } from './runtimeParts';
import './settings.css';

type Scope = 'signin' | 'all';

/** What the last change ended in. */
type Outcome =
  | { kind: 'applied'; revision: number; name: string }
  | { kind: 'refused'; message: string }
  | { kind: 'rolled-back'; message: string }
  | { kind: 'error'; message: string };

interface Editing {
  row: SettingRow;
  value: string;
  remove: boolean;
  reason: string;
}

const DRIFT_WORDS = {
  'not-taken': 'Not taken',
  stored: 'Stored value wins',
  outside: 'Set outside the console',
} as const;

/**
 * The Sign-in settings screen (docs/plan-console.md 4.6) and the Every
 * setting screen (4.10): each setting as the running deployment took it,
 * its source and default, what the console's revision gives it, and drift
 * where the two differ. A change is a new runtime revision: the deployment
 * restarts with it, and the console rolls back if it does not come up.
 */
export function Settings(props: { scope: Scope; onSignedOut: () => void }) {
  const [data, setData] = useState<SettingsView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [filter, setFilter] = useState('');
  const [onlySet, setOnlySet] = useState(props.scope === 'all');
  const [editing, setEditing] = useState<Editing | null>(null);
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const signedOut = useRef(props.onSignedOut);
  signedOut.current = props.onSignedOut;
  const ids = useId();

  const refresh = useCallback(async () => {
    try {
      setData(await loadSettings());
      setLoadError(null);
    } catch (e) {
      if (e instanceof ConsoleCallError && SIGNED_OUT.has(e.code)) {
        signedOut.current();
        return;
      }
      setLoadError(explain(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const heading = props.scope === 'signin' ? 'Sign-in settings' : 'Every setting';

  if (data === null) {
    return (
      <section className="card">
        <h1>{heading}</h1>
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

  const inScope = data.rows.filter((r) => props.scope === 'all' || SIGN_IN_GROUPS.includes(r.group));
  const needle = filter.trim().toLowerCase();
  const shown = inScope.filter((r) => {
    if (needle && !`${r.name} ${r.group} ${r.value ?? ''}`.toLowerCase().includes(needle)) return false;
    if (onlySet && !needle && !(r.set || r.console?.set || r.drift)) return false;
    return true;
  });
  const groups: string[] = [];
  for (const r of shown) if (!groups.includes(r.group)) groups.push(r.group);
  const drifted = inScope.filter((r) => r.drift).length;

  const startEdit = (row: SettingRow) => {
    setOutcome(null);
    setEditing({ row, value: row.secret ? '' : (row.console?.value ?? row.value ?? ''), remove: false, reason: '' });
  };

  const apply = async (e: Editing) => {
    const name = e.row.name;
    const sealed = e.row.sealed ?? false;
    const patch: Patch = { set: {}, unset: [], secrets: {}, reason: e.reason.trim() };
    if (sealed) patch.secrets[name] = e.remove ? null : { value: e.value };
    else if (e.remove) patch.unset.push(name);
    else patch.set[name] = e.value;
    setBusy(true);
    setOutcome(null);
    try {
      const done = await withTouch(() => post<Applied>('runtime/apply', patch));
      setOutcome({ kind: 'applied', revision: done.revision, name });
      setEditing(null);
    } catch (err) {
      if (err instanceof ConsoleCallError && SIGNED_OUT.has(err.code)) {
        signedOut.current();
        return;
      }
      if (err instanceof ConsoleCallError && err.code === 'settings_refused') {
        setOutcome({ kind: 'refused', message: err.message });
      } else if (err instanceof ConsoleCallError && err.code === 'apply_rolled_back') {
        setOutcome({ kind: 'rolled-back', message: err.message });
      } else {
        setOutcome({ kind: 'error', message: explain(err) });
      }
    } finally {
      setBusy(false);
      void refresh();
    }
  };

  return (
    <section className="card settings">
      <h1>{heading}</h1>
      <p>
        {props.scope === 'signin'
          ? 'How people sign in to the deployment: sessions, admission, passkeys and the app developer’s stored layers.'
          : 'Every setting the deployment reads.'}{' '}
        Each shows the value the running deployment took and where it came from. Secrets show only whether they are
        set.
      </p>
      {data.editable ? (
        <p className="muted small">
          A change files revision {(data.revision ?? 0) + 1} and restarts the deployment with it. If it does not come
          up, the console rolls back. Each change needs a touch of your passkey.
        </p>
      ) : (
        <p className="note">
          The console does not run the deployment here, so these settings are read-only. Change them where the
          deployment runs.
        </p>
      )}
      {data.doorError && (
        <p role="alert" className="error">
          The deployment did not answer, so this shows the console’s revision only: {data.doorError}
        </p>
      )}
      {drifted > 0 && (
        <p className="drift-summary">
          {drifted === 1 ? '1 setting differs' : `${drifted} settings differ`} between the console’s revision and the
          running deployment.
        </p>
      )}

      <div className="settings-find">
        <label htmlFor={`${ids}-find`}>Find a setting</label>
        <input
          id={`${ids}-find`}
          type="search"
          autoComplete="off"
          spellCheck={false}
          placeholder="ORDO_SESSION_SECS"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <div className="check">
          <input id={`${ids}-only`} type="checkbox" checked={onlySet} onChange={(e) => setOnlySet(e.target.checked)} />
          <label htmlFor={`${ids}-only`}>Only settings that are set or differ</label>
        </div>
      </div>

      <div role="status" className="progress">
        {busy && <p>Applying. The deployment restarts; this can take up to about 150 seconds.</p>}
        {outcome?.kind === 'applied' && (
          <p className="ok">
            Revision {outcome.revision} is live and ready with the new {outcome.name}.
          </p>
        )}
      </div>
      {outcome && outcome.kind !== 'applied' && (
        <div role="alert" className="error">
          <p>
            {outcome.kind === 'refused'
              ? 'The settings check refused the change. Nothing running was touched.'
              : outcome.kind === 'rolled-back'
                ? 'The deployment did not come up with the change, so the console rolled back.'
                : 'The change was not made.'}
          </p>
          <pre className="mono small">{outcome.message}</pre>
        </div>
      )}

      {groups.length === 0 && <p className="muted">No setting matches.</p>}
      {groups.map((g) => {
        const rows = shown.filter((r) => r.group === g);
        const differ = rows.filter((r) => r.drift).length;
        return (
          <details key={g} className="settings-group" open={needle !== '' || differ > 0 || props.scope === 'signin'}>
            <summary>
              {g} <span className="muted small">({rows.length})</span>
              {differ > 0 && <span className="tag tag-drift">{differ} differ</span>}
            </summary>
            <div className="settings-table" role="region" aria-label={g} tabIndex={0}>
              <table>
                <thead>
                  <tr>
                    <th scope="col">Setting</th>
                    <th scope="col">Running value</th>
                    <th scope="col">Source</th>
                    <th scope="col">Default</th>
                    <th scope="col">Console revision</th>
                    <th scope="col">
                      <span className="visually-hidden">Actions</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((r) => (
                    <SettingLine
                      key={r.name}
                      row={r}
                      editable={data.editable && r.kind === 'setting'}
                      editing={editing?.row.name === r.name ? editing : null}
                      busy={busy}
                      onEdit={() => startEdit(r)}
                      onChange={setEditing}
                      onCancel={() => setEditing(null)}
                      onApply={(e) => void apply(e)}
                      next={(data.revision ?? 0) + 1}
                    />
                  ))}
                </tbody>
              </table>
            </div>
          </details>
        );
      })}
    </section>
  );
}

function runningValue(r: SettingRow): string {
  if (r.set === null) return 'Not known';
  if (r.secret) return r.set ? 'Set' : 'Unset';
  if (r.source === 'default' || r.value === null || r.value === undefined) return '';
  return r.value;
}

function consoleWords(r: SettingRow): string {
  const c = r.console;
  if (!c) return '';
  if (!c.set) return 'Not set';
  if (c.secret) return `Secret, ${c.secret === 'managed' ? 'sealed by the console' : 'a mounted file'}`;
  return c.value ?? 'Set';
}

function SettingLine(props: {
  row: SettingRow;
  editable: boolean;
  editing: Editing | null;
  busy: boolean;
  next: number;
  onEdit: () => void;
  onChange: (e: Editing) => void;
  onCancel: () => void;
  onApply: (e: Editing) => void;
}) {
  const r = props.row;
  const e = props.editing;
  const ids = useId();
  const value = runningValue(r);
  const sealed = r.sealed ?? false;
  const preview = e
    ? e.remove
      ? `Revision ${props.next}: remove ${r.name}, so its default applies.`
      : sealed
        ? `Revision ${props.next}: set the secret ${r.name}. The console seals it; it is never shown again.`
        : r.console?.set
          ? `Revision ${props.next}: change ${r.name} from "${r.console.value ?? ''}" to "${e.value}".`
          : `Revision ${props.next}: set ${r.name} to "${e.value}".`
    : '';
  return (
    <>
      <tr data-setting={r.name} data-drift={r.drift ? r.drift.kind : undefined}>
        <th scope="row">
          <span className="setting-name mono">{r.name}</span>
          {r.secret && <span className="tag">secret</span>}
          {r.family && <span className="muted small setting-family">{plain(r.family)}</span>}
          {r.what && <span className="muted small setting-family">{r.what}</span>}
          {!r.listed && <span className="muted small setting-family">Not a setting the deployment lists</span>}
        </th>
        <td data-label="Running value">
          {value === '' ? <span className="muted">Default</span> : <span className="mono setting-value">{value}</span>}
          {r.source === 'stored' && r.stored && (
            <span className="muted small setting-family">
              Stored by {r.stored.op}, {when(r.stored.updatedAt)}.
              {r.environment != null && ` The environment says ${r.environment}, which does not win.`}
            </span>
          )}
        </td>
        <td data-label="Source">
          {r.source ? SOURCE_WORDS[r.source] : <span className="muted">Not known</span>}
          {r.var && r.source === 'file' && <span className="muted small setting-family mono">{r.var}</span>}
        </td>
        <td data-label="Default">
          {r.default ? plain(r.default) : <span className="muted">None</span>}
          {r.range && <span className="muted small setting-family">Takes {plain(r.range)}</span>}
        </td>
        <td data-label="Console revision">
          {r.console ? consoleWords(r) : <span className="muted">No revision</span>}
          {r.drift && (
            <span className="drift setting-family">
              <strong>{DRIFT_WORDS[r.drift.kind]}.</strong> {r.drift.message}
            </span>
          )}
        </td>
        <td data-label="Actions">
          {props.editable && !e && (
            <button type="button" disabled={props.busy} onClick={props.onEdit} aria-label={`Change ${r.name}`}>
              Change
            </button>
          )}
        </td>
      </tr>
      {e && (
        <tr className="setting-edit">
          <td colSpan={6}>
            <form
              aria-label={`Change ${r.name}`}
              onSubmit={(ev) => {
                ev.preventDefault();
                props.onApply(e);
              }}
            >
              {!e.remove && (
                <>
                  <label htmlFor={`${ids}-value`}>New value for {r.name}</label>
                  <input
                    id={`${ids}-value`}
                    className="mono"
                    type={sealed ? 'password' : 'text'}
                    autoComplete={sealed ? 'new-password' : 'off'}
                    spellCheck={false}
                    required={sealed}
                    value={e.value}
                    onChange={(ev) => props.onChange({ ...e, value: ev.target.value })}
                  />
                  {r.range && <p className="muted small">Takes {plain(r.range)}.</p>}
                </>
              )}
              {r.console?.set && (
                <div className="check">
                  <input
                    id={`${ids}-remove`}
                    type="checkbox"
                    checked={e.remove}
                    onChange={(ev) => props.onChange({ ...e, remove: ev.target.checked })}
                  />
                  <label htmlFor={`${ids}-remove`}>Remove it from the revision</label>
                </div>
              )}
              <label htmlFor={`${ids}-reason`}>Why (kept in the revision history)</label>
              <input
                id={`${ids}-reason`}
                maxLength={200}
                value={e.reason}
                onChange={(ev) => props.onChange({ ...e, reason: ev.target.value })}
              />
              <p className="setting-preview">{preview}</p>
              {r.source === 'stored' && r.storedBy && (
                <p className="note small">
                  The deployment keeps the value {r.storedBy} stored. This setting only seeds it on a first boot.
                </p>
              )}
              <div className="actions">
                <button type="submit" className="primary" disabled={props.busy}>
                  Apply and restart
                </button>
                <button type="button" className="link" onClick={props.onCancel}>
                  Cancel
                </button>
              </div>
            </form>
          </td>
        </tr>
      )}
    </>
  );
}
