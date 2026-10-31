import { useCallback, useEffect, useState } from 'react';
import { ConsoleCallError, explain, get, post, SIGNED_OUT, type Invite, type Owner, type Session } from '../api';
import { withTouch } from '../touch';

const MAX_OWNERS = 2;

function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

export function Owners(props: { session: Session; onSignedOut: () => void }) {
  const [owners, setOwners] = useState<Owner[] | null>(null);
  const [invite, setInvite] = useState<Invite | null>(null);
  const [confirm, setConfirm] = useState<Owner | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

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
    const r = await get<{ owners: Owner[] }>('owners');
    setOwners(r.owners);
  }, []);

  useEffect(() => {
    // Once, on open.
    void act(load);
  }, []);

  const full = (owners?.length ?? 0) >= MAX_OWNERS;

  return (
    <section className="card">
      <h1>Owners</h1>
      <p className="muted">
        Up to {MAX_OWNERS} people own this console. Each signs in with their own passkey, so one lost passkey does not
        lock you out.
      </p>
      {owners === null && !error && <p className="muted">Loading…</p>}
      {owners && (
        <ul className="owners">
          {owners.map((o) => (
            <li key={o.id}>
              <div>
                <strong>{o.label || 'Console owner'}</strong>
                {o.id === props.session.owner && <span className="tag">you</span>}
                <div className="muted small">Since {when(o.createdAt)}</div>
              </div>
              {owners.length > 1 && confirm?.id !== o.id && (
                <button type="button" disabled={busy} onClick={() => setConfirm(o)}>
                  Remove
                </button>
              )}
              {confirm?.id === o.id && (
                <div className="confirm" role="group" aria-label="Confirm removal">
                  <p>
                    Remove {o.id === props.session.owner ? 'yourself' : o.label || 'this owner'}? Their passkeys and
                    recovery codes stop working. This needs a touch of your passkey.
                  </p>
                  <div className="actions">
                    <button
                      type="button"
                      className="danger"
                      disabled={busy}
                      onClick={() =>
                        void act(async () => {
                          await withTouch(() => post('owners/remove', { owner: o.id }));
                          setConfirm(null);
                          if (o.id === props.session.owner) {
                            props.onSignedOut();
                            return;
                          }
                          setDone('Owner removed.');
                          await load();
                        })
                      }
                    >
                      Yes, remove
                    </button>
                    <button type="button" className="link" disabled={busy} onClick={() => setConfirm(null)}>
                      Cancel
                    </button>
                  </div>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
      {done && <p role="status" className="ok">{done}</p>}
      {error && <p role="alert" className="error">{error}</p>}

      <h2>Invite a second owner</h2>
      {full ? (
        <p className="muted">The console has {MAX_OWNERS} owners. Remove one to invite another.</p>
      ) : invite ? (
        <div className="invite">
          <p>Give this code to the second owner. They open this console, choose "Have an invite code?", and enter it.</p>
          <p className="mono big" aria-label="Invite code">{invite.code}</p>
          <p className="muted small">It works once, until {when(invite.expiresAt)}.</p>
          <button
            type="button"
            className="link"
            onClick={() => {
              setInvite(null);
              void act(load);
            }}
          >
            Done
          </button>
        </div>
      ) : (
        <>
          <p className="muted">This needs a touch of your passkey. The code works once, for 24 hours.</p>
          <button
            type="button"
            disabled={busy || owners === null}
            onClick={() =>
              void act(async () => {
                setInvite(await withTouch(() => post<Invite>('owners/invite')));
              })
            }
          >
            Make an invite code
          </button>
        </>
      )}
    </section>
  );
}
