import { useCallback, useEffect, useState } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import { link, type Link } from '../deployment';
import {
  activate,
  apply,
  history,
  preview,
  revoke,
  revokedInUse,
  unrevoke,
  untilServed,
  type History,
  type Preview,
  type RevokedInUse,
  type StoredVersion,
  type Serving,
  type Would,
} from '../deploys';
import { useDeploymentTouch } from './useDeploymentTouch';

function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** A version hash, short enough to read; the full one is in the title. */
function short(v: string): string {
  return v.length > 16 ? `${v.slice(0, 16)}…` : v;
}

/** `#3 v1.4 (abc1234)`: the order number, label and commit, where known. */
function named(v: StoredVersion): string {
  return [v.ordinal ? `#${v.ordinal}` : '', v.label ?? '', v.gitCommit ? `(${v.gitCommit.slice(0, 12)})` : '']
    .filter(Boolean)
    .join(' ');
}

function wouldWords(w: Would, p: Preview): string {
  switch (w.state) {
    case 'create':
      return `The database is empty. Applying creates the schema (${w.entities} entities) and makes this version the one that serves.`;
    case 'matches':
      return w.pending > 0
        ? `The database already holds this schema. ${w.pending} old column(s) or table(s) stay until a later change removes them.`
        : 'The database already holds this schema. Applying changes no table.';
    case 'migrate':
      return `The database is behind by safe changes only. Applying makes them for you (${p.safeChangesSetting} is on), then serves this version.`;
    case 'held':
      return p.safeChanges
        ? 'The database is behind by changes that need your say. Applying can store this version but not make it serve: run ordo migrate --apply first.'
        : `The database is behind. ${p.safeChangesSetting} is off, so applying can store this version but not make it serve. Set ${p.safeChangesSetting}=on, or run ordo migrate --apply first.`;
    case 'refused':
      return `The deployment would refuse it: ${w.message}`;
  }
}

/**
 * The app's program (docs/plan-console.md 4.3): choose a .ordo file,
 * preview what it does to the database, apply it, wait until every node
 * serves it, and roll the app back to a stored version. Revoke a version
 * with a reason, or lift the mark (docs/plan-versions.md 4, 2b), and see
 * which revoked versions still have tenants on them. Every change at the
 * deployment asks for the owner's own touch there.
 */
