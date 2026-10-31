/**
 * The settings screens' calls (docs/plan-console.md 4.2, 4.6 and 4.10).
 * One read: every setting as the deployment took it (std.deployment.settings
 * at the door), beside the console's live revision. A change goes through
 * `runtime/apply`, the same as on the Runtime screen.
 */

import { get } from './api';

export type Source = 'default' | 'environment' | 'file' | 'stored';

/** What the console's live revision gives a setting. Never a secret's value. */
export interface ConsoleSide {
  set: boolean;
  value?: string;
  secret?: 'managed' | 'referenced';
  changedAt?: number;
}

export interface Drift {
  kind: 'not-taken' | 'stored' | 'outside';
  message: string;
}

export interface SettingRow {
  kind: 'setting' | 'layer';
  name: string;
  group: string;
  default?: string | null;
  range?: string | null;
  /** The family it belongs to, such as ORDO_AUTH_<NAME>_CLIENT_ID. */
  family?: string | null;
  listed: boolean;
  secret: boolean;
  /** A change to it goes under `secrets`: the console seals it. */
  sealed?: boolean;
  /** `null` when the deployment did not answer. */
  set: boolean | null;
  source: Source | null;
  var?: string | null;
  /** The effective value. Never a secret's. */
  value?: string | null;
  /** When a stored value wins: what the environment says (a secret: set or unset). */
  environment?: string | null;
  storedBy?: string | null;
  stored?: { op: string; updatedBy: string; updatedAt: number } | null;
  what?: string;
  /** Absent in door-only mode. */
  console?: ConsoleSide;
  /** Absent when there is nothing to compare. */
  drift?: Drift | null;
}

export interface SettingsView {
  mode: 'supervise' | 'door-only';
  editable: boolean;
  revision: number | null;
  /** Why the deployment's own answer is missing, if it is. */
  doorError: string | null;
  secretSettings: string[];
  rows: SettingRow[];
}

export const loadSettings = (): Promise<SettingsView> => get<SettingsView>('settings');

/** The groups the Sign-in settings screen shows (docs/settings.md sections 1 to 3). */
export const SIGN_IN_GROUPS = [
  'Sign-in and sessions',
  'Enrolment and admission',
  'Passkeys and presence',
  'Set live by the app developer',
];

export const SOURCE_WORDS: Record<Source, string> = {
  default: 'Default',
  environment: 'Environment',
  file: 'File',
  stored: 'Stored by an op',
};

/** Markdown backticks from docs/settings.md, as plain text. */
export function plain(text: string | null | undefined): string {
  return (text ?? '').replace(/`/g, '');
}
