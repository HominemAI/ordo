import { useCallback, useEffect, useRef, useState, type DependencyList } from 'react';
import { friendly } from './errors.js';

export interface Loaded<T> {
  readonly data: T | undefined;
  readonly error: string | null;
  readonly loading: boolean;
  readonly reload: () => void;
}

/**
 * Run a read when `deps` change, and again on `reload()`. A slower answer to
 * an older request never replaces a newer one.
 */
export function useLoad<T>(read: () => Promise<T>, deps: DependencyList, enabled = true): Loaded<T> {
  const [data, setData] = useState<T | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [tick, setTick] = useState(0);
  const latest = useRef(0);

  useEffect(() => {
    if (!enabled) return;
    const mine = ++latest.current;
    setLoading(true);
    read().then(
      (value) => {
        if (mine !== latest.current) return;
        setData(value);
        setError(null);
        setLoading(false);
      },
      (e: unknown) => {
        if (mine !== latest.current) return;
        setError(friendly(e, window.location.origin));
        setLoading(false);
      },
    );
    // `read` is rebuilt every render; `deps` says when it changes.
  }, [...deps, tick, enabled]);

  const reload = useCallback(() => setTick((n) => n + 1), []);
  return { data, error, loading, reload };
}

/** A write in flight: one at a time, with its error in words. */
export function useAct(): {
  readonly busy: boolean;
  readonly error: string | null;
  readonly run: (work: () => Promise<unknown>) => Promise<boolean>;
  readonly clear: () => void;
} {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useCallback(async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await work();
      return true;
    } catch (e) {
      setError(friendly(e, window.location.origin));
      return false;
    } finally {
      setBusy(false);
    }
  }, []);
  return { busy, error, run, clear: () => setError(null) };
}
