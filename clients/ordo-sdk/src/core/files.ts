/**
 * Files: upload, show and download, over the blob door.
 *
 * A file is a row and its bytes. An op writes the row (name, owner, a `blob`
 * field left empty). These helpers send the bytes to that row and read them
 * back, with the same space header and token as every op call:
 *
 * - `PUT /blob/{entity}/{field}/{id}` stores the bytes and sets the row's
 *   `blob` field to a handle. The door needs `Content-Length`, which a
 *   browser sets for a `Blob` body.
 * - `GET /blob/{handle}` reads the bytes.
 * - `GET /blob/{handle}/meta` answers the media type and size, with no bytes.
 *
 * The row guard decides who reads, on all three. A person who may not see
 * the row gets the same `not_found` for its bytes and its metadata.
 *
 * # Errors
 *
 * Nothing new to catch:
 *
 * - The door's refusal is an {@link OrdoRefusal}: `not_found`,
 *   `access_denied`, `access_guard`, `bound_violation` (over the object cap)
 *   and the rest of the closed set.
 * - A file over the cap is refused before a byte is sent, as an
 *   {@link OrdoLocalRefusal} with the same class, `bound_violation`.
 * - A call that got no answer is an `OrdoNetworkError` (SDK) with its kind.
 * - A caller's own abort is thrown as it came (`AbortError`).
 *
 * # No deadline
 *
 * An op call has a deadline, `callTimeoutMs`. A 100 MiB upload on a slow
 * line needs longer than any fixed one, so a file call has none. Pass a
 * `signal` to stop it.
 *
 * # One request helper
 *
 * Every call goes through `core/door.ts`, so a busy door (`server_busy`) is
 * sent again under the client's `busyRetry`, as an op call is. The one
 * exception is an upload with `onProgress`: only `XMLHttpRequest` reports
 * upload progress, so that one request is made here, once.
 * `@hominemai/ordo-react` re-exports these calls as they are.
 */

import { busyRetryOf, type BusyRetryOptions } from './busy.js';
import { busyOnly, doorRequest, doorUrl, refusalOf } from './door.js';
import { OrdoLocalRefusal, OrdoRefusal, decodeRefusal } from './refusal.js';

/** The door's limit on one object, 100 MiB (`ordo-blob` policy). */
export const MAX_FILE_BYTES = 100 * 1024 * 1024;

/** What the file calls need from a client. */
export interface FileDoor {
  readonly baseUrl: string;
  readonly space: string;
  /** The caller's token, or `undefined` for a public file. */
  readonly token: () => Promise<string | undefined>;
  readonly fetch: typeof globalThis.fetch;
  /** Turns a failure to get an answer into the client's own error. */
  readonly wrap?: (e: unknown) => Promise<unknown>;
  /** How a call is sent again on `server_busy`. Default the client default. */
  readonly busyRetry?: BusyRetryOptions | undefined;
}

/** What the file calls take: a client (it has `fileDoor()`), or a {@link FileDoor} built by hand. */
export type FileClient = FileDoor | { fileDoor(): FileDoor };

function doorOf(c: FileClient): FileDoor {
  return 'fileDoor' in c ? c.fileDoor() : c;
}

/** How far a transfer has got. */
export interface FileProgress {
  /** Bytes moved so far. */
  readonly loaded: number;
  /** Bytes in all, or `null` when the length is not known. */
  readonly total: number | null;
  /** `loaded / total` from 0 to 1, or `null` when the total is not known. */
  readonly fraction: number | null;
}

export type OnFileProgress = (progress: FileProgress) => void;

/** The part of `XMLHttpRequest` an upload uses. Injectable for tests. */
export interface XhrLike {
  open(method: string, url: string): void;
  setRequestHeader(name: string, value: string): void;
  send(body: Blob): void;
  abort(): void;
  readonly upload: { onprogress: ((e: { loaded: number; total: number; lengthComputable: boolean }) => void) | null };
  onload: (() => void) | null;
  onerror: (() => void) | null;
  onabort: (() => void) | null;
  readonly status: number;
  readonly responseText: string;
}

export interface FileCallOptions {
  readonly signal?: AbortSignal | undefined;
}

export interface UploadOptions extends FileCallOptions {
  /**
   * Called as bytes leave. Uses `XMLHttpRequest`, the only way a browser
   * reports upload progress. Where there is none (Node), it is called with
   * 0 and then 100% around a plain `fetch`.
   */
  readonly onProgress?: OnFileProgress | undefined;
  /** Largest file to send, in bytes. Default {@link MAX_FILE_BYTES}. A lower value only. */
  readonly maxBytes?: number | undefined;
  /** A request factory, for tests. Default `new XMLHttpRequest()`. */
  readonly xhr?: (() => XhrLike) | undefined;
}