export function Deploys(props: { onSignedOut: () => void }) {
  const [state, setState] = useState<Link | null>(null);
  const [past, setPast] = useState<History | null>(null);
  const [file, setFile] = useState<{ name: string; text: string } | null>(null);
  const [planned, setPlanned] = useState<Preview | null>(null);
  const [activateNow, setActivateNow] = useState(true);
  const [rollbackTo, setRollbackTo] = useState<string | null>(null);
  const [inUse, setInUse] = useState<RevokedInUse[] | null>(null);
  const [inUseError, setInUseError] = useState<string | null>(null);
  const [marking, setMarking] = useState<{ version: string; lift: boolean } | null>(null);
  const [reason, setReason] = useState('');
  const [moveTo, setMoveTo] = useState('');
  const [waiting, setWaiting] = useState<{ version: string; seen: Serving | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const { atDeployment, prompt } = useDeploymentTouch();

  const load = useCallback(async () => {
    const s = await link();
    setState(s);
    if (!s.linked) return;
    setPast(await history());
    // The warning is best effort: the versions still show without it.
    try {
      setInUse((await revokedInUse()).versions.filter((v) => v.tenantCount > 0));
      setInUseError(null);
    } catch (e) {
      if (e instanceof ConsoleCallError && SIGNED_OUT.has(e.code)) throw e;
      setInUse(null);
      setInUseError(explain(e));
    }
  }, []);

  const act = useCallback(
    async (f: () => Promise<void>) => {
      setBusy(true);
      setError(null);
      setDone(null);
      try {
        await f();
      } catch (e) {
        if (e instanceof ConsoleCallError && SIGNED_OUT.has(e.code)) {
          props.onSignedOut();
          return;
        }
        setError(explain(e));
      } finally {
        setBusy(false);
        setWaiting(null);
      }
    },
    [props],
  );

  useEffect(() => {
    void act(load);
  }, []);

  /** Wait for every node to serve `version`, showing what each still does. */
  const waitFor = async (version: string) => {
    setWaiting({ version, seen: null });
    const s = await untilServed(version, (seen) => setWaiting({ version, seen }));
    return s.nodes.length;
  };

  const choose = async (f: File | undefined) => {
    setPlanned(null);
    setDone(null);
    setError(null);
    if (!f) {
      setFile(null);
      return;
    }
    setFile({ name: f.name, text: await f.text() });
  };

  const previewNow = () =>
    act(async () => {
      if (!file) return;
      const p = await atDeployment(() => preview(file.text));
      setPlanned(p);
      setActivateNow(p.would.state !== 'held');
    });

  const applyNow = () =>
    act(async () => {
      if (!file || !planned) return;
      const r = await atDeployment(() => apply(file.text, activateNow));
      if (r.activation.state === 'staged') {
        setDone(`Stored ${short(r.version)}. It does not serve yet; the app still runs the version it did.`);
      } else {
        const nodes = await waitFor(r.version);
        setDone(
          nodes === 0
            ? `Applied ${short(r.version)}. No node is running; the first one to start serves it.`
            : `Applied ${short(r.version)}. Every node serves it (${nodes}).`,
        );
      }
      setPlanned(null);
      await load();
    });

  const rollBack = (version: string) =>
    act(async () => {
      const r = await atDeployment(() => activate(version));
      setRollbackTo(null);
      if (r.state === 'alreadyActive') {
        setDone(`${short(version)} already serves.`);
      } else {
        const nodes = await waitFor(version);
        setDone(`Rolled the app back to ${short(version)}. ${nodes === 0 ? 'No node is running.' : `Every node serves it (${nodes}).`}`);
      }
      await load();
    });

  const openMark = (version: string, lift: boolean) => {
    setMarking({ version, lift });
    setReason('');
    setMoveTo('');
    setRollbackTo(null);
  };

  const mark = (v: StoredVersion, lift: boolean) =>
    act(async () => {
      if (lift) {
        await atDeployment(() => unrevoke(v.version, reason.trim()));
        setDone(`Lifted the revoke on ${short(v.version)}.`);
      } else {
        const r = await atDeployment(() => revoke(v.version, reason.trim(), v.serving ? moveTo : null));
        setDone(
          r.moved
            ? `Revoked ${short(v.version)}. The deployment moved to ${short(r.moved)}.`
            : `Revoked ${short(v.version)}.`,
        );
      }
      setMarking(null);
      await load();
    });

  const refused = planned?.would.state === 'refused';
  const held = planned?.would.state === 'held';

  return (
    <section className="card">
      <h1>Deploys</h1>
      <p className="muted small">
        The app's program: its schema and ops. Rolling back the app here is a different act from rolling back the
        server's settings on the Runtime screen.
      </p>
      {state === null && !error && <p className="muted">Loading…</p>}

      {state && !state.linked && (
        <p className="note">The console is not linked to the deployment yet. Link it on the Deployment screen first.</p>
      )}

      {state?.linked && (
        <>
          <h2>Apply a program</h2>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void previewNow();
            }}
          >
            <label htmlFor="program-file">Program (.ordo file)</label>
            <input
              id="program-file"
              type="file"
              accept=".ordo,text/plain"
              disabled={busy}
              onChange={(e) => void choose(e.target.files?.[0])}
            />
            {file && (
              <p className="muted small">
                {file.name}, {file.text.length} characters
              </p>
            )}
            <div className="actions">
              <button type="submit" className={planned ? '' : 'primary'} disabled={busy || !file}>
                Preview
              </button>
            </div>
          </form>

          {planned && (
            <div className="preview" role="region" aria-label="Preview">
              <h3>What applying does</h3>
              <dl className="facts">
                <dt>Version</dt>
                <dd className="mono" title={planned.version}>
                  {short(planned.version)}
                  {planned.alreadyStored && <span className="tag">stored already</span>}
                </dd>
                <dt>Serving now</dt>
                <dd className="mono">{planned.serving ? short(planned.serving) : 'nothing yet'}</dd>
              </dl>
              <p className={refused ? 'error' : ''}>{wouldWords(planned.would, planned)}</p>
              {planned.changes.length > 0 && (
                <ul aria-label="Schema changes">
                  {planned.changes.map((c, i) => (
                    <li key={i}>
                      <span className="mono small">{c.code}</span> {c.change}{' '}
                      <span className="muted small">class {c.class}</span>
                      {c.refused.length > 0 && <span className="error small"> refused: {c.refused.join(', ')}</span>}
                    </li>
                  ))}
                </ul>
              )}
              {planned.notes.map((n, i) => (
                <p key={i} className="muted small">
                  {n}
                </p>
              ))}
              {!refused && (
                <>
                  <div className="check">
                    <input
                      type="checkbox"
                      id="activate-now"
                      checked={activateNow}
                      disabled={held}
                      onChange={(e) => setActivateNow(e.target.checked)}
                    />
                    <label htmlFor="activate-now">Make it serve now (otherwise it is only stored)</label>
                  </div>
                  <div className="actions">
                    <button type="button" className="primary" disabled={busy} onClick={() => void applyNow()}>
                      {activateNow ? 'Apply' : 'Store only'}
                    </button>
                    <button type="button" className="link" disabled={busy} onClick={() => setPlanned(null)}>
                      Cancel
                    </button>
                  </div>
                </>
              )}
            </div>
          )}

          <div role="status" className="progress">
            {waiting && (
              <>
                <p>
                  Waiting for every node to serve {short(waiting.version)}
                  {waiting.seen
                    ? `: ${waiting.seen.nodes.length - waiting.seen.pending.length} of ${waiting.seen.nodes.length} done.`
                    : '.'}
                </p>
                <progress
                  aria-label="Nodes serving the new version"
                  {...(waiting.seen && waiting.seen.nodes.length > 0
                    ? {
                        max: waiting.seen.nodes.length,
                        value: waiting.seen.nodes.length - waiting.seen.pending.length,
                      }
                    : {})}
                />
              </>
            )}
            {done && <p className="ok">{done}</p>}
          </div>
          {error && (
            <p role="alert" className="error">
              {error}
            </p>
          )}

          {inUseError && (
            <p className="muted small">The list of revoked versions in use did not load: {inUseError}</p>
          )}
          {inUse && inUse.length > 0 && (
            <div className="note revoked-warning" role="region" aria-label="Revoked versions in use">
              <h3>Revoked versions still in use</h3>
              <ul>
                {inUse.map((r) => (
                  <li key={r.version}>
                    <span className="mono" title={r.version}>
                      {r.ordinal ? `#${r.ordinal} ` : ''}
                      {short(r.version)}
                    </span>
                    : {r.tenantCount} tenant(s) on it. Revoked: {r.reason}
                    {r.tenants.length > 0 && (
                      <span className="muted small"> ({r.tenants.map((t) => t.name || t.space).join(', ')})</span>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}

          <h2>Versions</h2>
          {past === null ? (
            <p className="muted">Loading…</p>
          ) : past.stored.length === 0 ? (
            <p className="muted">No program is stored yet.</p>
          ) : (
            <ol className="revisions" aria-label="Stored versions">
              {past.stored.map((v) => (
                <li key={v.version}>
                  <div className="rev-head">
                    <strong className="mono" title={v.version}>
                      {short(v.version)}
                    </strong>
                    {named(v) && <span>{named(v)}</span>}
                    {v.serving && <span className="tag">serving</span>}
                    {v.revoked && <span className="tag tag-revoked">revoked</span>}
                  </div>
                  <div className="muted small">
                    Applied {when(v.appliedAt)} by {v.actor}
                  </div>
                  {v.revoked && (
                    <div className="small error">
                      Revoked {when(v.revoked.at)} by {v.revoked.actor}: {v.revoked.reason}
                    </div>
                  )}
                  {marking?.version !== v.version && rollbackTo !== v.version && (
                    <div className="row-buttons">
                      {!v.serving && !v.revoked && (
                        <button type="button" disabled={busy} onClick={() => setRollbackTo(v.version)}>
                          Roll back the app to this
                        </button>
                      )}
                      {v.revoked ? (
                        <button type="button" disabled={busy} onClick={() => openMark(v.version, true)}>
                          Lift the revoke
                        </button>
                      ) : (
                        <button type="button" disabled={busy} onClick={() => openMark(v.version, false)}>
                          Revoke
                        </button>
                      )}
                    </div>
                  )}
                  {marking?.version === v.version && (
                    <form
                      className="confirm"
                      aria-label={`${marking.lift ? 'Lift the revoke on' : 'Revoke'} ${short(v.version)}`}
                      onSubmit={(e) => {
                        e.preventDefault();
                        void mark(v, marking.lift);
                      }}
                    >
                      <p>
                        {marking.lift
                          ? `Lift the revoke on ${short(v.version)}? It can serve again.`
                          : `Revoke ${short(v.version)}? It will not serve again until the revoke is lifted.`}
                      </p>
                      {!marking.lift && v.serving && (
                        <>
                          <label htmlFor={`move-to-${v.version}`}>It serves now. Move to</label>
                          <select
                            id={`move-to-${v.version}`}
                            name="moveTo"
                            required
                            value={moveTo}
                            onChange={(e) => setMoveTo(e.target.value)}
                          >
                            <option value="">Choose a version</option>
                            {past.stored
                              .filter((o) => o.version !== v.version && !o.revoked)
                              .map((o) => (
                                <option key={o.version} value={o.version}>
                                  {[named(o), short(o.version)].filter(Boolean).join(' ')}
                                </option>
                              ))}
                          </select>
                        </>
                      )}
                      <label htmlFor={`reason-${v.version}`}>Reason</label>
                      <input
                        id={`reason-${v.version}`}
                        name="reason"
                        required
                        maxLength={2048}
                        value={reason}
                        onChange={(e) => setReason(e.target.value)}
                      />
                      <div className="actions">
                        <button type="submit" className={marking.lift ? 'primary' : 'danger'} disabled={busy}>
                          {marking.lift ? 'Lift the revoke' : 'Revoke'}
                        </button>
                        <button type="button" className="link" onClick={() => setMarking(null)}>
                          Cancel
                        </button>
                      </div>
                    </form>
                  )}
                  {rollbackTo === v.version && (
                    <div className="confirm" role="group" aria-label={`Confirm roll back the app to ${short(v.version)}`}>
                      <p>
                        Make {short(v.version)} the app's program again? Every node switches to it. The data stays; a
                        column only the newer version uses stays in the database.
                      </p>
                      <div className="actions">
                        <button type="button" className="danger" disabled={busy} onClick={() => void rollBack(v.version)}>
                          Roll back the app
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
          {past && past.activations.length > 0 && (
            <details className="logs">
              <summary>Activation history ({past.activations.length})</summary>
              <ol className="kv" aria-label="Activations">
                {past.activations.map((a) => (
                  <li key={a.seq}>
                    <span className="mono" title={a.version}>
                      {short(a.version)}
                    </span>{' '}
                    <span className="muted small">
                      {when(a.at)} by {a.actor}
                    </span>
                  </li>
                ))}
              </ol>
            </details>
          )}
        </>
      )}
      {prompt}
    </section>
  );
}
