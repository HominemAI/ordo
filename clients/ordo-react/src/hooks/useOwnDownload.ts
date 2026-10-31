/**
 * `useOwnDownload`: a person's own download, `GET /download`, as a verb and
 * its state (owner ruling 2026-10-22).
 *
 * `download()` answers the zip and its name, or `null` when it was refused or
 * failed; `error` then holds why. Saving the file is the page's choice: this
 * package renders nothing and touches no DOM.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { OwnDownload } from '../client.js';
import { useOrdo } from './context.js';

export interface UseOwnDownload {
  readonly download: () => Promise<OwnDownload | null>;
  /** A download is being made. */
  readonly busy: boolean;
  /** Why the last download did not arrive, or `null`. */
  readonly error: Error | null;
}

export function useOwnDownload(): UseOwnDownload {
  const client = useOrdo();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const download = useCallback(async (): Promise<OwnDownload | null> => {
    setBusy(true);
    setError(null);
    try {
      return await client.downloadOwn();
    } catch (e) {
      if (mounted.current) setError(e instanceof Error ? e : new Error(String(e)));
      return null;
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [client]);

  return { download, busy, error };
}