export interface DownloadOptions extends FileCallOptions {
  /** Called as bytes arrive. `total` is known when the door sent `Content-Length`. */
  readonly onProgress?: OnFileProgress | undefined;
}

/** Where an upload goes: the row's entity, its `blob` field and its id. */
export interface FileTarget {
  readonly entity: string;
  readonly field: string;
  readonly id: string;
}

/** A file's media type and size, as the door has them. */
export interface FileInfo {
  /** The media type, `image/png`. */
  readonly type: string;
  /** The size in bytes. */
  readonly size: number;
}

/** A file shown from an object URL. Call {@link ShownFile.release} when done. */
export interface ShownFile extends FileInfo {
  /** An object URL, good for `<img src>`, `<a href>` and `<video src>`. */
  readonly url: string;
  /** Lets the object URL go. Safe to call twice. */
  release(): void;
}

function progress(loaded: number, total: number | null): FileProgress {
  return { loaded, total, fraction: total === null || total === 0 ? (total === 0 ? 1 : null) : Math.min(1, loaded / total) };
}

async function headers(door: FileDoor): Promise<Record<string, string>> {
  const out: Record<string, string> = { 'X-Ordo-Space': door.space };
  const token = await door.token();
  if (token) out['Authorization'] = `Bearer ${token}`;
  return out;
}

function url(door: FileDoor, path: string): string {
  return doorUrl(door.baseUrl, path);
}

async function failed(door: FileDoor, e: unknown): Promise<never> {
  throw (door.wrap ? await door.wrap(e) : e) ?? e;
}

/**
 * One file call through the request helper: no deadline, busy retried, and
 * a failure to get an answer named by the door's own `wrap`.
 */
async function send(door: FileDoor, path: string, init: RequestInit): Promise<Response> {
  try {
    return await doorRequest(
      { baseUrl: door.baseUrl, fetch: door.fetch, timeoutOption: 'signal' },
      url(door, path),
      init,
      { timeoutMs: null, named: false, retry: busyOnly(busyRetryOf(door.busyRetry)) },
    );
  } catch (e) {
    return failed(door, e);
  }
}

function tooLarge(size: number, max: number): OrdoLocalRefusal {
  return new OrdoLocalRefusal({
    class: 'bound_violation',
    message: `this file is ${size} bytes; the largest the door takes is ${max}`,
    op: 'std.$blob.update',
    param: null,
  });
}

/**
 * Send a file's bytes to the row made for it. The row must exist and the
 * caller's write guard must admit it. Resolves when the door has stored the
 * bytes and set the row's field.
 */
export async function uploadFile(
  client: FileClient,
  target: FileTarget,
  file: Blob,
  options: UploadOptions = {},
): Promise<void> {
  const door = doorOf(client);
  const max = Math.min(options.maxBytes ?? MAX_FILE_BYTES, MAX_FILE_BYTES);
  if (file.size > max) throw tooLarge(file.size, max);
  if (options.signal?.aborted) throw options.signal.reason ?? abortError();
  const path = `/blob/${encodeURIComponent(target.entity)}/${encodeURIComponent(target.field)}/${encodeURIComponent(target.id)}`;
  const type = file.type || 'application/octet-stream';
  const make = options.xhr ?? (typeof XMLHttpRequest === 'undefined' ? undefined : () => new XMLHttpRequest() as unknown as XhrLike);
  const sent = await headers(door);

  if (options.onProgress && make) {
    await new Promise<void>((resolve, reject) => {
      const req = make();
      const onProgress = options.onProgress!;
      const stop = () => req.abort();
      if (options.signal?.aborted) {
        reject(options.signal.reason ?? abortError());
        return;
      }
      options.signal?.addEventListener('abort', stop, { once: true });
      const done = () => options.signal?.removeEventListener('abort', stop);
      req.open('PUT', url(door, path));
      for (const [k, v] of Object.entries({ ...sent, 'Content-Type': type })) req.setRequestHeader(k, v);
      req.upload.onprogress = (e) => onProgress(progress(e.loaded, e.lengthComputable ? e.total : file.size));
      req.onload = () => {
        done();
        if (req.status >= 200 && req.status < 300) {
          onProgress(progress(file.size, file.size));
          resolve();
          return;
        }
        let payload: unknown = null;
        try {
          payload = req.responseText ? JSON.parse(req.responseText) : null;
        } catch {
          // Not JSON.
        }
        reject(new OrdoRefusal(decodeRefusal(payload, req.status, 'std.$blob.update', [])));
      };
      req.onerror = () => {
        done();
        failed(door, new TypeError('Failed to fetch')).catch(reject);
      };
      req.onabort = () => {
        done();
        reject(options.signal?.reason ?? abortError());
      };
      req.send(file);
    });
    return;
  }

  options.onProgress?.(progress(0, file.size));
  const res = await send(door, path, {
    method: 'PUT',
    headers: { ...sent, 'Content-Type': type },
    body: file,
    ...(options.signal ? { signal: options.signal } : {}),
  });
  if (!res.ok) throw await refusalOf(res, 'std.$blob.update');
  options.onProgress?.(progress(file.size, file.size));
}

