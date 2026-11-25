import { useCallback, useEffect, useState } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import { history, type History } from '../deploys';
import { link, usable, type Link } from '../deployment';
import {
  abortRollout,
  finishRollout,
  holdTenant,
  resumeRollout,
  retryTenant,
  rolloutStatus,
  secsWords,
  skipTenant,
  startRollout,
  STRATEGIES,
  unholdTenant,
  type Rollout,
  type RolloutTenant,
  type StartInput,
  type Strategy,
  type TenantState,
} from '../rollouts';
import { useDeploymentTouch } from './useDeploymentTouch';
import { versionName, when } from './versionNames';
import './rollouts.css';

const STRATEGY_WORDS: Record<Strategy, string> = {
  rolling: 'Rolling: a batch every interval',
  all: 'All: every tenant in one batch',
  canary: 'Canary: a few first, judged on health, then the rest',
  group: 'Group: the tenants you name first, then the rest',
  query: 'Query: the tenants a condition picks first, then the rest',
};

const STATES: TenantState[] = ['moving', 'waiting', 'failed', 'queued', 'moved', 'skipped'];

/** The start form's fields, as typed. Blank takes the deployment's setting. */
interface Draft {
  version: string;
  strategy: '' | Strategy;
  batchInterval: string;
  batchSize: string;
  window: string;
  failureThreshold: string;
  canaryCount: string;
  canaryWait: string;
  healthErrorRate: string;
  group: string;
  query: string;
  order: string;
  limit: string;
}

const EMPTY: Draft = {
  version: '',
  strategy: '',
  batchInterval: '',
  batchSize: '',
  window: '',
  failureThreshold: '',
  canaryCount: '',
  canaryWait: '',
  healthErrorRate: '',
  group: '',
  query: '',
  order: '',
  limit: '',
};

function input(d: Draft): StartInput {
  const text = (v: string) => (v.trim() ? v.trim() : undefined);
  const num = (v: string) => (v.trim() ? Number(v) : undefined);
  const s: StartInput = { version: d.version };
  const add = <K extends keyof StartInput>(k: K, v: StartInput[K] | undefined) => {
    if (v !== undefined && !(typeof v === 'number' && Number.isNaN(v))) s[k] = v;
  };
  add('strategy', d.strategy || undefined);
  add('batchInterval', text(d.batchInterval));
  add('batchSize', num(d.batchSize));
  add('window', text(d.window));
  add('failureThreshold', num(d.failureThreshold));
  add('canaryCount', num(d.canaryCount));
  add('canaryWait', text(d.canaryWait));
  add('healthErrorRate', text(d.healthErrorRate));
  add('group', text(d.group));
  add('query', text(d.query));
  add('order', text(d.order));
  add('limit', num(d.limit));
  return s;
}

/**
 * Rollouts (docs/plan-versions.md 3.7, increments 10 and 10c): where the
 * open or newest rollout stands, its tenants and its health; start one with
 * a strategy; resume, finish or abort it; skip, retry or hold one tenant.
 * Every control is a door op. A rollout's controls are moves and ask no
 * touch at the deployment (owner, 2026-11-05). Each refusal shows as the
 * door gives it: it names the setting or command that fixes it.
 */
