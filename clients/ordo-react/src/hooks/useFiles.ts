/**
 * Files in React: upload with progress, show an image, offer a download.
 *
 * Headless, as every hook here is. Each returns state and verbs; what a
 * progress bar or a thumbnail looks like is the app developer's. The bytes
 * go through the blob door with the client's own token and space, so the row
 * guard decides who reads (`../files.ts`).
 *
 * - `useFileUpload()`: `upload(target, file)` with `progress`.
 * - `useFile(handle)`: the bytes as an object URL for `<img src>`, released
 *   when the handle changes or the component goes.
 * - `useFileInfo(handle)`: the media type and size only, no bytes. For a
 *   list that shows "report.pdf, 1.2 MB" and fetches nothing.
 * - `useFileDownload()`: `download(handle, name)` with `progress`.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import {
  downloadFile,
  fileInfo,
  fileUrl,
  uploadFile,
  type FileInfo,
  type FileProgress,
  type FileTarget,
  type ShownFile,
  type UploadOptions,
} from '../files.js';
import { useOrdo } from './context.js';

/** Where a transfer stands. */
export type TransferPhase = 'idle' | 'working' | 'done' | 'failed';

export interface UseFileUpload {
  readonly phase: TransferPhase;
  /** The latest progress, or `null` before the first. */
  readonly progress: FileProgress | null;
  /** The last failure: an `OrdoRefusal`, an `OrdoNetworkError` or the fetch error. `null` otherwise. */
  readonly error: unknown;
  /**
   * Send a file's bytes to its row. Resolves when stored. A failure is put
   * in `error` and thrown again, so a caller may `await` and `catch` or only
   * read the state.
   */
  upload(target: FileTarget, file: Blob, options?: Pick<UploadOptions, 'maxBytes'>): Promise<void>;
  /** Stop the upload that is running. */
  cancel(): void;
  /** Back to `idle`. */
  reset(): void;
}

function isAbort(e: unknown): boolean {
  return typeof e === 'object' && e !== null && (e as { name?: unknown }).name === 'AbortError';
}

export function useFileUpload(): UseFileUpload {
  const client = useOrdo();
  const [phase, setPhase] = useState<TransferPhase>('idle');
  const [progress, setProgress] = useState<FileProgress | null>(null);
  const [error, setError] = useState<unknown>(null);
  const ctl = useRef<AbortController | null>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      ctl.current?.abort();
    };
  }, []);

  const upload = useCallback(
    async (target: FileTarget, file: Blob, options: Pick<UploadOptions, 'maxBytes'> = {}) => {
      ctl.current?.abort();
      const mine = new AbortController();
      ctl.current = mine;
      setPhase('working');
      setError(null);
      setProgress({ loaded: 0, total: file.size, fraction: file.size === 0 ? 1 : 0 });
      try {
        await uploadFile(client, target, file, {
          ...options,
          signal: mine.signal,
          onProgress: (p) => {
            if (alive.current && ctl.current === mine) setProgress(p);
          },
        });
        if (alive.current && ctl.current === mine) setPhase('done');
      } catch (e) {
        if (alive.current && ctl.current === mine) {
          if (isAbort(e)) {
            setPhase('idle');
            setProgress(null);
          } else {
            setPhase('failed');
            setError(e);
          }
        }
        throw e;
      }
    },
    [client],
  );
  const cancel = useCallback(() => ctl.current?.abort(), []);
  const reset = useCallback(() => {
    ctl.current?.abort();
    setPhase('idle');
    setProgress(null);
    setError(null);
  }, []);
  return { phase, progress, error, upload, cancel, reset };
}

export interface UseFile {
  /** `idle` with no handle, `working` while reading, `done` with the URL, `failed` with `error`. */
  readonly phase: TransferPhase;
  /** An object URL for `<img src>`, `<a href>` or `<video src>`, or `null`. */
  readonly url: string | null;
  readonly type: string | null;
  readonly size: number | null;
  readonly progress: FileProgress | null;
  readonly error: unknown;
  /** Read it again. */
  reload(): void;
}

