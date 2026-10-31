import { useCallback, useEffect, useRef, useState } from 'react';
import { ConsoleCallError, explain, get, post, SIGNED_OUT } from '../api';
import {
  doorGet,
  link,
  needsDeploymentTouch,
  touchDeployment,
  type ConsoleKey,
  type DoorRequest,
  type Link,
} from '../deployment';
import { withTouch } from '../touch';

function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/**
 * The link to the deployment (docs/plan-console.md 8): sign in with the
 * owner's passkey there, the console keys, rotation and unlink. The claim
 * step was removed with the claim code (2026-10-30): the deployment names
 * its owner (ORDO_DEPLOYMENT_OWNERS), or the identity service replaces the
 * link altogether.
 * Every write at the deployment needs the owner's own touch there, asked
 * for in a dialog.
 */
export function Deployment(props: { onSignedOut: () => void }) {
  const [state, setState] = useState<Link | null>(null);
  const [keys, setKeys] = useState<ConsoleKey[] | null>(null);
  const [refused, setRefused] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [confirmUnlink, setConfirmUnlink] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
  const pending = useRef<{ resolve: () => void; reject: (e: unknown) => void } | null>(null);

  const load = useCallback(async () => {
    const s = await link();
    setState(s);
    setRefused(false);
    if (!s.linked) {
      setKeys(null);
      return;
    }
    try {
      setKeys((await get<{ keys: ConsoleKey[] }>('door/keys')).keys);
    } catch (e) {
      if (e instanceof ConsoleCallError && e.code === 'console_key_refused') {
        setRefused(true);
        setKeys(null);
        return;
      }
      throw e;
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
      }
    },
    [props],
  );

  useEffect(() => {
    void act(load);
  }, []);

  /** Ask for the touch at the deployment in the dialog; settle when it is done or dismissed. */
  const askTouch = () =>
    new Promise<void>((resolve, reject) => {
      pending.current = { resolve, reject };
      dialog.current?.showModal();
    });

  /** Run a write at the deployment; when it asks for the owner's touch there, ask and run it again. */
  const atDeployment = async <T,>(f: () => Promise<T>): Promise<T> => {
    try {
      return await withTouch(f);
    } catch (e) {
      if (!needsDeploymentTouch(e)) throw e;
    }
    await askTouch();
    return withTouch(f);
  };

  const touchNow = async () => {
    const p = pending.current;
    try {
      await touchDeployment();
      dialog.current?.close();
      pending.current = null;
      p?.resolve();
    } catch (e) {
      dialog.current?.close();
      pending.current = null;
      p?.reject(e);
    }
  };

  const linkNow = () =>
    act(async () => {
      const begun = await withTouch(() => post<DoorRequest>('door/link/begin', { kind: 'sign-in' }));
      const credential = await doorGet(begun);
      await withTouch(() =>
        post('door/link/finish', {
          kind: 'sign-in',
          credential,
          ...(name.trim() ? { name: name.trim() } : {}),
        }),
      );
      setDone('Linked. The console now holds its own key for the deployment.');
      await load();
    });

  return (
    <section className="card">
      <h1>Deployment</h1>
      {state === null && !error && <p className="muted">Loading…</p>}

      {state?.via === 'identity-service' && (
        <>
          <p>
            You sign in at the identity service, and the console calls the deployment at{' '}
            <span className="mono">{state.door}</span> as you. There is no console key and nothing to link.
          </p>
          {state.reachable ? (
            <p className="ok">The deployment answers.</p>
          ) : (
            <p role="alert" className="error">
              The deployment did not answer{state.reachError ? `: ${state.reachError}` : '.'}
            </p>
          )}
          <div className="actions">
            <button type="button" disabled={busy} onClick={() => void act(load)}>
              Check again
            </button>
          </div>
        </>
      )}

      {state && !state.linked && state.via !== 'identity-service' && (
        <>
          <p>
            Link this console to the deployment at <span className="mono">{state.door}</span>. You use your passkey for
            the deployment once; the console then signs in with a key of its own.
          </p>
          <p className="muted small">
            Your browser runs the deployment's passkey here, so the deployment must list this address,{' '}
            <span className="mono">{window.location.origin}</span>, in ORDO_AUTH_ORIGINS when it shares the
            deployment's domain, or in ORDO_AUTH_RELATED_ORIGINS when it does not.
          </p>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void linkNow();
            }}
          >
            <p id="how-to-link" className="note">
              {state.howToLink ??
                'Use the passkey you hold at the deployment. The deployment must name you its owner in ORDO_DEPLOYMENT_OWNERS.'}
            </p>
            <label htmlFor="key-name">Name for this console's key (optional)</label>
            <input id="key-name" maxLength={64} value={name} onChange={(e) => setName(e.target.value)} />
            <div className="actions">
              <button type="submit" className="primary" disabled={busy} aria-describedby="how-to-link">
                Link with your passkey
              </button>
            </div>
          </form>
        </>
      )}

      {state?.linked && (
        <>
          <dl className="facts">
            <dt>Deployment</dt>
            <dd className="mono">{state.door}</dd>
            <dt>Key</dt>
            <dd>
              {state.name} <span className="mono small">{state.key}</span>
            </dd>
            {state.linkedAt !== undefined && (
              <>
                <dt>Enrolled</dt>
                <dd>{when(state.linkedAt)}</dd>
              </>
            )}
            {state.rotateDueAt !== undefined && (
              <>
                <dt>Rotation</dt>
                <dd>
                  {state.rotateDue ? (
                    <strong>Due now</strong>
                  ) : (
                    <>Due {when(state.rotateDueAt)}</>
                  )}{' '}
                  <span className="muted small">every {state.rotateDays} days (ORDO_CONSOLE_KEY_ROTATE_DAYS)</span>
                </dd>
              </>
            )}
            <dt>Your touch</dt>
            <dd>
              {state.presentUntil && state.presentUntil * 1000 > Date.now()
                ? `Counts until ${when(state.presentUntil)}`
                : 'None now. A change at the deployment asks for one.'}
            </dd>
          </dl>
          {refused && (
            <div className="note" role="alert">
              <p>
                The deployment refused this console's key. It was revoked there, or the deployment was reset. Forget
                the link here, then link again.
              </p>
              <button
                type="button"
                className="danger"
                disabled={busy}
                onClick={() =>
                  void act(async () => {
                    await withTouch(() => post('door/forget'));
                    setDone('The link is forgotten. Link again below.');
                    await load();
                  })
                }
              >
                Forget the link
              </button>
            </div>
          )}
          {!refused && (
            <div className="actions">
              <button
                type="button"
                className={state.rotateDue ? 'primary' : ''}
                disabled={busy}
                onClick={() =>
                  void act(async () => {
                    const r = await atDeployment(() => post<{ key: string }>('door/rotate', {}));
                    setDone(`Rotated. The new key is ${r.key}; the old one is revoked.`);
                    await load();
                  })
                }
              >
                Rotate the key now
              </button>
              {!confirmUnlink && (
                <button type="button" className="link" disabled={busy} onClick={() => setConfirmUnlink(true)}>
                  Unlink
                </button>
              )}
            </div>
          )}
          {confirmUnlink && (
            <div className="confirm" role="group" aria-label="Confirm unlink">
              <p>
                Unlink? The deployment revokes this console's key and ends its sessions. The deployment keeps running.
              </p>
              <div className="actions">
                <button
                  type="button"
                  className="danger"
                  disabled={busy}
                  onClick={() =>
                    void act(async () => {
                      await atDeployment(() => post('door/unlink'));
                      setConfirmUnlink(false);
                      setDone('Unlinked.');
                      await load();
                    })
                  }
                >
                  Yes, unlink
                </button>
                <button type="button" className="link" disabled={busy} onClick={() => setConfirmUnlink(false)}>
                  Cancel
                </button>
              </div>
            </div>
          )}

          {keys && (
            <>
              <h2>Console keys at the deployment</h2>
              <ul className="owners" aria-label="Console keys">
                {keys.map((k) => (
                  <li key={k.id}>
                    <div>
                      <strong>{k.name}</strong>
                      {k.thisConsole && <span className="tag">this console</span>}
                      {k.revokedAt !== null && <span className="tag">revoked</span>}
                      <div className="muted small">
                        Since {when(k.createdAt)}
                        {k.lastUsedAt !== null && <>, last used {when(k.lastUsedAt)}</>}
                      </div>
                    </div>
                    {!k.thisConsole && k.revokedAt === null && (
                      <button
                        type="button"
                        className="danger"
                        disabled={busy}
                        aria-label={`Revoke ${k.name}`}
                        onClick={() =>
                          void act(async () => {
                            await atDeployment(() => post('door/keys/revoke', { id: k.id }));
                            setDone(`Revoked ${k.name}.`);
                            await load();
                          })
                        }
                      >
                        Revoke
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </>
          )}
        </>
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

      <dialog
        ref={dialog}
        aria-labelledby="touch-title"
        onCancel={() => {
          pending.current?.reject(new DOMException('The touch was cancelled.', 'AbortError'));
          pending.current = null;
        }}
      >
        <h2 id="touch-title">Touch your passkey for the deployment</h2>
        <p>
          This change is made at the deployment, which asks for your own passkey there. One touch covers changes for
          the next 5 minutes.
        </p>
        <div className="actions">
          <button type="button" className="primary" onClick={() => void touchNow()}>
            Use my passkey
          </button>
          <button
            type="button"
            className="link"
            onClick={() => {
              dialog.current?.close();
              pending.current?.reject(new DOMException('The touch was cancelled.', 'AbortError'));
              pending.current = null;
            }}
          >
            Cancel
          </button>
        </div>
      </dialog>
    </section>
  );
}
