import { Fragment, useCallback, useEffect, useRef, useState } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import {
  setVersionSetting,
  versionSettings,
  type StoredVersion,
  type VersionSettingName,
  type VersionSettings as Settings,
} from '../deploys';

interface Row {
  name: VersionSettingName;
  key: 'served' | 'minimum' | 'newTenant';
  label: string;
  unset: string;
  hint: string;
}

const ROWS: Row[] = [
  {
    name: 'versions.served',
    key: 'served',
    label: 'Versions served',
    unset: 'in-use: every version a tenant is on',
    hint: 'in-use, or a comma list of versions (a name or #n) served beside the active one.',
  },
  {
    name: 'versions.minimum',
    key: 'minimum',
    label: 'Oldest version a tenant may stay on',
    unset: 'none',
    hint: 'A version, a name or #n. Tenants below it move in their maintenance window.',
  },
  {
    name: 'versions.new_tenant',
    key: 'newTenant',
    label: 'New tenants start on',
    unset: 'the active version',
    hint: 'A stored version that is not revoked, a name or #n.',
  },
];

function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/**
 * `versions.served`, `versions.minimum` and `versions.new_tenant`, stored in
 * Ordo (owner, 2026-11-05). Each is changed on its own, with a reason; the
 * deployment checks the change and its refusal names the setting and the
 * value. The deployment asks for its own touch only at
 * `ORDO_SECURITY_LEVEL=high`, which `atDeployment` takes when asked.
 */
export function VersionSettings(props: {
  stored: StoredVersion[];
  atDeployment: <T>(f: () => Promise<T>) => Promise<T>;
  onSignedOut: () => void;
  onChanged: () => void;
}) {
  const [now, setNow] = useState<Settings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Row | null>(null);
  const [value, setValue] = useState('');
  const [reason, setReason] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const form = useRef<HTMLFormElement>(null);

  const signedOut = (e: unknown) => e instanceof ConsoleCallError && SIGNED_OUT.has(e.code);

  const load = useCallback(async () => {
    try {
      setNow(await versionSettings());
      setLoadError(null);
    } catch (e) {
      if (signedOut(e)) return props.onSignedOut();
      setLoadError(explain(e));
    }
  }, [props]);

  useEffect(() => {
    void load();
  }, []);

  const open = (r: Row) => {
    setEditing(r);
    setValue(now?.[r.key] ?? '');
    setReason('');
    setError(null);
    setDone(null);
  };

  const save = async (r: Row, clear: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const v = clear ? null : value.trim() || null;
      const got = await props.atDeployment(() => setVersionSetting(r.name, v, reason.trim()));
      const shown = got.value ?? r.unset;
      setDone(got.changed ? `${r.label}: now ${shown}.` : `${r.label} was already ${shown}.`);
      setEditing(null);
      await load();
      props.onChanged();
    } catch (e) {
      if (signedOut(e)) return props.onSignedOut();
      setError(explain(e));
    } finally {
      setBusy(false);
    }
  };

  const errorId = 'version-setting-error';
  const listId = 'stored-version-names';

  return (
    <div role="region" aria-labelledby="version-settings-title">
      <h2 id="version-settings-title">Version settings</h2>
      <p className="muted small">
        Stored in Ordo, so every node reads the same value. A change is checked before it is kept, and kept with who
        made it and why.
      </p>
      {loadError && <p className="muted small">The version settings did not load: {loadError}</p>}
      {now && (
        <dl className="facts">
          {ROWS.map((r) => (
            <Fragment key={r.name}>
              <dt>{r.label}</dt>
              <dd>
                <span className="mono">{now[r.key] ?? r.unset}</span>{' '}
                {editing?.name !== r.name && (
                  <button
                    type="button"
                    className="link"
                    disabled={busy}
                    aria-label={`Change ${r.label.toLowerCase()}`}
                    onClick={() => open(r)}
                  >
                    Change
                  </button>
                )}
              </dd>
            </Fragment>
          ))}
        </dl>
      )}

      {editing && (
        <form
          ref={form}
          className="confirm"
          aria-label={`Change ${editing.label.toLowerCase()}`}
          onSubmit={(e) => {
            e.preventDefault();
            void save(editing, false);
          }}
        >
          <label htmlFor="version-setting-value">{editing.label}</label>
          <input
            id="version-setting-value"
            name="value"
            required
            maxLength={4096}
            list={editing.key === 'served' ? undefined : listId}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            aria-describedby={`version-setting-hint${error ? ` ${errorId}` : ''}`}
            aria-invalid={error ? true : undefined}
            value={value}
            onChange={(e) => {
              setValue(e.target.value);
              setError(null);
            }}
          />
          <span id="version-setting-hint" className="hint muted small">
            {editing.hint}
          </span>
          <datalist id={listId}>
            {props.stored
              .filter((v) => !v.revoked)
              .map((v) => (
                <option key={v.version} value={v.version}>
                  {v.ordinal ? `#${v.ordinal}` : ''}
                </option>
              ))}
          </datalist>
          <label htmlFor="version-setting-reason">Reason</label>
          <input
            id="version-setting-reason"
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
            <button
              type="button"
              disabled={busy}
              onClick={() => {
                // Clearing needs only the reason.
                const why = form.current?.elements.namedItem('reason') as HTMLInputElement | null;
                if (why && !why.reportValidity()) return;
                void save(editing, true);
              }}
            >
              Clear (back to {editing.unset.split(':')[0]})
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
          <summary>Setting changes ({now.history.length})</summary>
          <ol className="kv" aria-label="Version setting changes">
            {now.history.map((c, i) => (
              <li key={i}>
                <span className="mono">
                  {c.setting} = {c.value ?? '(cleared)'}
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