/**
 * The bytes of a file as an object URL. A `null` or `undefined` handle
 * (the row's `blob` field is empty until the bytes arrive) leaves the hook
 * `idle`. The URL is released when the handle changes or the component goes.
 */
export function useFile(handle: string | null | undefined): UseFile {
  const client = useOrdo();
  const [shown, setShown] = useState<ShownFile | null>(null);
  const [phase, setPhase] = useState<TransferPhase>('idle');
  const [progress, setProgress] = useState<FileProgress | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    if (!handle) {
      setShown(null);
      setPhase('idle');
      setProgress(null);
      setError(null);
      return;
    }
    const ctl = new AbortController();
    let mine: ShownFile | null = null;
    let gone = false;
    setShown(null);
    setPhase('working');
    setError(null);
    fileUrl(client, handle, {
      signal: ctl.signal,
      onProgress: (p) => {
        if (!gone) setProgress(p);
      },
    }).then(
      (s) => {
        if (gone) {
          s.release();
          return;
        }
        mine = s;
        setShown(s);
        setPhase('done');
      },
      (e: unknown) => {
        if (gone || isAbort(e)) return;
        setPhase('failed');
        setError(e);
      },
    );
    return () => {
      gone = true;
      ctl.abort();
      mine?.release();
    };
  }, [client, handle, tick]);

  const reload = useCallback(() => setTick((n) => n + 1), []);
  return {
    phase,
    url: shown?.url ?? null,
    type: shown?.type ?? null,
    size: shown?.size ?? null,
    progress,
    error,
    reload,
  };
}

export interface UseFileInfo {
  readonly phase: TransferPhase;
  readonly info: FileInfo | null;
  readonly error: unknown;
}

/** A file's media type and size, with no bytes read. */
export function useFileInfo(handle: string | null | undefined): UseFileInfo {
  const client = useOrdo();
  const [phase, setPhase] = useState<TransferPhase>('idle');
  const [info, setInfo] = useState<FileInfo | null>(null);
  const [error, setError] = useState<unknown>(null);
  useEffect(() => {
    setInfo(null);
    setError(null);
    if (!handle) {
      setPhase('idle');
      return;
    }
    const ctl = new AbortController();
    let gone = false;
    setPhase('working');
    fileInfo(client, handle, { signal: ctl.signal }).then(
      (i) => {
        if (gone) return;
        setInfo(i);
        setPhase('done');
      },
      (e: unknown) => {
        if (gone || isAbort(e)) return;
        setPhase('failed');
        setError(e);
      },
    );
    return () => {
      gone = true;
      ctl.abort();
    };
  }, [client, handle]);
  return { phase, info, error };
}

export interface UseFileDownload {
  readonly phase: TransferPhase;
  readonly progress: FileProgress | null;
  readonly error: unknown;
  /** Read the file and hand the browser it to save under `name`. A failure is put in `error` and thrown again. */
  download(handle: string, name: string): Promise<void>;
}

export function useFileDownload(): UseFileDownload {
  const client = useOrdo();
  const [phase, setPhase] = useState<TransferPhase>('idle');
  const [progress, setProgress] = useState<FileProgress | null>(null);
  const [error, setError] = useState<unknown>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const download = useCallback(
    async (handle: string, name: string) => {
      setPhase('working');
      setError(null);
      setProgress(null);
      try {
        await downloadFile(client, handle, name, {
          onProgress: (p) => {
            if (alive.current) setProgress(p);
          },
        });
        if (alive.current) setPhase('done');
      } catch (e) {
        if (alive.current) {
          setPhase('failed');
          setError(e);
        }
        throw e;
      }
    },
    [client],
  );
  return { phase, progress, error, download };
}
