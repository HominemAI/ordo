/**
 * What the Health and Runtime screens both show about the supervised
 * deployment: its state in plain words, its last exit, its log lines.
 */

import type { ChildState, LastExit, RuntimeStatus } from '../api';

export function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'medium' });
}

export const STATE_NAME: Record<ChildState, string> = {
  starting: 'Starting',
  ready: 'Ready',
  restarting: 'Restarting',
  'crash-looping': 'Crash-looping',
  held: 'Held',
  stopped: 'Stopped',
};

export const STATE_MEANS: Record<ChildState, string> = {
  starting: 'The deployment is starting. It counts as ready once it answers its health check.',
  ready: 'The deployment is running and answers its health check.',
  restarting:
    'The deployment stopped. The console starts it again after a short wait: 1 second, then 2, 4 and so on, up to 60.',
  'crash-looping':
    'The deployment stopped 5 times in 10 minutes. The console tries again every 60 seconds. Roll back to a revision that worked, or change the settings.',
  held: 'The deployment kept stopping after an automatic roll back. The console makes no more changes by itself. Apply a change or roll back to start it again.',
  stopped: 'The console is shutting down, so the deployment is stopped.',
};

const SIGNALS: Record<number, string> = {
  1: 'SIGHUP',
  2: 'SIGINT',
  6: 'SIGABRT',
  9: 'SIGKILL',
  11: 'SIGSEGV',
  15: 'SIGTERM',
};

/** One line for how the deployment last stopped. */
export function exitWords(e: LastExit): string {
  if (e.error) return `Could not start: ${e.error}`;
  if (e.signal !== null) {
    const name = SIGNALS[e.signal];
    return `Stopped by signal ${e.signal}${name ? ` (${name})` : ''}`;
  }
  if (e.code !== null) return `Exited with code ${e.code}`;
  return 'Exited';
}

/** State, revision, restarts and last exit, as a list of facts. */
export function StatusFacts(props: { status: RuntimeStatus }) {
  const s = props.status;
  return (
    <>
      <p className="state">
        <span className={`pill pill-${s.state}`} data-state={s.state}>
          {STATE_NAME[s.state]}
        </span>
        <span>{STATE_MEANS[s.state]}</span>
      </p>
      <dl className="facts">
        <dt>Live revision</dt>
        <dd data-testid="live-revision">{s.liveRevision}</dd>
        {s.applying !== null && (
          <>
            <dt>Bringing up</dt>
            <dd>Revision {s.applying}</dd>
          </>
        )}
        <dt>Restarts</dt>
        <dd data-testid="restarts">{s.restarts}</dd>
        {s.readySince !== null && s.state === 'ready' && (
          <>
            <dt>Ready since</dt>
            <dd>{when(s.readySince)}</dd>
          </>
        )}
        <dt>Last exit</dt>
        <dd>{s.lastExit ? `${exitWords(s.lastExit)}, ${when(s.lastExit.at)}` : 'None since the console started'}</dd>
        {s.autoChangesHeld && (
          <>
            <dt>Automatic changes</dt>
            <dd>Stopped until you apply a change or roll back</dd>
          </>
        )}
      </dl>
    </>
  );
}

/** The last lines the deployment wrote, folded away unless `open`. */
export function LogLines(props: { lines: string[]; open?: boolean }) {
  return (
    <details className="logs" open={props.open}>
      <summary>Last log lines ({props.lines.length})</summary>
      {props.lines.length === 0 ? (
        <p className="muted small">Nothing yet.</p>
      ) : (
        <pre className="mono small">{props.lines.join('\n')}</pre>
      )}
    </details>
  );
}
