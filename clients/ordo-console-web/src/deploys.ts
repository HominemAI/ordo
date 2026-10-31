/**
 * The app's program at the deployment (docs/plan-console.md 4.3): the
 * console's `deploys` routes, which reach the deployment's
 * `std.deployment.versions`, `plan`, `apply` and `activate`.
 */

import { get, post } from './api';

export interface StoredVersion {
  version: string;
  appliedAt: number;
  actor: string;
  serving: boolean;
}

export interface ActivationRow {
  seq: number;
  version: string;
  at: number;
  actor: string;
}

/** `std.deployment.versions`. */
export interface History {
  active: string | null;
  stored: StoredVersion[];
  activations: ActivationRow[];
}

export interface PlannedChange {
  code: string;
  change: string;
  /** A to D, plan-schema-evolution.md 5. */
  class: string;
  refused: string[];
}

/** What an apply would do to the database. */
export type Would =
  | { state: 'create'; entities: number }
  | { state: 'matches'; pending: number }
  | { state: 'migrate' }
  | { state: 'held' }
  | { state: 'refused'; class: string; code: string; message: string };

/** `std.deployment.plan`: nothing stored. */
export interface Preview {
  version: string;
  serving: string | null;
  alreadyStored: boolean;
  notes: string[];
  would: Would;
  changes: PlannedChange[];
  safeChanges: boolean;
  safeChangesSetting: string;
}

/** `std.deployment.apply`'s report. */
export interface ApplyReport {
  version: string;
  notes: string[];
  schema: { state: 'created' | 'matches' | 'migrated' | 'behind'; changes?: { code: string; change: string }[] };
  stored: 'written' | 'alreadyPresent';
  activation: { state: 'switched' | 'alreadyActive' | 'staged'; from?: string | null; to?: string };
  tenants?: { advanced: number; current: number; held: { target: string; reason: string }[] };
}

/** `std.deployment.activate`. */
export interface Activated {
  state: 'switched' | 'alreadyActive';
  version: string;
  from: string | null;
}

/** `GET /health/deploy`: which version each node serves. */
export interface Serving {
  version: string;
  settled: boolean;
  held: number;
  pending: { node: string; doing: string }[];
  nodes: { node: string; serving: string; caughtUp: string }[];
}

export const history = (): Promise<History> => get<History>('deploys');
export const serving = (): Promise<Serving> => get<Serving>('deploys/serving');
export const preview = (program: string): Promise<Preview> => post<Preview>('deploys/preview', { program });
export const apply = (program: string, activate: boolean): Promise<ApplyReport> =>
  post<ApplyReport>('deploys/apply', { program, activate });
export const activate = (version: string): Promise<Activated> => post<Activated>('deploys/activate', { version });

/** How long the screen waits for every node to serve a version. */
export const WAIT_SECS = 120;

/**
 * Poll until every node serves `version` and has caught its tenants up,
 * calling `progress` after each look. Resolves with the last look; rejects
 * after WAIT_SECS with what the nodes were still doing.
 */
export async function untilServed(version: string, progress: (s: Serving) => void): Promise<Serving> {
  const until = Date.now() + WAIT_SECS * 1000;
  for (;;) {
    const s = await serving();
    progress(s);
    if (s.version === version && s.pending.length === 0) return s;
    if (Date.now() > until) {
      const doing = s.pending.map((p) => `${p.node}: ${p.doing}`).join('; ');
      throw new Error(
        `Not every node serves it after ${WAIT_SECS} seconds. ${doing || 'The deployment serves another version.'}`,
      );
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
}
