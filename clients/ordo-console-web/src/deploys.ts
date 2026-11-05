/**
 * The app's program at the deployment (docs/plan-console.md 4.3): the
 * console's `deploys` routes, which reach the deployment's
 * `std.deployment.versions`, `plan`, `apply`, `activate`, and the revoke
 * ops `revoke`, `unrevoke` and `revoked` (docs/plan-versions.md 4, 2b).
 */

import { get, post } from './api';

/** A version's revoke mark. */
export interface RevokeMark {
  reason: string;
  actor: string;
  at: number;
}

export interface StoredVersion {
  version: string;
  /** Its order number, `#n`. */
  ordinal: number | null;
  label: string | null;
  gitCommit: string | null;
  appliedAt: number;
  actor: string;
  serving: boolean;
  /** How many tenants are on it. */
  tenants?: number;
  revoked: RevokeMark | null;
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

/** `std.deployment.revoke` and `unrevoke`. */
export interface Revoked {
  version: string;
  ordinal: number | null;
  revoked: RevokeMark | null;
  moved?: string | null;
}

/** One revoked version and the tenants still on it (`std.deployment.revoked`). */
export interface RevokedInUse {
  version: string;
  ordinal: number | null;
  reason: string;
  actor: string;
  at: number;
  tenantCount: number;
  tenants: { space: string; name: string; state: string }[];
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
export const revokedInUse = (): Promise<{ versions: RevokedInUse[] }> =>
  get<{ versions: RevokedInUse[] }>('deploys/revoked');
export const revoke = (version: string, reason: string, moveTo: string | null): Promise<Revoked> =>
  post<Revoked>('deploys/revoke', { version, reason, ...(moveTo ? { moveTo } : {}) });
export const unrevoke = (version: string, reason: string): Promise<Revoked> =>
  post<Revoked>('deploys/unrevoke', { version, reason });

/** The three version settings, stored in Ordo (owner, 2026-11-05). `null` is unset. */
export interface VersionSettings {
  served: string | null;
  minimum: string | null;
  newTenant: string | null;
  history?: { setting: string; value: string | null; actor: string; reason: string; at: number }[];
}

/** What a change answers: the settings now, and whether this one changed. */
export interface SettingChanged extends VersionSettings {
  setting: string;
  value: string | null;
  changed: boolean;
}

export type VersionSettingName = 'versions.served' | 'versions.minimum' | 'versions.new_tenant';

export const versionSettings = (): Promise<VersionSettings> => get<VersionSettings>('deploys/settings');
export const setVersionSetting = (
  setting: VersionSettingName,
  value: string | null,
  reason: string,
): Promise<SettingChanged> => post<SettingChanged>('deploys/setting', { setting, reason, ...(value ? { value } : {}) });

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
