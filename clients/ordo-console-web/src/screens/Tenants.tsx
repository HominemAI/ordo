import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { ConsoleCallError, explain, SIGNED_OUT } from '../api';
import { needsDeploymentTouch, touchDeployment } from '../deployment';
import {
  attachBucket,
  completeSuccession,
  createTenant,
  EVIDENCE,
  listCases,
  openSuccession,
  type OwnerCase,
  RESOLVE_REASONS,
  resolveObjection,
  deleteTenant,
  detachBucket,
  exportTenant,
  listTenants,
  PAUSE_NOTES,
  pauseTenant,
  reapTenant,
  resumeTenant,
  size,
  stored,
  type Tenant,
} from '../tenants';
import { withTouch } from '../touch';
import './tenants.css';

function day(secs: number): string {
  return new Date(secs * 1000).toLocaleDateString(undefined, { dateStyle: 'medium' });
}

function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** Where a case stands, in words. */
function standing(c: OwnerCase): string {
  if (c.paused) return 'Paused by an objection';
  if (c.mature) return 'Ready to complete';
  return `Matures ${when(c.maturesAt)}`;
}

const STATUS_WORDS: Record<Tenant['status'], string> = {
  provisioning: 'Being set up',
  active: 'Active',
  paused: 'Paused',
  failed: 'Failed',
  reaped: 'Removed',
};

/** The one action a dialog is open for. */
type Open =
  | { kind: 'pause'; t: Tenant }
  | { kind: 'delete'; t: Tenant }
  | { kind: 'bucket'; t: Tenant }
  | { kind: 'resolve'; c: OwnerCase }
  | { kind: 'complete'; c: OwnerCase }
  | null;

/**
 * Every tenant of the deployment (docs/plan-console.md 4.4): status, members,
 * usage, and the tenants it holds closed. Create, pause, resume, and remove one
 * stuck being set up. Where the deployment owner owns the tenant, export,
 * delete and its file bucket. Every change asks for the owner's touch at the
 * deployment, relayed by the console.
 */