export function Rollouts(props: { onSignedOut: () => void }) {
  const [state, setState] = useState<Link | null>(null);
  const [rollout, setRollout] = useState<Rollout | null>(null);
  const [past, setPast] = useState<History | null>(null);
  const [draft, setDraft] = useState<Draft>(EMPTY);
  const [closing, setClosing] = useState<'abort' | 'finish' | null>(null);
  const [closeReason, setCloseReason] = useState('');
  const [holding, setHolding] = useState<string | null>(null);
  const [holdReason, setHoldReason] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const { atDeployment, prompt } = useDeploymentTouch();

  const load = useCallback(async () => {
    const s = await link();
    setState(s);
    if (!usable(s)) return;
    const [r, h] = await Promise.all([rolloutStatus(), history()]);
    setRollout(r);
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
  const open = rollout?.id != null && (rollout.state === 'running' || rollout.state === 'paused');

  const start = () =>
    act(async () => {
      const r = await atDeployment(() => startRollout(input(draft)));
      setDone(`Started rollout #${r.id} to ${name(r.target)}.`);
      setDraft(EMPTY);
      await load();
    });

  const control = (f: () => Promise<unknown>, words: string) =>
    act(async () => {
      await atDeployment(f);
      setDone(words);
      setClosing(null);
      setCloseReason('');
      setHolding(null);
      setHoldReason('');
      await load();
    });

  const refresh = () =>
    act(async () => {
      await load();
    });

  const tenantRow = (t: RolloutTenant) => {
    const held = t.state === 'waiting' && t.reason.startsWith('on hold');
    const pending = t.state === 'queued' || t.state === 'waiting' || t.state === 'failed';
    return (
      <li key={t.tenant}>
        <div className="rev-head">
          <strong className="mono">{t.tenant}</strong>
          <span className={`tag state-${t.state}`}>{t.state}</span>
          {t.firstStage && <span className="tag">first stage</span>}
        </div>
        {t.reason && <p className="small rollout-reason">{t.reason}</p>}
        {open && holding !== t.tenant && (pending || held) && (
          <div className="row-actions">
            {t.state === 'failed' && (
              <button
                type="button"
                disabled={busy}
                onClick={() => void control(() => retryTenant(t.tenant), `Trying ${t.tenant} again.`)}
              >
                Try again
              </button>
            )}
            {pending && (
              <button
                type="button"
                disabled={busy}
                onClick={() => void control(() => skipTenant(t.tenant, ''), `Skipped ${t.tenant}. It stays where it is.`)}
              >
                Skip
              </button>
            )}
            {held ? (
              <button
                type="button"
                disabled={busy}
                onClick={() => void control(() => unholdTenant(t.tenant), `Lifted the hold on ${t.tenant}.`)}
              >
                Lift the hold
              </button>
            ) : (
              <button
                type="button"
                disabled={busy}
                onClick={() => {
                  setHolding(t.tenant);
                  setHoldReason('');
                }}
              >
                Hold
              </button>
            )}
          </div>
        )}
        {holding === t.tenant && (
          <form
            className="confirm"
            aria-label={`Hold ${t.tenant}`}
            onSubmit={(e) => {
              e.preventDefault();
              void control(() => holdTenant(t.tenant, holdReason.trim()), `Holding ${t.tenant}.`);
            }}
          >
            <p className="small">A held tenant waits in every rollout until the hold is lifted.</p>
            <label htmlFor={`hold-reason-${t.tenant}`}>Reason</label>
            <input
              id={`hold-reason-${t.tenant}`}
              name="reason"
              required
              value={holdReason}
              onChange={(e) => setHoldReason(e.target.value)}
            />
            <div className="actions">
              <button type="submit" className="primary" disabled={busy}>
                Hold this tenant
              </button>
              <button type="button" className="link" onClick={() => setHolding(null)}>
                Cancel
              </button>
            </div>
          </form>
        )}
      </li>
    );
  };

  return (
    <section className="card rollouts">
      <h1>Rollouts</h1>
      <p className="muted small">
        A rollout moves tenants to a stored version in batches. One runs at a time. Store a version on the Deploys
        screen first, without making it active.
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

      {state && usable(state) && rollout && (
        <>
          <div className="heading-row">
            <h2>Where it stands</h2>
            <button type="button" className="link" disabled={busy} onClick={() => void refresh()}>
              Refresh
            </button>
          </div>
          {rollout.id == null ? (
            <p className="muted">No rollout has run yet.</p>
          ) : (
            <div className="rollout" role="region" aria-label={`Rollout #${rollout.id}`}>
              <div className="rev-head">
                <strong>
                  Rollout #{rollout.id} to {name(rollout.target)}
                </strong>
                <span className={`tag rollout-${rollout.state}`}>{rollout.state}</span>
              </div>
              {rollout.reason && (
                <p className={rollout.state === 'paused' ? 'note small' : 'small'}>
                  {rollout.state === 'paused' ? 'Paused: ' : ''}
                  {rollout.reason}
                </p>
              )}
              <dl className="facts">
                <dt>From</dt>
                <dd>{name(rollout.from)}</dd>
                <dt>Strategy</dt>
                <dd>
                  {rollout.strategy}
                  {rollout.selection ? ` (${rollout.selection})` : ''}
                </dd>
                <dt>Batches</dt>
                <dd>
                  {rollout.batches ?? 0} so far, {(rollout.batchSize ?? 0) > 1_000_000 ? 'all at once' : `${rollout.batchSize} per batch`}, every{' '}
                  {secsWords(rollout.batchInterval)}
                </dd>
                <dt>Window</dt>
                <dd>{secsWords(rollout.window)}</dd>
                <dt>Pauses after</dt>
                <dd>{rollout.failureThreshold} failed</dd>
                <dt>Deployment's own space</dt>
                <dd>
                  {rollout.deploymentSpace}
                  {rollout.deploymentStep ? `: ${rollout.deploymentStep.state}` : ''}
                  {rollout.deploymentStep?.reason ? ` (${rollout.deploymentStep.reason})` : ''}
                </dd>
                {rollout.gate && (
                  <>
                    <dt>First stage</dt>
                    <dd>
                      {rollout.gate === 'passed' ? 'passed' : `judged for ${secsWords(rollout.canaryWait)} after it moves`}
                    </dd>
                  </>
                )}
                {rollout.health && rollout.gate && (
                  <>
                    <dt>Health</dt>
                    <dd>
                      {rollout.health.healthy ? 'healthy' : 'unhealthy'}: {rollout.health.faults} server faults in{' '}
                      {rollout.health.calls} calls (pauses at {rollout.healthErrorRate})
                    </dd>
                  </>
                )}
                <dt>Started</dt>
                <dd>
                  {rollout.startedAt ? when(rollout.startedAt) : ''} by {rollout.actor}
                </dd>
                {rollout.endedAt ? (
                  <>
                    <dt>Ended</dt>
                    <dd>{when(rollout.endedAt)}</dd>
                  </>
                ) : null}
              </dl>

              <ul className="counts" aria-label="Tenants by state">
                {STATES.map((s) => (
                  <li key={s}>
                    <span className="count">{rollout.counts?.[s] ?? 0}</span> {s}
                  </li>
                ))}
              </ul>

              {open && closing === null && (
                <div className="row-actions">
                  {rollout.state === 'paused' && (
                    <button
                      type="button"
                      className="primary"
                      disabled={busy}
                      onClick={() => void control(resumeRollout, `Resumed rollout #${rollout.id}.`)}
                    >
                      Resume
                    </button>
                  )}
                  <button type="button" disabled={busy} onClick={() => setClosing('finish')}>
                    Finish now
                  </button>
                  <button type="button" disabled={busy} onClick={() => setClosing('abort')}>
                    Abort
                  </button>
                </div>
              )}
              {open && closing !== null && (
                <form
                  className="confirm"
                  aria-label={closing === 'abort' ? 'Abort the rollout' : 'Finish the rollout'}
                  onSubmit={(e) => {
                    e.preventDefault();
                    void (closing === 'abort'
                      ? control(() => abortRollout(closeReason.trim()), `Aborted rollout #${rollout.id}.`)
                      : control(() => finishRollout(closeReason.trim()), `Finished rollout #${rollout.id}.`));
                  }}
                >
                  <p>
                    {closing === 'abort'
                      ? 'Stop the rollout? Tenants already moved stay on the new version. A rollback moves them back.'
                      : 'Close the rollout now? The tenants not yet moved are skipped and stay where they are.'}
                  </p>
                  <label htmlFor="close-reason">{closing === 'abort' ? 'Reason' : 'Reason (optional)'}</label>
                  <input
                    id="close-reason"
                    name="reason"
                    required={closing === 'abort'}
                    value={closeReason}
                    onChange={(e) => setCloseReason(e.target.value)}
                  />
                  <div className="actions">
                    <button type="submit" className="danger" disabled={busy}>
                      {closing === 'abort' ? 'Abort the rollout' : 'Finish the rollout'}
                    </button>
                    <button type="button" className="link" onClick={() => setClosing(null)}>
                      Cancel
                    </button>
                  </div>
                </form>
              )}

              <h3>Tenants</h3>
              {rollout.tenants.length === 0 ? (
                <p className="muted small">No tenant is in this rollout.</p>
              ) : (
                <ol className="revisions" aria-label="Rollout tenants">
                  {rollout.tenants.map(tenantRow)}
                </ol>
              )}
            </div>
          )}

          <h2 id="rollout-start-title">Start a rollout</h2>
          {open ? (
            <p className="muted small">One rollout runs at a time. Finish or abort this one first.</p>
          ) : (
            <form
              className="rollout-form"
              aria-labelledby="rollout-start-title"
              onSubmit={(e) => {
                e.preventDefault();
                void start();
              }}
            >
              <label htmlFor="rollout-version">Version</label>
              <select
                id="rollout-version"
                name="version"
                required
                value={draft.version}
                onChange={(e) => setDraft({ ...draft, version: e.target.value })}
              >
                <option value="">Choose a stored version</option>
                {stored
                  .filter((v) => !v.revoked)
                  .map((v) => (
                    <option key={v.version} value={v.version}>
                      {name(v.version)}
                      {v.serving ? ' (active)' : ''}
                    </option>
                  ))}
              </select>

              <label htmlFor="rollout-strategy">Strategy</label>
              <select
                id="rollout-strategy"
                name="strategy"
                aria-describedby="rollout-strategy-help"
                value={draft.strategy}
                onChange={(e) => setDraft({ ...draft, strategy: e.target.value as Draft['strategy'] })}
              >
                <option value="">The deployment's setting</option>
                {STRATEGIES.map((s) => (
                  <option key={s} value={s}>
                    {STRATEGY_WORDS[s]}
                  </option>
                ))}
              </select>
              <p id="rollout-strategy-help" className="muted small">
                Set the default with <span className="mono">ordo rollout set rollout.strategy</span>.
              </p>

              {(draft.strategy === 'canary' || draft.strategy === 'group' || draft.strategy === 'query') && (
                <fieldset>
                  <legend>The first stage</legend>
                  {draft.strategy === 'canary' && (
                    <>
                      <label htmlFor="rollout-canary-count">How many tenants first</label>
                      <input
                        id="rollout-canary-count"
                        name="canaryCount"
                        type="number"
                        inputMode="numeric"
                        min={1}
                        value={draft.canaryCount}
                        onChange={(e) => setDraft({ ...draft, canaryCount: e.target.value })}
                      />
                    </>
                  )}
                  {draft.strategy === 'group' && (
                    <>
                      <label htmlFor="rollout-group">Tenants first (ids, comma separated)</label>
                      <input
                        id="rollout-group"
                        name="group"
                        required
                        autoCapitalize="none"
                        autoCorrect="off"
                        spellCheck={false}
                        className="mono"
                        value={draft.group}
                        onChange={(e) => setDraft({ ...draft, group: e.target.value })}
                      />
                    </>
                  )}
                  {draft.strategy === 'query' && (
                    <>
                      <label htmlFor="rollout-query">Condition over the tenant facts</label>
                      <input
                        id="rollout-query"
                        name="query"
                        required
                        autoCapitalize="none"
                        autoCorrect="off"
                        spellCheck={false}
                        className="mono"
                        aria-describedby="rollout-query-help"
                        value={draft.query}
                        onChange={(e) => setDraft({ ...draft, query: e.target.value })}
                      />
                      <p id="rollout-query-help" className="muted small">
                        Like <span className="mono">members &lt; 5</span>.
                      </p>
                      <label htmlFor="rollout-order">Order (optional)</label>
                      <input
                        id="rollout-order"
                        name="order"
                        autoCapitalize="none"
                        autoCorrect="off"
                        spellCheck={false}
                        className="mono"
                        value={draft.order}
                        onChange={(e) => setDraft({ ...draft, order: e.target.value })}
                      />
                      <label htmlFor="rollout-limit">At most (optional)</label>
                      <input
                        id="rollout-limit"
                        name="limit"
                        type="number"
                        inputMode="numeric"
                        min={1}
                        value={draft.limit}
                        onChange={(e) => setDraft({ ...draft, limit: e.target.value })}
                      />
                    </>
                  )}
                  <label htmlFor="rollout-canary-wait">Judge it for (like 30m)</label>
                  <input
                    id="rollout-canary-wait"
                    name="canaryWait"
                    autoCapitalize="none"
                    autoCorrect="off"
                    spellCheck={false}
                    value={draft.canaryWait}
                    onChange={(e) => setDraft({ ...draft, canaryWait: e.target.value })}
                  />
                  <label htmlFor="rollout-health">Pause at this server fault rate (like 1%)</label>
                  <input
                    id="rollout-health"
                    name="healthErrorRate"
                    autoCapitalize="none"
                    autoCorrect="off"
                    spellCheck={false}
                    value={draft.healthErrorRate}
                    onChange={(e) => setDraft({ ...draft, healthErrorRate: e.target.value })}
                  />
                </fieldset>
              )}

              <details className="more">
                <summary>Batches and limits</summary>
                <p className="muted small">Blank takes the deployment's rollout setting.</p>
                <div className="pair">
                  <div>
                    <label htmlFor="rollout-interval">Batch every (like 5m)</label>
                    <input
                      id="rollout-interval"
                      name="batchInterval"
                      autoCapitalize="none"
                      autoCorrect="off"
                      spellCheck={false}
                      value={draft.batchInterval}
                      onChange={(e) => setDraft({ ...draft, batchInterval: e.target.value })}
                    />
                  </div>
                  <div>
                    <label htmlFor="rollout-size">Tenants per batch</label>
                    <input
                      id="rollout-size"
                      name="batchSize"
                      type="number"
                      inputMode="numeric"
                      value={draft.batchSize}
                      onChange={(e) => setDraft({ ...draft, batchSize: e.target.value })}
                    />
                  </div>
                  <div>
                    <label htmlFor="rollout-window">Spread over (like 24h)</label>
                    <input
                      id="rollout-window"
                      name="window"
                      autoCapitalize="none"
                      autoCorrect="off"
                      spellCheck={false}
                      value={draft.window}
                      onChange={(e) => setDraft({ ...draft, window: e.target.value })}
                    />
                  </div>
                  <div>
                    <label htmlFor="rollout-threshold">Pause after this many failed</label>
                    <input
                      id="rollout-threshold"
                      name="failureThreshold"
                      type="number"
                      inputMode="numeric"
                      value={draft.failureThreshold}
                      onChange={(e) => setDraft({ ...draft, failureThreshold: e.target.value })}
                    />
                  </div>
                </div>
              </details>

              <div className="actions">
                <button type="submit" className="primary" disabled={busy}>
                  Start the rollout
                </button>
              </div>
            </form>
          )}
        </>
      )}
      {prompt}
    </section>
  );
}
