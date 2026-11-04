/**
 * The Maintenance windows screen's calls: the console's `windows` routes,
 * which reach the deployment's `std.deployment.windows`, `setWindow`,
 * `deleteWindow`, `skipWindow` and `deleteSkip` (owner, 2026-11-04).
 *
 * A window is a block of time the deployment owner sets. It does nothing by
 * itself. Times on the wire are UTC, `YYYY-MM-DDTHH:MM:SSZ`.
 */

import { get, post } from './api';

export type Target = 'all' | 'only' | 'except';

/** One window, as stored. A range has `starts` and `ends`; a cron has `cron` and `lengthSecs`. */
export interface Window {
  id: string;
  target: Target;
  tenants: string[];
  /** Any text. Shown as plain text, never rendered. */
  message: string;
  setBy: string;
  setAt: number;
  starts?: string;
  ends?: string;
  cron?: string;
  lengthSecs?: number;
}

/** A range over which a cron window's times are skipped. */
export interface Skip {
  id: string;
  window: string;
  starts: string;
  ends: string;
  reason: string | null;
  setBy: string;
  setAt: number;
}

/** One time a window is in force. */
export interface Occurrence {
  window: string;
  starts: string;
  ends: string;
  target: Target;
  /** The skip that suppresses it, when one does. */
  skippedBy: string | null;
}

/** `std.deployment.windows`: 12 months back to 12 months ahead. */
export interface Listing {
  from: string;
  to: string;
  tenant: string | null;
  windows: Window[];
  skips: Skip[];
  occurrences: Occurrence[];
  /** The list reached its cap and stopped there. */
  truncated: boolean;
}

/** What the form sends: a range or a cron. */
export interface WindowInput {
  id?: string;
  target: Target;
  tenants: string[];
  message: string;
  schedule: { kind: 'range'; starts: string; ends: string } | { kind: 'cron'; cron: string; lengthSecs: number };
}

export const listWindows = (): Promise<Listing> => get<Listing>('windows');

export const setWindow = (w: WindowInput): Promise<Window> =>
  post<Window>('windows/set', {
    ...(w.id ? { id: w.id } : {}),
    target: w.target,
    tenants: w.target === 'all' ? [] : w.tenants,
    message: w.message,
    ...(w.schedule.kind === 'range'
      ? { starts: w.schedule.starts, ends: w.schedule.ends }
      : { cron: w.schedule.cron, lengthSecs: w.schedule.lengthSecs }),
  });

export const deleteWindow = (id: string) => post<{ id: string; deleted: boolean }>('windows/delete', { id });

export const skipWindow = (window: string, starts: string, ends: string, reason: string) =>
  post<Skip>('windows/skip', { window, starts, ends, ...(reason ? { reason } : {}) });

export const deleteSkip = (id: string) => post<{ id: string; deleted: boolean }>('windows/delete-skip', { id });

/** A door time as milliseconds since 1970. */
export const ms = (utc: string): number => Date.parse(utc);

/** A `datetime-local` value (the viewer's local time) as a door time, or '' when blank. */
export function localToUtc(local: string): string {
  if (!local) return '';
  const d = new Date(local);
  return Number.isNaN(d.getTime()) ? '' : d.toISOString().replace(/\.\d{3}Z$/, 'Z');
}

/** A door time as a `datetime-local` value in the viewer's local time. */
export function utcToLocal(utc: string | undefined): string {
  if (!utc) return '';
  const d = new Date(utc);
  if (Number.isNaN(d.getTime())) return '';
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** A length in seconds, in words: `2 h 30 min`. */
export function lengthWords(secs: number): string {
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const parts = [d ? `${d} d` : '', h ? `${h} h` : '', m ? `${m} min` : ''].filter(Boolean);
  return parts.length ? parts.join(' ') : `${secs} s`;
}
