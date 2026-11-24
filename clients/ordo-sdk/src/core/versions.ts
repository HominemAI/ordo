/**
 * The tenant's program version, as every answer names it
 * (`plan-versions.md` §1.9).
 *
 * The door puts `x-ordo-tenant-version` on every answer past it: the version
 * serving the tenant the call was bound to. A client remembers the last one
 * it saw for each tenant. When an answer names another, the tenant moved,
 * and the client raises a "version changed" event. The sync manager listens
 * and drops the tenant's device copy, as it does on the feed's
 * `version_changed` frame (owner ruling Q7, 2026-11-23).
 */

import { TENANT_VERSION_HEADER } from './version.js';

export { TENANT_VERSION_HEADER };

/** A tenant moved from one program version to another. */
export interface VersionChange {
  /** The tenant. */
  readonly space: string;
  /** The version the last answer before this one named. */
  readonly from: string;
  /** The version this answer names. */
  readonly to: string;
}

/**
 * The last version seen for each tenant, shared by a client and every
 * client {@link import('./client.js').OrdoClient.withSpace} makes from it.
 */
export class TenantVersions {
  readonly #seen = new Map<string, string>();
  readonly #changed = new Set<(change: VersionChange) => void>();
  readonly #heard = new Set<(space: string, version: string) => void>();

  /** The last version an answer named for `space`, or `null`. */
  last(space: string): string | null {
    return this.#seen.get(space) ?? null;
  }

  /**
   * Note the version an answer for `space` named. The first one seen is
   * remembered; a different one after it raises the change. An answer with
   * no version (an older door, a refusal before the door) changes nothing.
   */
  saw(space: string, version: string | null): VersionChange | null {
    if (!version) return null;
    const before = this.#seen.get(space);
    this.#seen.set(space, version);
    for (const listener of [...this.#heard]) listener(space, version);
    if (before === undefined || before === version) return null;
    const change: VersionChange = { space, from: before, to: version };
    for (const listener of [...this.#changed]) listener(change);
    return change;
  }

  /** Note the version a response names. */
  sawResponse(space: string, response: Response): VersionChange | null {
    return this.saw(space, response.headers.get(TENANT_VERSION_HEADER));
  }

  /** Call `listener` on every change. Answers the function that stops it. */
  onChange(listener: (change: VersionChange) => void): () => void {
    this.#changed.add(listener);
    return () => this.#changed.delete(listener);
  }

  /**
   * Call `listener` with every version an answer names, a change or not.
   * The sync manager uses it to tell a move it already handled from a new
   * one. Answers the function that stops it.
   * @internal
   */
  onEvery(listener: (space: string, version: string) => void): () => void {
    this.#heard.add(listener);
    return () => this.#heard.delete(listener);
  }
}
