import { useCallback, useEffect, useState } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import { joinBuild, leaveBuild, listBuilds, removeBuild, setBuild, type Build } from '../builds';
import { history, type History } from '../deploys';
import { link, usable, type Link } from '../deployment';
import { listTenants, type Tenant } from '../tenants';
import { useDeploymentTouch } from './useDeploymentTouch';
import { versionName, when } from './versionNames';
import './rollouts.css';

/** What a build's row has open: a form to set its version, to add a tenant, or to remove it. */
type Open = { build: string; kind: 'set' | 'join' | 'remove' } | null;

/**
 * Tenant builds (docs/plan-versions.md 3.8, increment 16): a named build
 * pins named tenants to its version, apart from the active one. Activations,
 * rollbacks, rollouts and the revoked sweep leave them; only set, join and
 * leave move them. Each change asks for the owner's touch at the deployment,
 * and each refusal shows as the door gives it, naming the command that
 * fixes it.
 */
export function Builds(props: { onSignedOut: () => void }) {
  const [state, setState] = useState<Link | null>(null);
  const [builds, setBuilds] = useState<Build[] | null>(null);
  const [tenants, setTenants] = useState<Tenant[]>([]);
  const [past, setPast] = useState<History | null>(null);
  const [open, setOpen] = useState<Open>(null);
  const [form, setForm] = useState({ version: '', reason: '', tenant: '' });
  const [draft, setDraft] = useState({ name: '', version: '', reason: '' });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const { atDeployment, prompt } = useDeploymentTouch();

  const load = useCallback(async () => {
    const s = await link();
    setState(s);
    if (!usable(s)) return;
    const [b, t, h] = await Promise.all([listBuilds(), listTenants(), history()]);
    setBuilds(b.builds);
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
  const tenantName = (id: string) => tenants.find((t) => t.id === id)?.name || id;
  const inBuild = new Set((builds ?? []).flatMap((b) => b.tenants.map((m) => m.tenant)));

  const openRow = (build: string, kind: 'set' | 'join' | 'remove') => {
    setOpen({ build, kind });
    setForm({ version: '', reason: '', tenant: '' });
  };

  const moved = (n: number) => (n === 1 ? '1 tenant moved' : `${n} tenants moved`);

  const create = () =>
    act(async () => {
      const m = await atDeployment(() => setBuild(draft.name.trim(), draft.version, draft.reason.trim()));
      setDone(`Build ${m.build} is on ${name(m.version)}.`);
      setDraft({ name: '', version: '', reason: '' });
      await load();
    });

  const submitRow = (b: Build) =>
    act(async () => {
      if (open?.kind === 'set') {
        const m = await atDeployment(() => setBuild(b.name, form.version, form.reason.trim()));
        setDone(`Build ${m.build} is on ${name(m.version)}, ${moved(m.tenants.length)}.`);
      } else if (open?.kind === 'join') {
        const m = await atDeployment(() => joinBuild(b.name, form.tenant));
        setDone(`${tenantName(form.tenant)} joined ${m.build} and serves ${name(m.version)}.`);
      } else if (open?.kind === 'remove') {
        await atDeployment(() => removeBuild(b.name, form.reason.trim()));
        setDone(`Removed build ${b.name}.`);
      }
      setOpen(null);
      await load();
    });

  const leave = (tenant: string) =>
    act(async () => {
      const m = await atDeployment(() => leaveBuild(tenant));
      setDone(`${tenantName(tenant)} left ${m.build} and serves ${name(m.version)}.`);
      await load();
    });

  const versionOptions = stored
    .filter((v) => !v.revoked)
    .map((v) => (
      <option key={v.version} value={v.version}>
        {name(v.version)}
        {v.serving ? ' (active)' : ''}
      </option>
    ));

  return (
    <section className="card rollouts">
      <h1>Tenant builds</h1>
      <p className="muted small">
        A build pins the tenants you name to its version, apart from the active one: a beta tenant serves the branch while
        the rest serve main. Rollouts and rollbacks leave them where they are.
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

      {state && usable(state) && builds && (
        <>
          <h2>Builds</h2>
          {builds.length === 0 ? (
            <p className="muted">No build is set.</p>
          ) : (
            <ol className="revisions" aria-label="Builds">
              {builds.map((b) => (
                <li key={b.name}>
                  <div className="rev-head">
                    <strong className="mono">{b.name}</strong>
                    <span className="mono">{name(b.version)}</span>
                  </div>
                  <p className="small rollout-reason">{b.reason}</p>
                  <div className="muted small">
                    Set by {b.setBy}, {when(b.setAt)}.
                  </div>
                  {b.tenants.length === 0 ? (
                    <p className="muted small">It holds no tenant.</p>
                  ) : (
                    <ul className="kv" aria-label={`Tenants in ${b.name}`}>
                      {b.tenants.map((m) => (
                        <li key={m.tenant}>
                          <span>
                            {tenantName(m.tenant)} <span className="mono muted small">{m.tenant}</span>
                          </span>
                          <button type="button" className="link" disabled={busy} onClick={() => void leave(m.tenant)}>
                            Leave
                          </button>
                        </li>
                      ))}
                    </ul>
                  )}
                  {open?.build !== b.name && (
                    <div className="row-actions">
                      <button type="button" disabled={busy} onClick={() => openRow(b.name, 'join')}>
                        Add a tenant
                      </button>
                      <button type="button" disabled={busy} onClick={() => openRow(b.name, 'set')}>
                        Change its version
                      </button>
                      <button type="button" disabled={busy} onClick={() => openRow(b.name, 'remove')}>
                        Remove
                      </button>
                    </div>
                  )}
                  {open?.build === b.name && (
                    <form
                      className="confirm"
                      aria-label={
                        open.kind === 'join'
                          ? `Add a tenant to ${b.name}`
                          : open.kind === 'set'
                            ? `Change the version of ${b.name}`
                            : `Remove ${b.name}`
                      }
                      onSubmit={(e) => {
                        e.preventDefault();
                        void submitRow(b);
                      }}
                    >
                      {open.kind === 'join' && (
                        <>
                          <label htmlFor={`join-${b.name}`}>Tenant</label>
                          <select
                            id={`join-${b.name}`}
                            name="tenant"
                            required
                            value={form.tenant}
                            onChange={(e) => setForm({ ...form, tenant: e.target.value })}
                          >
                            <option value="">Choose a tenant</option>
                            {tenants
                              .filter((t) => !inBuild.has(t.id))
                              .map((t) => (
                                <option key={t.id} value={t.id}>
                                  {t.name || t.id} ({t.id})
                                </option>
                              ))}
                          </select>
                          <p className="muted small">It moves to {name(b.version)} now.</p>
                        </>
                      )}
                      {open.kind === 'set' && (
                        <>
                          <label htmlFor={`set-version-${b.name}`}>Version</label>
                          <select
                            id={`set-version-${b.name}`}
                            name="version"
                            required
                            value={form.version}
                            onChange={(e) => setForm({ ...form, version: e.target.value })}
                          >
                            <option value="">Choose a stored version</option>
                            {versionOptions}
                          </select>
                          <p className="muted small">Its tenants move to it now.</p>
                        </>
                      )}
                      {open.kind === 'remove' && <p>Remove this build? It must hold no tenant.</p>}
                      {open.kind !== 'join' && (
                        <>
                          <label htmlFor={`row-reason-${b.name}`}>Reason</label>
                          <input
                            id={`row-reason-${b.name}`}
                            name="reason"
                            required
                            value={form.reason}
                            onChange={(e) => setForm({ ...form, reason: e.target.value })}
                          />
                        </>
                      )}
                      <div className="actions">
                        <button type="submit" className={open.kind === 'remove' ? 'danger' : 'primary'} disabled={busy}>
                          {open.kind === 'join' ? 'Add it' : open.kind === 'set' ? 'Move the build' : 'Remove the build'}
                        </button>
                        <button type="button" className="link" onClick={() => setOpen(null)}>
                          Cancel
                        </button>
                      </div>
                    </form>
                  )}
                </li>
              ))}
            </ol>
          )}

          <h2 id="build-new-title">New build</h2>
          <form
            className="rollout-form"
            aria-labelledby="build-new-title"
            onSubmit={(e) => {
              e.preventDefault();
              void create();
            }}
          >
            <label htmlFor="build-name">Name</label>
            <input
              id="build-name"
              name="name"
              required
              maxLength={40}
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
              className="mono"
              aria-describedby="build-name-help"
              value={draft.name}
              onChange={(e) => setDraft({ ...draft, name: e.target.value })}
            />
            <p id="build-name-help" className="muted small">
              Lowercase letters, digits and dashes, starting with a letter, like <span className="mono">beta</span>.
            </p>
            <label htmlFor="build-version">Version</label>
            <select
              id="build-version"
              name="version"
              required
              value={draft.version}
              onChange={(e) => setDraft({ ...draft, version: e.target.value })}
            >
              <option value="">Choose a stored version</option>
              {versionOptions}
            </select>
            <label htmlFor="build-reason">Reason</label>
            <input
              id="build-reason"
              name="reason"
              required
              value={draft.reason}
              onChange={(e) => setDraft({ ...draft, reason: e.target.value })}
            />
            <div className="actions">
              <button type="submit" className="primary" disabled={busy}>
                Add the build
              </button>
            </div>
          </form>
        </>
      )}
      {prompt}
    </section>
  );
}