export function Tenants(props: { onSignedOut: () => void }) {
  const [list, setList] = useState<Tenant[] | null>(null);
  const [readyError, setReadyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [owner, setOwner] = useState('');
  const [open, setOpen] = useState<Open>(null);
  const [note, setNote] = useState<string>('other');
  const [typed, setTyped] = useState('');
  const [bucket, setBucket] = useState({ endpoint: '', region: '', bucket: '', credential: '', platformSealed: true });
  const [cases, setCases] = useState<OwnerCase[] | null>(null);
  const [succ, setSucc] = useState({ tenant: '', principal: '', departed: '', evidence: '', reference: '' });
  const [decision, setDecision] = useState<'resume' | 'cancel'>('resume');
  const [reason, setReason] = useState<string>(RESOLVE_REASONS.resume[0]);
  const [endGrants, setEndGrants] = useState(true);
  const actionDialog = useRef<HTMLDialogElement>(null);
  const touchDialog = useRef<HTMLDialogElement>(null);
  const pending = useRef<{ resolve: () => void; reject: (e: unknown) => void } | null>(null);

  const load = useCallback(async () => {
    const [r, c] = await Promise.all([listTenants(), listCases()]);
    setList(r.tenants);
    setReadyError(r.readyError);
    setCases(c.cases);
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

  useEffect(() => {
    const d = actionDialog.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    if (!open && d.open) d.close();
  }, [open]);

  /** Ask for the touch at the deployment; settle when it is done or dismissed. */
  const askTouch = () =>
    new Promise<void>((resolve, reject) => {
      pending.current = { resolve, reject };
      touchDialog.current?.showModal();
    });

  const cancelTouch = () => {
    touchDialog.current?.close();
    pending.current?.reject(new DOMException('The touch was cancelled.', 'AbortError'));
    pending.current = null;
  };

  const touchNow = async () => {
    const p = pending.current;
    pending.current = null;
    try {
      await touchDeployment();
      touchDialog.current?.close();
      p?.resolve();
    } catch (e) {
      touchDialog.current?.close();
      p?.reject(e);
    }
  };

  /** A change at the deployment: the console owner's touch, then the deployment owner's, as asked. */
  const atDeployment = async <T,>(f: () => Promise<T>): Promise<T> => {
    try {
      return await withTouch(f);
    } catch (e) {
      if (!needsDeploymentTouch(e)) throw e;
    }
    await askTouch();
    return withTouch(f);
  };

  const change = (f: () => Promise<unknown>, said: string) =>
    act(async () => {
      await atDeployment(f);
      setOpen(null);
      setDone(said);
      await load();
    });

  const create = (e: FormEvent) => {
    e.preventDefault();
    const n = name.trim();
    if (!n) return;
    void act(async () => {
      const r = await atDeployment(() => createTenant(n, owner.trim()));
      setName('');
      setOwner('');
      setDone(`Created ${r.created.name}. Its id is ${r.created.space}.`);
      await load();
    });
  };

  const startSuccession = (e: FormEvent) => {
    e.preventDefault();
    const { tenant, principal, evidence, reference } = succ;
    const departed = succ.departed.split(/[\s,]+/).filter(Boolean);
    if (!tenant.trim() || !principal.trim() || departed.length === 0 || !evidence) return;
    void act(async () => {
      const r = await atDeployment(() =>
        openSuccession(tenant.trim(), principal.trim(), departed, evidence, reference.trim()),
      );
      setSucc({ tenant: '', principal: '', departed: '', evidence: '', reference: '' });
      setDone(
        `Opened succession ${r.opened.case}. The tenant's managers are told and may object until ${when(r.opened.maturesAt)}.`,
      );
      await load();
    });
  };

  const successions = (cases ?? []).filter((c) => c.path === 'succession');
  const held = (list ?? []).filter((t) => t.quarantined);

  return (
    <section className="card">
      <h1>Tenants</h1>
      <p className="muted">
        Every tenant of the deployment: its status, members and usage. You see how many members a tenant has, never who
        they are. Every change here asks for your passkey at the deployment.
      </p>
      {list === null && !error && <p className="muted">Loading…</p>}
      {held.length > 0 && (
        <p className="note" role="alert">
          {held.length === 1 ? '1 tenant is' : `${held.length} tenants are`} held closed by the deployment. See the
          reason beside each.
        </p>
      )}
      {readyError && <p className="muted small">The deployment's list of tenants held closed did not answer: {readyError}</p>}

      {list && list.length === 0 && <p>The deployment has no tenants yet. Create the first below.</p>}
      {list && list.length > 0 && (
        <div className="tenant-table" role="region" aria-labelledby="tenants-caption" tabIndex={0}>
          <table>
            <caption id="tenants-caption">Tenants, newest first</caption>
            <thead>
              <tr>
                <th scope="col">Tenant</th>
                <th scope="col">Status</th>
                <th scope="col">Members</th>
                <th scope="col">Usage, last period</th>
                <th scope="col">Made</th>
                <th scope="col">Actions</th>
              </tr>
            </thead>
            <tbody>
              {list.map((t) => (
                <tr key={t.id}>
                  <th scope="row">
                    <span className="tenant-name">{t.name}</span>
                    <span className="mono small muted tenant-id">{t.id}</span>
                    {t.yourRole && <span className="tag">you: {t.yourRole}</span>}
                  </th>
                  <td data-label="Status">
                    <span className={`pill pill-tenant-${t.status}`}>{STATUS_WORDS[t.status]}</span>
                    {t.status === 'paused' && t.pausedNote && (
                      <span className="small muted">
                        {' '}
                        by the {t.pausedBy}, {t.pausedNote}
                      </span>
                    )}
                    {t.status === 'failed' && t.note && <span className="small muted detail"> {t.note}</span>}
                    {t.quarantined && <span className="small error detail"> Held closed: {t.quarantined}</span>}
                  </td>
                  <td data-label="Members">{t.members ?? '-'}</td>
                  <td data-label="Usage">
                    {t.usage ? (
                      <>
                        {t.usage.writes} writes, {size(stored(t.usage))}
                      </>
                    ) : (
                      <span className="muted">none yet</span>
                    )}
                  </td>
                  <td data-label="Made">{day(t.createdAt)}</td>
                  <td data-label="Actions">
                    <div className="row-actions">
                      {t.status === 'active' && (
                        <button
                          type="button"
                          disabled={busy}
                          aria-label={`Pause ${t.name}`}
                          onClick={() => {
                            setNote('other');
                            setOpen({ kind: 'pause', t });
                          }}
                        >
                          Pause
                        </button>
                      )}
                      {t.status === 'paused' && (
                        <button
                          type="button"
                          disabled={busy}
                          aria-label={`Resume ${t.name}`}
                          onClick={() => void change(() => resumeTenant(t.id), `Resumed ${t.name}.`)}
                        >
                          Resume
                        </button>
                      )}
                      {(t.status === 'provisioning' || t.status === 'failed') && (
                        <button
                          type="button"
                          disabled={busy}
                          aria-label={`Remove ${t.name}`}
                          onClick={() =>
                            void change(() => reapTenant(t.id), `Removed ${t.name} and released what it held.`)
                          }
                        >
                          Remove
                        </button>
                      )}
                      {t.yourRole === 'owner' && (t.status === 'active' || t.status === 'paused') && (
                        <>
                          <button
                            type="button"
                            disabled={busy}
                            aria-label={`Export ${t.name}`}
                            onClick={() =>
                              void act(async () => {
                                const r = await atDeployment(() => exportTenant(t.id));
                                setDone(`Exported ${t.name} to ${r.exported.bundle} on the deployment.`);
                              })
                            }
                          >
                            Export
                          </button>
                          <button
                            type="button"
                            disabled={busy}
                            aria-label={`File bucket for ${t.name}`}
                            onClick={() => setOpen({ kind: 'bucket', t })}
                          >
                            Bucket
                          </button>
                          <button
                            type="button"
                            className="danger"
                            disabled={busy}
                            aria-label={`Delete ${t.name}`}
                            onClick={() => {
                              setTyped('');
                              setOpen({ kind: 'delete', t });
                            }}
                          >
                            Delete
                          </button>
                        </>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {done && (
        <p role="status" className="ok">
          {done}
        </p>
      )}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}

      <h2>Create a tenant</h2>
      <form onSubmit={create}>
        <label htmlFor="tenant-name">Name</label>
        <input
          id="tenant-name"
          value={name}
          maxLength={256}
          required
          onChange={(e) => setName(e.target.value)}
          autoComplete="off"
        />
        <label htmlFor="tenant-owner">Owner (optional)</label>
        <input
          id="tenant-owner"
          className="mono"
          value={owner}
          aria-describedby="tenant-owner-hint"
          onChange={(e) => setOwner(e.target.value)}
          autoComplete="off"
          spellCheck={false}
        />
        <p id="tenant-owner-hint" className="muted small">
          The id of the person who will own it, such as usr_…. Leave it empty to own it yourself.
        </p>
        <div className="actions">
          <button type="submit" className="primary" disabled={busy || !name.trim()}>
            Create tenant
          </button>
        </div>
      </form>

      <h2 id="succession-title">Owner succession</h2>
      <p className="muted">
        When a tenant's owner is gone for good, make an existing member its owner. You type the successor's id: the
        console shows no list of the tenant's members. The tenant's managers are told and may object during the delay;
        an objection pauses the case until you resume or cancel it.
      </p>
      {cases && successions.length === 0 && <p className="muted small">No succession is open.</p>}
      {successions.length > 0 && (
        <ul className="case-list" aria-labelledby="succession-title">
          {successions.map((c) => (
            <li key={c.case} className="case">
              <div>
                <span className="mono">{c.case}</span>{' '}
                <span className={`pill ${c.paused ? 'pill-tenant-paused' : c.mature ? 'pill-tenant-active' : ''}`}>
                  {standing(c)}
                </span>
              </div>
              <p className="small">
                Tenant <span className="mono">{c.tenant}</span>, successor <span className="mono">{c.principal}</span>,
                evidence {c.evidence}
                {c.reference && <>, ref {c.reference}</>}.{' '}
                {c.departed.length > 0 && (
                  <>
                    Has left:{' '}
                    <span className="mono">{c.departed.join(', ')}</span>.
                  </>
                )}
              </p>
              {c.objections.length > 0 && (
                <ul className="small objections">
                  {c.objections.map((o) => (
                    <li key={o.objector}>
                      Objection by <span className="mono">{o.objector}</span> on {when(o.at)}:{' '}
                      {o.resolution ? (
                        <>
                          resolved, {o.resolution} ({o.reason}) on {when(o.resolvedAt ?? o.at)}
                        </>
                      ) : (
                        <strong>waiting for your decision</strong>
                      )}
                    </li>
                  ))}
                </ul>
              )}
              <div className="row-actions">
                {c.paused && (
                  <button
                    type="button"
                    disabled={busy}
                    aria-label={`Resolve the objection to ${c.case}`}
                    onClick={() => {
                      setDecision('resume');
                      setReason(RESOLVE_REASONS.resume[0]);
                      setOpen({ kind: 'resolve', c });
                    }}
                  >
                    Resolve objection
                  </button>
                )}
                {c.mature && (
                  <button
                    type="button"
                    className="primary"
                    disabled={busy}
                    aria-label={`Complete ${c.case}`}
                    onClick={() => {
                      setEndGrants(true);
                      setOpen({ kind: 'complete', c });
                    }}
                  >
                    Complete
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
      <form onSubmit={startSuccession} aria-labelledby="succession-form-title">
        <h3 id="succession-form-title">Open a succession</h3>
        <div className="pair">
          <div>
            <label htmlFor="succ-tenant">Tenant id</label>
            <input
              id="succ-tenant"
              name="tenant"
              className="mono"
              required
              maxLength={128}
              value={succ.tenant}
              onChange={(e) => setSucc({ ...succ, tenant: e.target.value })}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
            />
          </div>
          <div>
            <label htmlFor="succ-principal">Successor's id</label>
            <input
              id="succ-principal"
              name="principal"
              className="mono"
              required
              maxLength={256}
              aria-describedby="succ-principal-hint"
              value={succ.principal}
              onChange={(e) => setSucc({ ...succ, principal: e.target.value })}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
            />
          </div>
          <div>
            <label htmlFor="succ-departed">Who has left</label>
            <input
              id="succ-departed"
              name="departed"
              className="mono"
              required
              maxLength={2600}
              aria-describedby="succ-departed-hint"
              value={succ.departed}
              onChange={(e) => setSucc({ ...succ, departed: e.target.value })}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
            />
          </div>
          <div>
            <label htmlFor="succ-evidence">Evidence</label>
            <select
              id="succ-evidence"
              name="evidence"
              required
              value={succ.evidence}
              onChange={(e) => setSucc({ ...succ, evidence: e.target.value })}
            >
              <option value="">Choose one</option>
              {EVIDENCE.map((v) => (
                <option key={v} value={v}>
                  {v}
                </option>
              ))}
            </select>
          </div>
          <div>
            <label htmlFor="succ-reference">Reference (optional)</label>
            <input
              id="succ-reference"
              name="reference"
              className="mono"
              maxLength={120}
              pattern="[A-Za-z0-9._:/\-]*"
              aria-describedby="succ-reference-hint"
              value={succ.reference}
              onChange={(e) => setSucc({ ...succ, reference: e.target.value })}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
            />
          </div>
        </div>
        <p id="succ-principal-hint" className="muted small">
          The person must already be an active member of the tenant. Ask the tenant for their id.
        </p>
        <p id="succ-departed-hint" className="muted small">
          The id of each owner who has left, separated by commas. Only they lose their grants to apps and
          delegations when the succession completes; any other owner keeps theirs.
        </p>
        <p id="succ-reference-hint" className="muted small">
          Your own ticket number. Letters, digits and . _ : / - only; no free text is recorded.
        </p>
        <div className="actions">
          <button type="submit" className="primary" disabled={busy}>
            Open succession
          </button>
        </div>
      </form>

      <dialog ref={actionDialog} aria-labelledby="tenant-action-title" onCancel={() => setOpen(null)}>
        {open?.kind === 'resolve' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void change(
                () => resolveObjection(open.c.case, decision, reason),
                decision === 'resume' ? `Resumed ${open.c.case}.` : `Cancelled ${open.c.case}.`,
              );
            }}
          >
            <h2 id="tenant-action-title">Resolve the objection to {open.c.case}</h2>
            <p>Resume lets the succession go on. Cancel ends it. Your decision and reason are recorded.</p>
            <fieldset>
              <legend>Decision</legend>
              {(['resume', 'cancel'] as const).map((d) => (
                <div className="check" key={d}>
                  <input
                    id={`decision-${d}`}
                    type="radio"
                    name="decision"
                    value={d}
                    checked={decision === d}
                    onChange={() => {
                      setDecision(d);
                      setReason(RESOLVE_REASONS[d][0]);
                    }}
                  />
                  <label htmlFor={`decision-${d}`}>{d === 'resume' ? 'Resume' : 'Cancel the succession'}</label>
                </div>
              ))}
            </fieldset>
            <label htmlFor="resolve-reason">Reason</label>
            <select id="resolve-reason" name="reason" value={reason} onChange={(e) => setReason(e.target.value)}>
              {RESOLVE_REASONS[decision].map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
            <div className="actions">
              <button type="submit" className={decision === 'cancel' ? 'danger' : 'primary'} disabled={busy}>
                {decision === 'resume' ? 'Resume succession' : 'Cancel succession'}
              </button>
              <button type="button" className="link" onClick={() => setOpen(null)}>
                Close
              </button>
            </div>
          </form>
        )}
        {open?.kind === 'complete' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              const c = open.c;
              void act(async () => {
                const r = await atDeployment(() => completeSuccession(c.tenant, c.case, endGrants));
                const s = r.succeeded;
                setOpen(null);
                const ended = s.endsGrants
                  ? ` Ended ${s.delegationsEnded.length} delegation(s) and ${s.grantsEnded.length} grant(s) of the departed owner.`
                  : ' The departed owner\'s grants and delegations were kept.';
                const left = s.notEnded.length ? ` Not ended: ${s.notEnded.join(', ')}.` : '';
                setDone(`${s.principal} now owns ${s.tenant}.${ended}${left}`);
                await load();
              });
            }}
          >
            <h2 id="tenant-action-title">Complete {open.c.case}</h2>
            <p>
              <span className="mono">{open.c.principal}</span> becomes an owner of{' '}
              <span className="mono">{open.c.tenant}</span>. No credential or session changes.
            </p>
            <div className="check">
              <input
                id="succ-end-grants"
                type="checkbox"
                checked={endGrants}
                aria-describedby="succ-end-grants-hint"
                onChange={(e) => setEndGrants(e.target.checked)}
              />
              <label htmlFor="succ-end-grants">
                End the grants to apps and delegations of <span className="mono">{open.c.departed.join(', ')}</span>
              </label>
            </div>
            <p id="succ-end-grants-hint" className="muted small">
              On by default. The record names each one ended, and the new owner can grant again.
            </p>
            <div className="actions">
              <button type="submit" className="primary" disabled={busy}>
                Complete succession
              </button>
              <button type="button" className="link" onClick={() => setOpen(null)}>
                Cancel
              </button>
            </div>
          </form>
        )}
        {open?.kind === 'pause' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void change(() => pauseTenant(open.t.id, note), `Paused ${open.t.name}.`);
            }}
          >
            <h2 id="tenant-action-title">Pause {open.t.name}</h2>
            <p>
              A paused tenant keeps its data and refuses its ops until you resume it. Its people cannot lift your
              pause.
            </p>
            <label htmlFor="pause-note">Reason</label>
            <select id="pause-note" value={note} onChange={(e) => setNote(e.target.value)}>
              {PAUSE_NOTES.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
            <div className="actions">
              <button type="submit" className="primary" disabled={busy}>
                Pause tenant
              </button>
              <button type="button" className="link" onClick={() => setOpen(null)}>
                Cancel
              </button>
            </div>
          </form>
        )}
        {open?.kind === 'delete' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (typed.trim() !== open.t.id) return;
              void change(() => deleteTenant(open.t.id, typed.trim()), `Deleted ${open.t.name}.`);
            }}
          >
            <h2 id="tenant-action-title">Delete {open.t.name}</h2>
            <p>
              This destroys the tenant's data for good. Only the record that it was deleted is kept. Export it first if
              anyone may need it.
            </p>
            <label htmlFor="delete-confirm">
              Type the tenant's id, <span className="mono">{open.t.id}</span>, to confirm
            </label>
            <input
              id="delete-confirm"
              className="mono"
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
            <div className="actions">
              <button type="submit" className="danger" disabled={busy || typed.trim() !== open.t.id}>
                Delete for good
              </button>
              <button type="button" className="link" onClick={() => setOpen(null)}>
                Cancel
              </button>
            </div>
          </form>
        )}
        {open?.kind === 'bucket' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void change(() => attachBucket(open.t.id, bucket), `Attached ${bucket.bucket} to ${open.t.name}.`);
            }}
          >
            <h2 id="tenant-action-title">File bucket for {open.t.name}</h2>
            <p className="muted small">
              Uploads from now on go to this bucket. Files already stored stay where they are.
            </p>
            <div className="pair">
              <div>
                <label htmlFor="b-endpoint">Endpoint</label>
                <input
                  id="b-endpoint"
                  type="url"
                  required
                  value={bucket.endpoint}
                  onChange={(e) => setBucket({ ...bucket, endpoint: e.target.value })}
                />
              </div>
              <div>
                <label htmlFor="b-region">Region</label>
                <input
                  id="b-region"
                  required
                  value={bucket.region}
                  onChange={(e) => setBucket({ ...bucket, region: e.target.value })}
                />
              </div>
              <div>
                <label htmlFor="b-bucket">Bucket</label>
                <input
                  id="b-bucket"
                  required
                  value={bucket.bucket}
                  onChange={(e) => setBucket({ ...bucket, bucket: e.target.value })}
                />
              </div>
              <div>
                <label htmlFor="b-credential">Credential label</label>
                <input
                  id="b-credential"
                  required
                  value={bucket.credential}
                  aria-describedby="b-credential-hint"
                  onChange={(e) => setBucket({ ...bucket, credential: e.target.value })}
                />
              </div>
            </div>
            <p id="b-credential-hint" className="muted small">
              The name of a credential the deployment already holds, never the secret itself.
            </p>
            <div className="check">
              <input
                id="b-sealed"
                type="checkbox"
                checked={bucket.platformSealed}
                onChange={(e) => setBucket({ ...bucket, platformSealed: e.target.checked })}
              />
              <label htmlFor="b-sealed">Seal files with the platform's keys</label>
            </div>
            <div className="actions">
              <button type="submit" className="primary" disabled={busy}>
                Attach bucket
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() =>
                  void change(() => detachBucket(open.t.id), `Detached the bucket from ${open.t.name}.`)
                }
              >
                Detach the current bucket
              </button>
              <button type="button" className="link" onClick={() => setOpen(null)}>
                Cancel
              </button>
            </div>
          </form>
        )}
      </dialog>

      <dialog ref={touchDialog} aria-labelledby="tenant-touch-title" onCancel={cancelTouch}>
        <h2 id="tenant-touch-title">Touch your passkey for the deployment</h2>
        <p>
          This change is made at the deployment, which asks for your own passkey there. One touch covers changes for
          the next 5 minutes.
        </p>
        <div className="actions">
          <button type="button" className="primary" onClick={() => void touchNow()}>
            Use my passkey
          </button>
          <button type="button" className="link" onClick={cancelTouch}>
            Cancel
          </button>
        </div>
      </dialog>
    </section>
  );
}
