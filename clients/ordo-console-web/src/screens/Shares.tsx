import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import {
  KINDS,
  listShares,
  readSecurity,
  revokeLink,
  revokeShare,
  when,
  type AuditEvent,
  type LinkState,
  type SecurityEvent,
  type SecurityPage,
  type Source,
  type TenantShares,
} from '../shares';
import { listTenants, type Tenant } from '../tenants';
import { useDeploymentTouch } from './useDeploymentTouch';
import './shares.css';

const STATE_WORDS: Record<LinkState, string> = {
  live: 'Live',
  expired: 'Expired',
  'used-up': 'Used up',
  revoked: 'Revoked',
};

const SOURCE_WORDS: Record<Source, string> = {
  deployment: 'The deployment',
  tenant: 'One tenant',
  console: 'This console',
};

/** What a revoke dialog is open for. */
type Revoke = { kind: 'link'; id: string; label: string } | { kind: 'share'; seat: string; label: string } | null;

type View = 'links' | 'log';

/**
 * Shares and links, and the security log (docs/plan-console.md 4.7 and 4.8).
 *
 * Links and shares are read in a tenant where the deployment owner holds a
 * role, with the owner's touch at the deployment: a console session leaves
 * the deployment's own space only with one. The log reads the deployment's
 * own record (no touch), one tenant's record (a touch), or this console's
 * own audit log.
 */
export function Shares(props: { onSignedOut: () => void }) {
  const [view, setView] = useState<View>('links');
  const [tenants, setTenants] = useState<Tenant[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const { atDeployment, prompt } = useDeploymentTouch();

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
    // Once, on open: the tenants where the owner holds a role.
    void act(async () => {
      const r = await listTenants();
      setTenants(r.tenants.filter((t) => t.yourRole && t.status !== 'reaped'));
    });
  }, []);

  return (
    <>
      <h1>Shares and security</h1>
      <div className="view-switch" role="group" aria-label="View">
        <button type="button" aria-pressed={view === 'links'} onClick={() => setView('links')}>
          Links and shares
        </button>
        <button type="button" aria-pressed={view === 'log'} onClick={() => setView('log')}>
          Security log
        </button>
      </div>
      <p role="status" className="ok">
        {done}
      </p>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {view === 'links' ? (
        <LinksView
          tenants={tenants}
          busy={busy}
          act={act}
          atDeployment={atDeployment}
          onDone={setDone}
        />
      ) : (
        <LogView tenants={tenants} busy={busy} act={act} atDeployment={atDeployment} />
      )}
      {prompt}
    </>
  );
}

type Act = (f: () => Promise<void>) => Promise<void>;
type AtDeployment = <T>(f: () => Promise<T>) => Promise<T>;

function TenantPicker(props: { id: string; tenants: Tenant[]; value: string; onChange: (v: string) => void }) {
  return (
    <>
      <label htmlFor={props.id}>Tenant</label>
      <select id={props.id} value={props.value} onChange={(e) => props.onChange(e.target.value)}>
        <option value="">Choose a tenant</option>
        {props.tenants.map((t) => (
          <option key={t.id} value={t.id}>
            {t.name} ({t.id}), you: {t.yourRole}
          </option>
        ))}
      </select>
    </>
  );
}