/** A file's media type and size, without its bytes. The same row guard as the bytes. */
export async function fileInfo(client: FileClient, handle: string, options: FileCallOptions = {}): Promise<FileInfo> {
  const door = doorOf(client);
  const res = await send(door, `/blob/${encodeURIComponent(handle)}/meta`, {
    headers: { ...(await headers(door)), Accept: 'application/json' },
    ...(options.signal ? { signal: options.signal } : {}),
  });
  if (!res.ok) throw await refusalOf(res, 'std.$blob.get');
  const body = (await res.json()) as { mime?: unknown; size?: unknown };
  return {
    type: typeof body.mime === 'string' ? body.mime : 'application/octet-stream',
    size: typeof body.size === 'number' ? body.size : 0,
  };
}

/** A file's bytes, by the handle its row holds, as a `Blob` of the right type. */
export async function readFile(client: FileClient, handle: string, options: DownloadOptions = {}): Promise<Blob> {
  const door = doorOf(client);
  const res = await send(door, `/blob/${encodeURIComponent(handle)}`, {
    headers: await headers(door),
    ...(options.signal ? { signal: options.signal } : {}),
  });
  if (!res.ok) throw await refusalOf(res, 'std.$blob.get');
  const type = res.headers.get('content-type') ?? 'application/octet-stream';
  const length = Number(res.headers.get('content-length'));
  const total = Number.isFinite(length) && res.headers.has('content-length') ? length : null;
  if (!options.onProgress || !res.body) {
    const blob = await res.blob();
    options.onProgress?.(progress(blob.size, blob.size));
    return blob;
  }
  const reader = res.body.getReader();
  const parts: Uint8Array[] = [];
  let loaded = 0;
  options.onProgress(progress(0, total));
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      parts.push(value);
      loaded += value.length;
      options.onProgress(progress(loaded, total));
    }
  } catch (e) {
    return failed(door, e);
  }
  return new Blob(parts as BlobPart[], { type });
}

/**
 * A file as an object URL, for `<img src>`, `<video src>` or `<a href>`. A
 * page cannot put a token in an `<img src>`, so the bytes are read with the
 * token and the URL points at them in memory. Call `release()` when the
 * image goes away, or the browser keeps the bytes.
 */
export async function fileUrl(client: FileClient, handle: string, options: DownloadOptions = {}): Promise<ShownFile> {
  const door = doorOf(client);
  const blob = await readFile(door, handle, options);
  const made = URL.createObjectURL(blob);
  let live = true;
  return {
    url: made,
    type: blob.type,
    size: blob.size,
    release() {
      if (!live) return;
      live = false;
      URL.revokeObjectURL(made);
    },
  };
}

/**
 * Hand the browser a file to save, under `name`. Needs a page: it clicks a
 * link made for the purpose. Resolves when the click is made.
 */
export async function downloadFile(
  client: FileClient,
  handle: string,
  name: string,
  options: DownloadOptions = {},
): Promise<void> {
  const door = doorOf(client);
  const blob = await readFile(door, handle, options);
  saveBlob(blob, name);
}

/** Save a `Blob` under a name. A page only. */
export function saveBlob(data: Blob, name: string): void {
  if (typeof document === 'undefined') {
    throw new TypeError('saveBlob needs a page (no document here). Use readFile for the bytes.');
  }
  const href = URL.createObjectURL(data);
  const a = document.createElement('a');
  a.href = href;
  a.download = name;
  a.rel = 'noopener';
  document.body.append(a);
  a.click();
  a.remove();
  // The save has started from the URL; a minute is ample before it goes.
  setTimeout(() => URL.revokeObjectURL(href), 60_000);
}

function abortError(): Error {
  const e = new Error('aborted');
  e.name = 'AbortError';
  return e;
}
