import { useCallback, useEffect, useState } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import { history, type History } from '../deploys';
import { link, usable, type Link } from '../deployment';
import { closeSoak, retryTenant, revertSoak, rollbackCheck, type RollbackCheck } from '../rollouts';
import { listTenants, type Tenant } from '../tenants';
import { useDeploymentTouch } from './useDeploymentTouch';
import { versionName, when } from './versionNames';
import './rollouts.css';

/**
 * Moves (docs/plan-versions.md 2.1, increments 7 and 11a): each tenant's
 * version and its open move, read from `std.deployment.tenants`. A refused
 * move is tried again; a move with dual writes ends in a soak, which can be
 * ended now or put back on the version it moved from, every write kept. The
 * dry run of a rollback says what returning to a version would do, and
 * changes nothing. Each refusal shows as the door gives it.
 */
export function Moves(props: { onSignedOut: () => void }) {
  const [state, setState] = useState<Link | null>(null);
  const [tenants, setTenants] = useState<Tenant[] | null>(null);
  const [past, setPast] = useState<History | null>(null);
  const [confirming, setConfirming] = useState<{ tenant: string; kind: 'close' | 'revert' } | null>(null);
  const [checkVersion, setCheckVersion] = useState('');
  const [check, setCheck] = useState<RollbackCheck | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const { atDeployment, prompt } = useDeploymentTouch();

  const load = useCallback(async () => {
    const s = await link();
    setState(s);
    if (!usable(s)) return;
    const [t, h] = await Promise.all([listTenants(), history()]);
    setTenants(t.tenants.filter((x) => x.status !== 'reaped'));
    setPast(h);
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
        if (e instanceof DOMException && e.name === 'AbortError') return;
        setError(explain(e));
      } finally {
        setBusy(false);
      }
    },
    [props],
  );

  useEffect(() => {
    // Once, on open.
    void act(load);
  }, []);

  const stored = past?.stored ?? [];
  const name = (v: string | null | undefined) => versionName(v, stored);
  const moving = (tenants ?? []).filter((t) => t.move);

  const retry = (t: Tenant) =>
    act(async () => {
      const r = await atDeployment(() => retryTenant(t.id));
      setDone(r.move ? `${t.name || t.id} moves to ${name(r.move.to)} again.` : `Trying ${t.name || t.id} again.`);
      await load();
    });

  const soak = (t: Tenant, kind: 'close' | 'revert') =>
    act(async () => {
      const r = await atDeployment(() => (kind === 'close' ? closeSoak(t.id) : revertSoak(t.id)));
      setConfirming(null);
      setDone(
        kind === 'close'
          ? `Ended the soak of ${t.name || t.id}. It stays on ${name(r.to)}.`
          : `Put ${t.name || t.id} back on ${name(r.from)}. Every write is kept.`,
      );
      await load();
    });

  const runCheck = () =>
    act(async () => {
      setCheck(null);
      setCheck(await rollbackCheck(checkVersion));
    });

  return (
    <section className="card rollouts">
      <h1>Moves</h1>
      <p className="muted small">
        A tenant with a database of its own keeps its version while that database is brought to the new one. Each open
        move shows here.
      </p>
      {state === null && !error && <p className="muted">Loading…</p>}
      {state && !usable(state) && (
        <p className="note">The console is not linked to the deployment yet. Link it on the Connection page first.</p>
      )}

      <div role="status">{done && <p className="ok">{done}</p>}</div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}

      {state && usable(state) && tenants && (
        <>
          <h2>Open moves</h2>
          {moving.length === 0 ? (
            <p className="muted">No tenant is moving.</p>
          ) : (
            <ol className="revisions" aria-label="Open moves">
              {moving.map((t) => {
                const m = t.move!;
                return (
                  <li key={t.id}>
                    <div className="rev-head">
                      <strong>{t.name || t.id}</strong>
                      <span className={`tag move-${m.phase}`}>{m.phase}</span>
                    </div>
                    <p className="small rollout-reason">{m.line}</p>
                    <div className="muted small">
                      From {name(m.from)} to {name(m.to)}, since {when(m.since)}.{' '}
                      {m.soakUntil ? <>Its soak ends {when(m.soakUntil)}. </> : null}
                      <span className="mono">{t.id}</span>
                    </div>
                    {confirming?.tenant !== t.id && (m.phase === 'refused' || m.phase === 'soaking') && (
                      <div className="row-actions">
                        {m.phase === 'refused' && (
                          <button type="button" disabled={busy} onClick={() => void retry(t)}>
                            Try the move again
                          </button>
                        )}
                        {m.phase === 'soaking' && (
                          <>
                            <button
                              type="button"
                              disabled={busy}
                              onClick={() => setConfirming({ tenant: t.id, kind: 'close' })}
                            >
                              End the soak now
                            </button>
                            <button
                              type="button"
                              disabled={busy}
                              onClick={() => setConfirming({ tenant: t.id, kind: 'revert' })}
                            >
                              Put it back on {name(m.from)}
                            </button>
                          </>
                        )}
                      </div>
                    )}
                    {confirming?.tenant === t.id && (
                      <div
                        className="confirm"
                        role="group"
                        aria-label={confirming.kind === 'close' ? `Confirm end the soak of ${t.id}` : `Confirm put ${t.id} back`}
                      >
                        <p>
                          {confirming.kind === 'close'
                            ? `End the soak now? ${t.name || t.id} stays on ${name(m.to)} and its old columns are left behind. It can no longer go back without losing writes.`
                            : `Put ${t.name || t.id} back on ${name(m.from)}? Every write made since the move is kept.`}
                        </p>
                        <div className="actions">
                          <button
                            type="button"
                            className="danger"
                            disabled={busy}
                            onClick={() => void soak(t, confirming.kind)}
                          >
                            {confirming.kind === 'close' ? 'End the soak' : 'Put it back'}
                          </button>
                          <button type="button" className="link" onClick={() => setConfirming(null)}>
                            Cancel
                          </button>
                        </div>
                      </div>
                    )}
                  </li>
                );
              })}
            </ol>
          )}

          <h2>Each tenant's version</h2>
          {tenants.length === 0 ? (
            <p className="muted">The deployment has no tenants.</p>
          ) : (
            <ul className="kv" aria-label="Tenant versions">
              {tenants.map((t) => (
                <li key={t.id}>
                  <span>
                    {t.name || t.id} <span className="mono muted small">{t.id}</span>
                  </span>
                  <span className="mono" title={t.version ?? ''}>
                    {t.version ? name(t.version) : 'no version yet'}
                    {t.move ? ` (moving to ${name(t.move.to)})` : ''}
                  </span>
                </li>
              ))}
            </ul>
          )}

          <h2 id="rollback-check-title">Dry run of a rollback</h2>
          <p className="muted small">
            What returning to a stored version would do now: the outside tables it reads, and each tenant not on it.
            Nothing changes. Roll back on the Deploys screen.
          </p>
          <form
            className="rollout-form"
            aria-labelledby="rollback-check-title"
            onSubmit={(e) => {
              e.preventDefault();
              void runCheck();
            }}
          >
            <label htmlFor="check-version">Version to return to</label>
            <select
              id="check-version"
              name="version"
              required
              value={checkVersion}
              onChange={(e) => {
                setCheckVersion(e.target.value);
                setCheck(null);
              }}
            >
              <option value="">Choose a stored version</option>
              {stored.map((v) => (
                <option key={v.version} value={v.version}>
                  {name(v.version)}
                  {v.serving ? ' (active)' : ''}
                </option>
              ))}
            </select>
            <div className="actions">
              <button type="submit" disabled={busy}>
                Check
              </button>
            </div>
          </form>
          {check && (
            <div className="rollback-check" role="region" aria-label="Rollback check">
              <p className={check.ok ? 'ok' : 'error'}>
                {check.ok
                  ? `A rollback to ${name(check.version)} would be allowed.`
                  : `A rollback to ${name(check.version)} would be refused.`}
              </p>
              {check.refusals.length > 0 && (
                <ul aria-label="Why it would be refused">
                  {check.refusals.map((r) => (
                    <li key={r} className="small">
                      {r}
                    </li>
                  ))}
                </ul>
              )}
              {check.notes.length > 0 && (
                <ul aria-label="Notes">
                  {check.notes.map((n) => (
                    <li key={n} className="small muted">
                      {n}
                    </li>
                  ))}
                </ul>
              )}
              {check.tenants.length === 0 ? (
                <p className="muted small">Every tenant is on it already.</p>
              ) : (
                <ul className="kv" aria-label="What happens to each tenant">
                  {check.tenants.map((t) => (
                    <li key={t.tenant}>
                      <span className="mono">{t.tenant}</span>
                      <span className="small">
                        {!t.kept && <span className="tag tag-revoked">loses writes</span>} {t.note}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </>
      )}
      {prompt}
    </section>
  );
}
