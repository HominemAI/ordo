import { useEffect, useRef, useState } from 'react';
import { ConsoleCallError, explain, get, ready, SIGNED_OUT, type DoorOnly, type Ready, type Runtime } from '../api';
import type { Link } from '../deployment';
import { LogLines, StatusFacts } from './runtimeParts';

/** Health from the deployment's door (docs/plan-console.md 4.1). */
interface DoorHealth {
  ready: { status?: string; error?: string; message?: string };
  deep: { status?: string; boot?: { revision?: string | null; applied_by?: string | null }; error?: string; message?: string };
  deploy: { nodes?: unknown[]; error?: string; message?: string };
  link: Link;
}

function DoorPart(props: { label: string; part: { status?: string; error?: string; message?: string } }) {
  return (
    <>
      <dt>{props.label}</dt>
      <dd>{props.part.error ? <span className="error">{props.part.message ?? props.part.error}</span> : (props.part.status ?? 'ok')}</dd>
    </>
  );
}

const POLL_MS = 4000;

/**
 * The home screen (docs/plan-console.md 4.1). It leads with the
 * supervisor, so it has something true to say when the deployment is down.
 */
export function Health(props: { onSignedOut: () => void; onRuntime: () => void; onDeployment: () => void }) {
  const [runtime, setRuntime] = useState<Runtime | DoorOnly | null>(null);
  const [door, setDoor] = useState<DoorHealth | null>(null);
  const [doorNote, setDoorNote] = useState<string | null>(null);
  const [console_, setConsole] = useState<Ready | null>(null);
  const [error, setError] = useState<string | null>(null);
  const signedOut = useRef(props.onSignedOut);
  signedOut.current = props.onSignedOut;

  useEffect(() => {
    let live = true;
    let timer: number | undefined;
    const tick = async () => {
      try {
        const [r, h, d] = await Promise.all([
          get<Runtime | DoorOnly>('runtime'),
          ready().catch(() => null),
          get<DoorHealth>('door/health').catch((e: unknown) => e),
        ]);
        if (!live) return;
        setRuntime(r);
        setConsole(h);
        if (d instanceof ConsoleCallError) {
          if (SIGNED_OUT.has(d.code)) throw d;
          setDoor(null);
          setDoorNote(d.code === 'not_linked' ? 'The console is not linked to the deployment yet.' : explain(d));
        } else {
          setDoor(d as DoorHealth);
          setDoorNote(null);
        }
        setError(null);
      } catch (e) {
        if (!live) return;
        if (e instanceof ConsoleCallError && SIGNED_OUT.has(e.code)) {
          signedOut.current();
          return;
        }
        setError(explain(e));
      }
      if (live) timer = window.setTimeout(() => void tick(), POLL_MS);
    };
    void tick();
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, []);

  const down = runtime?.mode === 'supervise' && ['crash-looping', 'held'].includes(runtime.status.state);

  return (
    <section className="card">
      <h1>Health</h1>
      {runtime === null && !error && <p className="muted">Loading…</p>}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {runtime?.mode === 'door-only' && (
        <>
          <p>The console does not run the deployment here, so it cannot say whether it is running.</p>
          <p className="muted">{runtime.message}</p>
        </>
      )}
      {runtime?.mode === 'supervise' && (
        <>
          <h2>The deployment</h2>
          <StatusFacts status={runtime.status} />
          {down && (
            <>
              <LogLines lines={runtime.logs} open />
              <div className="actions">
                <button type="button" className="primary" onClick={props.onRuntime}>
                  Roll back
                </button>
                <span className="muted small">Opens Runtime, where you pick the revision.</span>
              </div>
            </>
          )}
          {!down && <LogLines lines={runtime.logs} />}
        </>
      )}
      <h2>The deployment's door</h2>
      {doorNote && (
        <p>
          {doorNote}{' '}
          <button type="button" className="link" onClick={props.onDeployment}>
            Open Deployment
          </button>
        </p>
      )}
      {door && (
        <>
          {/* With the identity service there is no link: the console calls the door as you. */}
          {!door.link.linked && door.link.via !== 'identity-service' ? (
            <p>
              The console is not linked to the deployment yet.{' '}
              <button type="button" className="link" onClick={props.onDeployment}>
                Link it
              </button>
            </p>
          ) : (
            <dl className="facts" aria-label="The deployment's door">
              <DoorPart label="Ready" part={door.ready} />
              <DoorPart label="Deep check" part={door.deep} />
              {door.deep.boot && (
                <>
                  <dt>Started by</dt>
                  <dd>
                    revision {door.deep.boot.revision ?? 'none'}, applied by {door.deep.boot.applied_by ?? 'nobody'}
                  </dd>
                </>
              )}
              <dt>Nodes</dt>
              <dd>{door.deploy.error ? <span className="error">{door.deploy.message}</span> : (door.deploy.nodes?.length ?? 0)}</dd>
            </dl>
          )}
          {door.link.rotateDue && (
            <p className="note" role="status">
              The console's key is due for rotation.{' '}
              <button type="button" className="link" onClick={props.onDeployment}>
                Rotate it
              </button>
            </p>
          )}
        </>
      )}
      {console_ && (
        <>
          <h2>The console</h2>
          <p>
            Its database is {console_.consoleDatabase === 'ok' ? 'reachable' : 'unreachable'}. The console keeps
            answering while the deployment is down.
          </p>
        </>
      )}
    </section>
  );
}
