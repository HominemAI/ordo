import type { StoredVersion } from '../deploys';

/** A time in seconds since 1970, in the viewer's words. */
export function when(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** A version hash, short enough to read; the full one goes in a title. */
export function short(v: string): string {
  return v.length > 16 ? `${v.slice(0, 16)}…` : v;
}

/** `#3 0.2.0`: a version's order number and its short name, from the stored list. */
export function versionName(version: string | null | undefined, stored: StoredVersion[]): string {
  if (!version) return 'none';
  const v = stored.find((s) => s.version === version);
  return [v?.ordinal ? `#${v.ordinal}` : '', short(version)].filter(Boolean).join(' ');
}
