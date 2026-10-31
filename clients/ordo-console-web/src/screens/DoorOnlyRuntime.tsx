import { Fragment } from 'react';
import type { DoorOnly, Reported } from '../api';
import { when } from './runtimeParts';

const BOOT_FACTS: [string, string][] = [
  ['revision', 'Revision'],
  ['applied_by', 'Applied by'],
  ['version', 'Version'],
  ['image', 'Image'],
  ['binary_sha256', 'Binary (SHA-256)'],
  ['settings_fingerprint', 'Settings fingerprint'],
];

function shown(v: unknown): string {
  if (v === null || v === undefined || v === '') return 'Not reported';
  return typeof v === 'string' || typeof v === 'number' ? String(v) : JSON.stringify(v);
}

/** A part of the door's answer: the sentence of its error, or null. */
function failed(part: Reported | undefined): string | null {
  if (!part?.error) return null;
  return part.message ?? part.error;
}

/**
 * The Runtime screen in door-only mode (docs/plan-console.md 9.2): read-only.
 * The console reaches the deployment only through its door, so it shows what
 * the deployment reports (`/health/ready`, and the boot record from
 * `/health/deep`) and nothing about the process.
 */
export function DoorOnlyRuntime(props: { data: DoorOnly }) {
  const { data } = props;
  const reported = data.reported ?? null;
  const boot = reported?.boot ?? null;
  const readyError = failed(reported?.ready);
  const deepError = failed(reported?.deep);
  return (
    <section className="card" data-testid="runtime-door-only">
      <h1>Runtime</h1>
      <p>
        This console does not manage this deployment's process. It cannot start, stop, restart or change the
        deployment. Change its settings on the platform that runs it.
      </p>
      <p className="muted">{data.message}</p>
      <h2>What the deployment reports</h2>
      {data.doorError ? (
        <p role="status">{data.doorError}</p>
      ) : data.answering === false ? (
        <p role="status" data-testid="not-answering">
          The deployment is not answering.
        </p>
      ) : (
        <dl className="facts" aria-label="What the deployment reports">
          <dt>Ready</dt>
          <dd data-testid="door-ready">{readyError ?? shown(reported?.ready?.status)}</dd>
          {boot ? (
            BOOT_FACTS.map(([key, label]) => (
              <Fragment key={key}>
                <dt>{label}</dt>
                <dd>{key === 'revision' && boot[key] == null ? 'Not started by a console' : shown(boot[key])}</dd>
              </Fragment>
            ))
          ) : (
            <>
              <dt>Boot record</dt>
              <dd>{deepError ? `Not available: ${deepError}` : 'Not reported'}</dd>
            </>
          )}
          {boot && typeof boot.at === 'number' && (
            <>
              <dt>Started</dt>
              <dd>{when(boot.at)}</dd>
            </>
          )}
        </dl>
      )}
    </section>
  );
}