function LinksView(props: {
  tenants: Tenant[];
  busy: boolean;
  act: Act;
  atDeployment: AtDeployment;
  onDone: (s: string) => void;
}) {
  const [tenant, setTenant] = useState('');
  const [data, setData] = useState<TenantShares | null>(null);
  const [revoke, setRevoke] = useState<Revoke>(null);
  const dialog = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const d = dialog.current;
    if (!d) return;
    if (revoke && !d.open) d.showModal();
    if (!revoke && d.open) d.close();
  }, [revoke]);

  const load = (id: string) => props.atDeployment(() => listShares(id)).then(setData);

  const show = (e: FormEvent) => {
    e.preventDefault();
    if (!tenant) return;
    void props.act(() => load(tenant));
  };

  const confirm = () => {
    const r = revoke;
    if (!r || !data) return;
    setRevoke(null);
    void props.act(async () => {
      if (r.kind === 'link') {
        await props.atDeployment(() => revokeLink(data.tenant, r.id));
        props.onDone(`The link "${r.label}" is revoked. Whoever holds it is refused at their next call.`);
      } else {
        await props.atDeployment(() => revokeShare(data.tenant, r.seat));
        props.onDone(`The share with ${r.label} is revoked.`);
      }
      await load(data.tenant);
    });
  };

  return (
    <section aria-labelledby="links-title">
      <h2 id="links-title">Links and shares in a tenant</h2>
      {props.tenants.length === 0 ? (
        <p className="muted">
          You hold a role in no tenant. A tenant's links and shares are its own: you see them only where you were given
          a role.
        </p>
      ) : (
        <form onSubmit={show} className="picker">
          <TenantPicker id="links-tenant" tenants={props.tenants} value={tenant} onChange={setTenant} />
          <button type="submit" className="primary" disabled={props.busy || !tenant}>
            Show links and shares
          </button>
          <p className="muted small">Reading a tenant asks for your touch at the deployment, once every 5 minutes.</p>
        </form>
      )}

      {data && (
        <>
          {data.links.length === 0 ? (
            <p>This tenant has no share links.</p>
          ) : (
            <div className="share-table" role="region" aria-labelledby="links-caption" tabIndex={0}>
              <table>
                <caption id="links-caption">Share links in {data.tenant}, newest first</caption>
                <thead>
                  <tr>
                    <th scope="col">Link</th>
                    <th scope="col">State</th>
                    <th scope="col">Opens</th>
                    <th scope="col">Devices</th>
                    <th scope="col">Lock</th>
                    <th scope="col">Ends</th>
                    <th scope="col">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {data.links.map((l) => (
                    <tr key={l.id}>
                      <th scope="row">
                        <span className="share-name">{l.label || 'Untitled link'}</span>
                        <span className="mono small muted share-id">{l.id}</span>
                        {l.hasPassword && <span className="tag">password</span>}
                      </th>
                      <td data-label="State">
                        <span className={`pill pill-link-${l.state}`}>{STATE_WORDS[l.state]}</span>
                      </td>
                      <td data-label="Opens">{l.opens}</td>
                      <td data-label="Devices">
                        {l.devices}
                        {l.maxOpens !== null && <span className="muted"> of {l.maxOpens}</span>}
                      </td>
                      <td data-label="Lock">
                        {l.locked && l.lockedUntil ? (
                          <span className="error">Locked until {when(l.lockedUntil)}</span>
                        ) : (
                          <span className="muted">none</span>
                        )}
                      </td>
                      <td data-label="Ends">{when(l.expiresAt)}</td>
                      <td data-label="Actions">
                        {l.state !== 'revoked' && (
                          <button
                            type="button"
                            className="danger"
                            disabled={props.busy}
                            aria-label={`Revoke ${l.label || l.id}`}
                            onClick={() => setRevoke({ kind: 'link', id: l.id, label: l.label || l.id })}
                          >
                            Revoke
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {data.shares.length === 0 ? (
            <p>This tenant has no live shares.</p>
          ) : (
            <div className="share-table" role="region" aria-labelledby="shares-caption" tabIndex={0}>
              <table>
                <caption id="shares-caption">Live shares in {data.tenant}</caption>
                <thead>
                  <tr>
                    <th scope="col">Shared with</th>
                    <th scope="col">Reaches</th>
                    <th scope="col">Ends</th>
                    <th scope="col">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {data.shares.map((s, i) => (
                    <tr key={s.seat ?? `none-${i}`}>
                      <th scope="row">
                        <span className="share-name">{s.displayName || 'A guest'}</span>
                        {s.seat && <span className="mono small muted share-id">{s.seat}</span>}
                        {!s.guest && <span className="tag">member</span>}
                      </th>
                      <td data-label="Reaches">{s.ops.join(', ')}</td>
                      <td data-label="Ends">{when(s.expiresAt)}</td>
                      <td data-label="Actions">
                        {s.seat && (
                          <button
                            type="button"
                            className="danger"
                            disabled={props.busy}
                            aria-label={`Revoke the share with ${s.displayName || s.seat}`}
                            onClick={() =>
                              setRevoke({ kind: 'share', seat: s.seat as string, label: s.displayName || s.seat || '' })
                            }
                          >
                            Revoke
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}

      <dialog ref={dialog} aria-labelledby="revoke-title" onClose={() => setRevoke(null)}>
        <h2 id="revoke-title">{revoke?.kind === 'share' ? 'Revoke this share?' : 'Revoke this link?'}</h2>
        <p>
          {revoke?.kind === 'share'
            ? `${revoke.label} loses access at their next call.`
            : `Whoever holds "${revoke?.label ?? ''}" is refused at their next call. A revoked link cannot be turned back on.`}
        </p>
        <div className="actions">
          <button type="button" className="danger" onClick={confirm}>
            Revoke
          </button>
          <button type="button" className="link" onClick={() => setRevoke(null)}>
            Keep it
          </button>
        </div>
      </dialog>
    </section>
  );
}

function isAudit(e: SecurityEvent | AuditEvent): e is AuditEvent {
  return 'action' in e;
}

function LogView(props: { tenants: Tenant[]; busy: boolean; act: Act; atDeployment: AtDeployment }) {
  const [source, setSource] = useState<Source>('deployment');
  const [kind, setKind] = useState('');
  const [tenant, setTenant] = useState('');
  const [action, setAction] = useState('');
  const [page, setPage] = useState<SecurityPage | null>(null);
  const [rows, setRows] = useState<(SecurityEvent | AuditEvent)[]>([]);

  const query = (more: boolean) => ({
    source,
    kind: source === 'deployment' ? kind : undefined,
    tenant: source === 'console' ? undefined : tenant,
    action: source === 'console' ? action : undefined,
    before: more && page?.before != null ? page.before : undefined,
    from: more && page?.from != null ? page.from : undefined,
  });

  const read = (more: boolean) =>
    props.act(async () => {
      const q = query(more);
      const p = source === 'tenant' ? await props.atDeployment(() => readSecurity(q)) : await readSecurity(q);
      setPage(p);
      setRows(more ? [...rows, ...p.events] : p.events);
    });

  useEffect(() => {
    // The deployment's own record, on open.
    void read(false);
  }, []);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (source === 'tenant' && !tenant) return;
    void read(false);
  };

  const more = page && (page.before != null || page.from != null);

  return (
    <section aria-labelledby="log-title">
      <h2 id="log-title">Security log</h2>
      <form onSubmit={submit} className="log-filters">
        <div>
          <label htmlFor="log-source">Record</label>
          <select id="log-source" value={source} onChange={(e) => setSource(e.target.value as Source)}>
            {(Object.keys(SOURCE_WORDS) as Source[]).map((s) => (
              <option key={s} value={s}>
                {SOURCE_WORDS[s]}
              </option>
            ))}
          </select>
        </div>
        {source === 'deployment' && (
          <>
            <div>
              <label htmlFor="log-kind">Kind</label>
              <select id="log-kind" value={kind} onChange={(e) => setKind(e.target.value)}>
                <option value="">Every kind</option>
                {KINDS.map((k) => (
                  <option key={k} value={k}>
                    {k}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label htmlFor="log-about">About tenant</label>
              <input
                id="log-about"
                className="mono"
                value={tenant}
                placeholder="any"
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => setTenant(e.target.value.trim())}
              />
            </div>
          </>
        )}
        {source === 'tenant' && (
          <div>
            <TenantPicker id="log-tenant" tenants={props.tenants} value={tenant} onChange={setTenant} />
          </div>
        )}
        {source === 'console' && (
          <div>
            <label htmlFor="log-action">Action starts with</label>
            <input
              id="log-action"
              className="mono"
              value={action}
              placeholder="any, or tenants. or door."
              autoComplete="off"
              spellCheck={false}
              onChange={(e) => setAction(e.target.value.trim())}
            />
          </div>
        )}
        <div className="log-go">
          <button type="submit" className="primary" disabled={props.busy || (source === 'tenant' && !tenant)}>
            Show
          </button>
        </div>
      </form>
      {source === 'deployment' && (
        <p className="muted small">
          What has no tenant home: sign-ins, strangers, deploys, quotas, console keys, tenants made or paused. Newest
          first.
        </p>
      )}
      {source === 'tenant' && (
        <p className="muted small">The tenant's own record, oldest first. It asks for your touch at the deployment.</p>
      )}
      {source === 'console' && (
        <p className="muted small">Every change made through this console, and who signed in to it. Newest first.</p>
      )}

      {page && rows.length === 0 && <p>Nothing recorded matches.</p>}
      {rows.length > 0 && page?.source === 'console' && (
        <div className="share-table" role="region" aria-labelledby="audit-caption" tabIndex={0}>
          <table>
            <caption id="audit-caption">This console's audit log</caption>
            <thead>
              <tr>
                <th scope="col">When</th>
                <th scope="col">Action</th>
                <th scope="col">Outcome</th>
                <th scope="col">Who</th>
                <th scope="col">Detail</th>
              </tr>
            </thead>
            <tbody>
              {rows.filter(isAudit).map((e) => (
                <tr key={e.id}>
                  <th scope="row">{when(e.at)}</th>
                  <td data-label="Action" className="mono">
                    {e.action}
                  </td>
                  <td data-label="Outcome">
                    <span className={e.outcome === 'ok' ? 'ok' : 'error'}>{e.outcome}</span>
                  </td>
                  <td data-label="Who" className="mono small">
                    {e.actor}
                  </td>
                  <td data-label="Detail" className="mono small detail">
                    {e.detail || '-'}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {rows.length > 0 && page?.source !== 'console' && (
        <div className="share-table" role="region" aria-labelledby="events-caption" tabIndex={0}>
          <table>
            <caption id="events-caption">
              {page?.source === 'tenant' ? 'The tenant' : 'The deployment'}'s security record
            </caption>
            <thead>
              <tr>
                <th scope="col">When</th>
                <th scope="col">What</th>
                <th scope="col">Who</th>
                <th scope="col">About</th>
                <th scope="col">Times</th>
                <th scope="col">Detail</th>
              </tr>
            </thead>
            <tbody>
              {rows
                .filter((e): e is SecurityEvent => !isAudit(e))
                .map((e) => (
                  <tr key={e.index}>
                    <th scope="row">{when(e.lastAt)}</th>
                    <td data-label="What">
                      <span className={`pill pill-kind-${e.kind}`}>{e.kind}</span>
                      {e.act && <span className="mono small"> {e.act}</span>}
                    </td>
                    <td data-label="Who" className="mono small">
                      {e.subject}
                    </td>
                    <td data-label="About" className="mono small">
                      {e.tenant ?? (e.op || '-')}
                    </td>
                    <td data-label="Times">{e.count}</td>
                    <td data-label="Detail" className="mono small detail">
                      {e.decisions?.join(', ') || e.detail || '-'}
                    </td>
                  </tr>
                ))}
            </tbody>
          </table>
        </div>
      )}
      {more && (
        <button type="button" disabled={props.busy} onClick={() => void read(true)}>
          {page?.source === 'tenant' ? 'Newer' : 'Older'}
        </button>
      )}
    </section>
  );
}
