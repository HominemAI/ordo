import { useCallback, useEffect, useId, useState } from 'react';
import {
  addMachineKey,
  auditPage,
  endAdmin,
  endMachineKey,
  inviteAdmin,
  loadAdmins,
  revokeInvite,
  type Admins as AdminsData,
  type AuditPage,
} from '../admins';
import { ConsoleCallError, explain, SIGNED_OUT, type Session } from '../api';
import { withTouch } from '../touch';
import './admins.css';

function when(secs: number | null | undefined): string {
  if (!secs) return 'never';
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** A secret shown once: an invite link or a machine key's private half. */
function Once(props: { label: string; value: string; note: string; onDone: () => void }) {
  const [copied, setCopied] = useState<string | null>(null);
  return (
    <div className="once">
      <p className="mono big" aria-label={props.label}>
        {props.value}
      </p>
      <p className="muted small">{props.note}</p>
      <div className="actions">
        <button
          type="button"
          onClick={() => {
            navigator.clipboard.writeText(props.value).then(
              () => setCopied('Copied.'),
              () => setCopied('Copy did not work here. Select the text and copy it.'),
            );
          }}
        >
          Copy
        </button>
        <button type="button" className="link" onClick={props.onDone}>
          Done
        </button>
        {copied && (
          <span className="muted" role="status">
            {copied}
          </span>
        )}
      </div>
    </div>
  );
}

/**
 * Deployment people (docs/plan-identity-service.md 5.6, 5.7, 7): the admins
 * the identity service holds, their invites, the deployment's machine keys,
 * and the service's audit log. Every change needs a passkey touch.
 */
export function Admins(props: { session: Session; onSignedOut: () => void }) {
  const [data, setData] = useState<AdminsData | null>(null);
  const [audit, setAudit] = useState<AuditPage | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [link, setLink] = useState<string | null>(null);
  const [secret, setSecret] = useState<string | null>(null);
  const [ending, setEnding] = useState<string | null>(null);
  const [typed, setTyped] = useState('');
  const [label, setLabel] = useState('');
  const [keyName, setKeyName] = useState('');
  const ids = { label: useId(), key: useId(), typed: useId() };

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
      }
    },
    [props],
  );

  const load = useCallback(async () => {
    const [d, a] = await Promise.all([loadAdmins(), auditPage()]);
    setData(d);
    setAudit(a);
  }, []);

  useEffect(() => {
    // Once, on open.
    void act(load);
  }, []);

  const live = data?.people.filter((p) => p.endedAt === null) ?? [];
  const openInvites = data?.invites.filter((i) => i.state === 'open') ?? [];
  const keys = data?.machineKeys.filter((k) => k.endedAt === null) ?? [];

  return (
    <section className="card admins">
      <h1>Deployment people</h1>
      <p className="muted">
        The people who manage this deployment sign in at its identity service with a passkey. Every change here needs a
        touch of your passkey. Ending someone's access takes effect at once for new sign-ins, and within the token life
        for a page they already have open.
      </p>
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
      {data === null && !error && <p className="muted">Loading…</p>}

      {data && (
        <>
          <h2>People</h2>
          <ul className="owners" aria-label="People">
            {live.map((p) => (
              <li key={p.id}>
                <div>
                  <strong>{p.displayName || p.id}</strong>
                  {p.id === props.session.person && <span className="tag">you</span>}
                  <div className="muted small">
                    {p.id} · admin since {when(p.createdAt)}
                  </div>
                </div>
                {ending !== p.id && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => {
                      setEnding(p.id);
                      setTyped('');
                    }}
                  >
                    End access
                  </button>
                )}
                {ending === p.id && (
                  <form
                    className="confirm"
                    aria-label={`End access for ${p.displayName || p.id}`}
                    onSubmit={(e) => {
                      e.preventDefault();
                      if (typed.trim() !== (p.displayName || p.id)) return;
                      void act(async () => {
                        await withTouch(() => endAdmin(p.id));
                        setEnding(null);
                        if (p.id === props.session.person) {
                          props.onSignedOut();
                          return;
                        }
                        setDone(`${p.displayName || p.id} can no longer sign in.`);
                        await load();
                      });
                    }}
                  >
                    <p>
                      Their sessions end and their recovery codes and open invites stop working. Type{' '}
                      <strong>{p.displayName || p.id}</strong> to confirm.
                    </p>
                    <label htmlFor={ids.typed}>Name</label>
                    <input
                      id={ids.typed}
                      autoComplete="off"
                      spellCheck={false}
                      value={typed}
                      onChange={(e) => setTyped(e.target.value)}
                    />
                    <div className="actions">
                      <button
                        type="submit"
                        className="danger"
                        disabled={busy || typed.trim() !== (p.displayName || p.id)}
                      >
                        End access
                      </button>
                      <button type="button" className="link" disabled={busy} onClick={() => setEnding(null)}>
                        Cancel
                      </button>
                    </div>
                  </form>
                )}
              </li>
            ))}
          </ul>

          <h2>Invite an admin</h2>
          {link ? (
            <Once
              label="Invite link"
              value={link}
              note="Send this link to the new admin. It works once, and they make a passkey when they open it. It is not shown again."
              onDone={() => setLink(null)}
            />
          ) : (
            <form
              className="people-form"
              onSubmit={(e) => {
                e.preventDefault();
                void act(async () => {
                  const made = await withTouch(() => inviteAdmin(label.trim()));
                  setLink(made.link);
                  setLabel('');
                  await load();
                });
              }}
            >
              <div className="field">
                <label htmlFor={ids.label}>Who it is for (optional)</label>
                <input
                  id={ids.label}
                  autoComplete="off"
                  value={label}
                  maxLength={80}
                  onChange={(e) => setLabel(e.target.value)}
                />
              </div>
              <button type="submit" disabled={busy}>
                Make an invite link
              </button>
            </form>
          )}
          {openInvites.length > 0 && (
            <ul className="owners" aria-label="Open invites">
              {openInvites.map((i) => (
                <li key={i.id}>
                  <div>
                    <strong>{i.label || 'Invite'}</strong>
                    <div className="muted small">
                      {i.id} · works until {when(i.expiresAt)}
                    </div>
                  </div>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() =>
                      void act(async () => {
                        await withTouch(() => revokeInvite(i.id));
                        setDone('The invite no longer works.');
                        await load();
                      })
                    }
                  >
                    Revoke
                  </button>
                </li>
              ))}
            </ul>
          )}

          <h2>Machine keys</h2>
          <p className="muted">
            A machine key signs CI in to deploy (ORDO_MACHINE_KEY). It belongs to the deployment, so it outlives the
            person who made it.
          </p>
          {secret ? (
            <Once
              label="Machine key"
              value={secret}
              note="Put this in your CI's secrets as ORDO_MACHINE_KEY. It is not shown again."
              onDone={() => setSecret(null)}
            />
          ) : (
            <form
              className="people-form"
              onSubmit={(e) => {
                e.preventDefault();
                if (!keyName.trim()) return;
                void act(async () => {
                  const made = await withTouch(() => addMachineKey(keyName.trim()));
                  setSecret(made.secret ?? null);
                  setKeyName('');
                  setDone(`Machine key ${made.id} made.`);
                  await load();
                });
              }}
            >
              <div className="field">
                <label htmlFor={ids.key}>Name</label>
                <input
                  id={ids.key}
                  autoComplete="off"
                  spellCheck={false}
                  value={keyName}
                  maxLength={64}
                  placeholder="ci-main"
                  onChange={(e) => setKeyName(e.target.value)}
                />
              </div>
              <button type="submit" disabled={busy || !keyName.trim()}>
                Make a machine key
              </button>
            </form>
          )}
          {keys.length > 0 && (
            <ul className="owners" aria-label="Machine keys">
              {keys.map((k) => (
                <li key={k.id}>
                  <div>
                    <strong className="mono">{k.name}</strong>
                    <div className="muted small">
                      {k.id} · made {when(k.createdAt)} · last used {when(k.lastUsedAt)}
                    </div>
                  </div>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() =>
                      void act(async () => {
                        await withTouch(() => endMachineKey(k.id));
                        setDone(`Machine key ${k.name} ended.`);
                        await load();
                      })
                    }
                  >
                    End
                  </button>
                </li>
              ))}
            </ul>
          )}
        </>
      )}

      {audit && (
        <>
          <h2>Audit log</h2>
          {audit.verified.ok ? (
            <p className="muted small">
              The log checks out: {audit.verified.rows} entries, {audit.verified.checkpoints} signed checkpoints, checked{' '}
              {when(audit.verified.at)}.
            </p>
          ) : (
            <p role="alert" className="audit-broken">
              The log does not check out: {audit.verified.problem}. Checked {when(audit.verified.at)}.
            </p>
          )}
          <ol className="audit" aria-label="Audit log, newest first">
            {audit.entries.map((e) => (
              <li key={e.seq}>
                <span className="muted small">{when(e.at)}</span>{' '}
                <span className="mono">{e.action}</span> by <span className="mono">{e.actor}</span>
                {e.subject && (
                  <>
                    {' '}
                    on <span className="mono">{e.subject}</span>
                  </>
                )}
                {e.outcome !== 'ok' && <span className="tag">{e.outcome}</span>}
              </li>
            ))}
          </ol>
          {audit.entries.length === 50 && (
            <button
              type="button"
              className="link"
              disabled={busy}
              onClick={() =>
                void act(async () => {
                  const last = audit.entries[audit.entries.length - 1];
                  const older = await auditPage(last?.seq);
                  setAudit({ ...older, entries: [...audit.entries, ...older.entries] });
                })
              }
            >
              Older entries
            </button>
          )}
        </>
      )}
    </section>
  );
}
