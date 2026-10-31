/**
 * Files: upload, show and download. A mock door; no server, no DOM.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  MAX_FILE_BYTES,
  OrdoClient,
  OrdoLocalRefusal,
  OrdoNetworkError,
  OrdoRefusal,
  downloadFile,
  fileInfo,
  fileUrl,
  readFile,
  uploadFile,
  type FileProgress,
  type XhrLike,
} from '../src/index.js';
import { resetNetworkProbes } from '../src/core/network.js';

interface Seen {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: unknown;
}

function door(reply: (seen: Seen) => Response | Promise<Response>) {
  const seen: Seen[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const s: Seen = {
      url: String(input),
      method: init?.method ?? 'GET',
      headers: Object.fromEntries(Object.entries((init?.headers ?? {}) as Record<string, string>)),
      body: init?.body ?? null,
    };
    seen.push(s);
    return reply(s);
  };
  const client = new OrdoClient({ baseUrl: 'https://door.example/api/', space: 'spc_a', token: () => 'tok', fetch });
  return { seen, client };
}

const refusal = (status: number, code: string) =>
  new Response(JSON.stringify({ error: { code, message: `no: ${code}` } }), { status });

afterEach(() => {
  resetNetworkProbes();
  vi.unstubAllGlobals();
});

describe('uploadFile', () => {
  it('PUTs the bytes to the row with the space, the token and the file type', async () => {
    const { seen, client } = door(() => new Response(null, { status: 204 }));
    const file = new Blob(['hello'], { type: 'text/plain' });
    await uploadFile(client, { entity: 'attachments', field: 'file', id: 'att 1' }, file);
    expect(seen).toHaveLength(1);
    expect(seen[0]!.url).toBe('https://door.example/api/blob/attachments/file/att%201');
    expect(seen[0]!.method).toBe('PUT');
    expect(seen[0]!.headers['X-Ordo-Space']).toBe('spc_a');
    expect(seen[0]!.headers['Authorization']).toBe('Bearer tok');
    expect(seen[0]!.headers['Content-Type']).toBe('text/plain');
    expect(seen[0]!.body).toBe(file);
  });

  it('sends application/octet-stream for a file with no type', async () => {
    const { seen, client } = door(() => new Response(null, { status: 204 }));
    await uploadFile(client, { entity: 'e', field: 'f', id: 'i' }, new Blob(['x']));
    expect(seen[0]!.headers['Content-Type']).toBe('application/octet-stream');
  });

  it('refuses a file over the cap before sending anything', async () => {
    const { seen, client } = door(() => new Response(null, { status: 204 }));
    const big = { size: MAX_FILE_BYTES + 1, type: '' } as unknown as Blob;
    const err = await uploadFile(client, { entity: 'e', field: 'f', id: 'i' }, big).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoLocalRefusal);
    expect((err as OrdoLocalRefusal).class).toBe('bound_violation');
    expect(seen).toHaveLength(0);
  });

  it('honours a lower maxBytes', async () => {
    const { client } = door(() => new Response(null, { status: 204 }));
    const err = await uploadFile(client, { entity: 'e', field: 'f', id: 'i' }, new Blob(['12345']), { maxBytes: 4 }).catch(
      (e: unknown) => e,
    );
    expect(err).toBeInstanceOf(OrdoLocalRefusal);
  });

  it("throws the door's refusal as an OrdoRefusal", async () => {
    const { client } = door(() => refusal(404, 'not_found'));
    const err = await uploadFile(client, { entity: 'e', field: 'f', id: 'i' }, new Blob(['x'])).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoRefusal);
    expect((err as OrdoRefusal).class).toBe('not_found');
    expect((err as OrdoRefusal).detail.status).toBe(404);
  });

  it('names a call that got no answer', async () => {
    const fetch: typeof globalThis.fetch = async () => {
      throw new TypeError('Failed to fetch');
    };
    const client = new OrdoClient({ baseUrl: 'https://door.example', space: 's', token: 't', fetch });
    const err = await uploadFile(client, { entity: 'e', field: 'f', id: 'i' }, new Blob(['x'])).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect((err as OrdoNetworkError).kind).toBe('unreachable');
  });

  it('reports progress through XMLHttpRequest when asked', async () => {
    const { client } = door(() => new Response(null, { status: 204 }));
    const sent: { method: string; url: string; headers: Record<string, string> } = { method: '', url: '', headers: {} };
    const xhr: XhrLike = {
      open(method, url) {
        sent.method = method;
        sent.url = url;
      },
      setRequestHeader(k, v) {
        sent.headers[k] = v;
      },
      send() {
        queueMicrotask(() => {
          xhr.upload.onprogress?.({ loaded: 2, total: 5, lengthComputable: true });
          xhr.upload.onprogress?.({ loaded: 5, total: 5, lengthComputable: true });
          (xhr as { status: number }).status = 204;
          xhr.onload?.();
        });
      },
      abort() {},
      upload: { onprogress: null },
      onload: null,
      onerror: null,
      onabort: null,
      status: 0,
      responseText: '',
    };
    const marks: FileProgress[] = [];
    await uploadFile(client, { entity: 'e', field: 'f', id: 'i' }, new Blob(['hello'], { type: 'text/plain' }), {
      onProgress: (p) => marks.push(p),
      xhr: () => xhr,
    });
    expect(sent.method).toBe('PUT');
    expect(sent.url).toBe('https://door.example/api/blob/e/f/i');
    expect(sent.headers['Authorization']).toBe('Bearer tok');
    expect(sent.headers['X-Ordo-Space']).toBe('spc_a');
    expect(marks.map((m) => m.fraction)).toEqual([0.4, 1, 1]);
  });

  it('turns an XMLHttpRequest refusal into an OrdoRefusal', async () => {
    const { client } = door(() => new Response(null, { status: 204 }));
    const xhr: XhrLike = {
      open() {},
      setRequestHeader() {},
      send() {
        queueMicrotask(() => {
          (xhr as { status: number }).status = 403;
          (xhr as { responseText: string }).responseText = JSON.stringify({ error: { code: 'access_guard', message: 'no' } });
          xhr.onload?.();
        });
      },
      abort() {},
      upload: { onprogress: null },
      onload: null,
      onerror: null,
      onabort: null,
      status: 0,
      responseText: '',
    };
    const err = await uploadFile(client, { entity: 'e', field: 'f', id: 'i' }, new Blob(['x']), {
      onProgress: () => {},
      xhr: () => xhr,
    }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoRefusal);
    expect((err as OrdoRefusal).class).toBe('access_guard');
  });

  it('aborts an XMLHttpRequest upload with the caller signal', async () => {
    const { client } = door(() => new Response(null, { status: 204 }));
    const xhr: XhrLike = {
      open() {},
      setRequestHeader() {},
      send() {},
      abort() {
        xhr.onabort?.();
      },
      upload: { onprogress: null },
      onload: null,
      onerror: null,
      onabort: null,
      status: 0,
      responseText: '',
    };
    const ctl = new AbortController();
    const p = uploadFile(client, { entity: 'e', field: 'f', id: 'i' }, new Blob(['x']), {
      onProgress: () => {},
      xhr: () => xhr,
      signal: ctl.signal,
    }).catch((e: unknown) => e);
    await new Promise((r) => setTimeout(r, 0));
    ctl.abort();
    const err = (await p) as Error;
    expect(err.name).toBe('AbortError');
  });
});

describe('fileInfo', () => {
  it('reads the type and size from /meta', async () => {
    const { seen, client } = door(
      () => new Response(JSON.stringify({ mime: 'image/png', size: 42 }), { headers: { 'Content-Type': 'application/json' } }),
    );
    expect(await fileInfo(client, 'ab12')).toEqual({ type: 'image/png', size: 42 });
    expect(seen[0]!.url).toBe('https://door.example/api/blob/ab12/meta');
    expect(seen[0]!.headers['X-Ordo-Space']).toBe('spc_a');
  });

  it('is not_found for a handle the caller may not read', async () => {
    const { client } = door(() => refusal(404, 'not_found'));
    const err = await fileInfo(client, 'ab12').catch((e: unknown) => e);
    expect((err as OrdoRefusal).class).toBe('not_found');
  });
});

describe('readFile, fileUrl, downloadFile', () => {
  const bytes = () =>
    new Response(new Uint8Array([1, 2, 3, 4]), { headers: { 'Content-Type': 'image/png', 'Content-Length': '4' } });

  it('reads a Blob of the served type', async () => {
    const { seen, client } = door(bytes);
    const blob = await readFile(client, 'ab12');
    expect(blob.type).toBe('image/png');
    expect(blob.size).toBe(4);
    expect(seen[0]!.url).toBe('https://door.example/api/blob/ab12');
  });

  it('reports download progress with the total from Content-Length', async () => {
    const { client } = door(bytes);
    const marks: FileProgress[] = [];
    await readFile(client, 'ab12', { onProgress: (p) => marks.push(p) });
    expect(marks[0]).toEqual({ loaded: 0, total: 4, fraction: 0 });
    expect(marks.at(-1)).toEqual({ loaded: 4, total: 4, fraction: 1 });
  });

  it('shows a file from an object URL and lets it go once', async () => {
    const made: Blob[] = [];
    const revoked: string[] = [];
    vi.stubGlobal('URL', {
      createObjectURL: (b: Blob) => {
        made.push(b);
        return 'blob:one';
      },
      revokeObjectURL: (u: string) => revoked.push(u),
    });
    const { client } = door(bytes);
    const shown = await fileUrl(client, 'ab12');
    expect(shown.url).toBe('blob:one');
    expect(shown).toMatchObject({ type: 'image/png', size: 4 });
    shown.release();
    shown.release();
    expect(revoked).toEqual(['blob:one']);
  });

  it('downloads under a name by clicking a link', async () => {
    const clicked: { href: string; download: string }[] = [];
    const a = {
      href: '',
      download: '',
      rel: '',
      click() {
        clicked.push({ href: this.href, download: this.download });
      },
      remove() {},
    };
    vi.stubGlobal('document', { createElement: () => a, body: { append() {} } });
    vi.stubGlobal('URL', { createObjectURL: () => 'blob:two', revokeObjectURL() {} });
    vi.useFakeTimers();
    try {
      const { client } = door(bytes);
      await downloadFile(client, 'ab12', 'photo.png');
      expect(clicked).toEqual([{ href: 'blob:two', download: 'photo.png' }]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('says so when there is no page to save into', async () => {
    const { client } = door(bytes);
    await expect(downloadFile(client, 'ab12', 'x')).rejects.toThrow(/needs a page/);
  });

  it('sends no Authorization for a public file', async () => {
    const seen: Seen[] = [];
    const fetch: typeof globalThis.fetch = async (input, init) => {
      seen.push({ url: String(input), method: 'GET', headers: init?.headers as Record<string, string>, body: null });
      return bytes();
    };
    const client = new OrdoClient({ baseUrl: 'https://door.example', space: 'spc_a', fetch });
    await readFile(client, 'ab12');
    expect(seen[0]!.headers['Authorization']).toBeUndefined();
  });
});
