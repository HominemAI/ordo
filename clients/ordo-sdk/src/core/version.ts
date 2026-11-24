/**
 * The program version the door serves a tenant, read off its answers.
 *
 * Every answer past the door carries `x-ordo-tenant-version`
 * (`plan-versions.md` §1.4, §1.9). A client keeps one {@link VersionWatch}.
 * The first answer sets the baseline and fires nothing; a later answer that
 * names another version fires each listener with `(from, to)`. One place
 * reads the header: the SDK's client, the React client and the sync manager
 * all take their signal from here.
 */

export const TENANT_VERSION_HEADER = 'x-ordo-tenant-version';

export class VersionWatch {
  #current: string | null = null;
  readonly #listeners = new Set<(from: string, to: string) => void>();

  /** The version the door last named, or `null` before any answer did. */
  get current(): string | null {
    return this.#current;
  }

  /** Read one answer. Returns the answer, so a caller can chain it. */
  observe(response: Response): Response {
    const seen = response.headers.get(TENANT_VERSION_HEADER);
    if (seen !== null && seen !== this.#current) {
      const from = this.#current;
      this.#current = seen;
      if (from !== null) for (const l of [...this.#listeners]) l(from, seen);
    }
    return response;
  }

  /** Call `listener(from, to)` on a change. Returns the stop function. */
  onChange(listener: (from: string, to: string) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }
}
