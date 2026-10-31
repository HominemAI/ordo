/**
 * Files in React: upload with progress, show, info and download. A mock
 * door; the hooks run in a test renderer.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { createElement, type ReactNode } from 'react';
import { OrdoProvider } from '../src/hooks/context.js';
import { OrdoRefusal } from '../src/refusal.js';
import { DescriptorClient } from '../src/client.js';
import { MockDoor, iface } from './harness.js';
import {
  useFile,
  useFileDownload,
  useFileInfo,
  useFileUpload,
  type UseFile,
  type UseFileDownload,
  type UseFileInfo,
  type UseFileUpload,
} from '../src/index.js';
import { uploadFile, type XhrLike } from '../src/files.js';

let renderer: ReactTestRenderer | null = null;
afterEach(() => {
  if (renderer) void act(() => renderer?.unmount());
  renderer = null;
  vi.unstubAllGlobals();
});

async function render(client: DescriptorClient, probe: () => ReactNode): Promise<void> {
  await act(async () => {
    renderer = create(createElement(OrdoProvider, { client }, createElement(probe)));
  });
}

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
}

function client(fetch: typeof globalThis.fetch): DescriptorClient {
  return new DescriptorClient({ baseUrl: 'http://door.test', iface: iface(), space: 'spc_test', token: 'tok-abc', fetch });
}

const png = () => new Response(new Uint8Array([1, 2, 3]), { headers: { 'Content-Type': 'image/png', 'Content-Length': '3' } });

describe('uploadFile', () => {
  it('PUTs to the row with the space and the bearer token', async () => {
    const d = new MockDoor().always({ status: 200 });
    await uploadFile(client(d.fetch), { entity: 'attachments', field: 'file', id: 'a1' }, new Blob(['hi'], { type: 'text/plain' }));
    expect(d.last.url).toBe('http://door.test/blob/attachments/file/a1');
    expect(d.last.method).toBe('PUT');
    expect(d.last.headers['X-Ordo-Space']).toBe('spc_test');
    expect(d.last.headers['Authorization']).toBe('Bearer tok-abc');
    expect(d.last.headers['Content-Type']).toBe('text/plain');
  });

  it('throws the door refusal as an OrdoRefusal', async () => {
    const d = new MockDoor().always({ status: 404, body: { error: { code: 'not_found', message: 'no' } } });
    const err = await uploadFile(client(d.fetch), { entity: 'e', field: 'f', id: 'i' }, new Blob(['x'])).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoRefusal);
    expect((err as OrdoRefusal).refusal.class).toBe('not_found');
  });

  it('reports XMLHttpRequest progress', async () => {
    const xhr: XhrLike = {
      open() {},
      setRequestHeader() {},
      send() {
        queueMicrotask(() => {
          xhr.upload.onprogress?.({ loaded: 1, total: 4, lengthComputable: true });
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
    const marks: (number | null)[] = [];
    const d = new MockDoor();
    await uploadFile(client(d.fetch), { entity: 'e', field: 'f', id: 'i' }, new Blob(['abcd']), {
      onProgress: (p) => marks.push(p.fraction),
      xhr: () => xhr,
    });
    expect(marks).toEqual([0.25, 1]);
  });
});

describe('useFileUpload', () => {
  it('goes working to done and keeps the last progress', async () => {
    const d = new MockDoor().always({ status: 200 });
    let last!: UseFileUpload;
    await render(client(d.fetch), () => {
      last = useFileUpload();
      return null;
    });
    expect(last.phase).toBe('idle');
    await act(async () => {
      await last.upload({ entity: 'e', field: 'f', id: 'i' }, new Blob(['hello']));
    });
    expect(last.phase).toBe('done');
    expect(last.progress).toEqual({ loaded: 5, total: 5, fraction: 1 });
  });

  it('puts a refusal in error and throws it again', async () => {
    const d = new MockDoor().always({ status: 403, body: { error: { code: 'access_guard', message: 'no' } } });
    let last!: UseFileUpload;
    await render(client(d.fetch), () => {
      last = useFileUpload();
      return null;
    });
    let thrown: unknown;
    await act(async () => {
      thrown = await last.upload({ entity: 'e', field: 'f', id: 'i' }, new Blob(['x'])).catch((e: unknown) => e);
    });
    expect(thrown).toBeInstanceOf(OrdoRefusal);
    expect(last.phase).toBe('failed');
    expect(last.error).toBe(thrown);
  });
});

describe('useFile', () => {
  it('shows a file from an object URL and releases it on unmount', async () => {
    const revoked: string[] = [];
    vi.stubGlobal('URL', { createObjectURL: () => 'blob:img', revokeObjectURL: (u: string) => revoked.push(u) });
    let last!: UseFile;
    await render(client(async () => png()), () => {
      last = useFile('ab12');
      return null;
    });
    await settle();
    expect(last.phase).toBe('done');
    expect(last.url).toBe('blob:img');
    expect(last).toMatchObject({ type: 'image/png', size: 3 });
    await act(async () => renderer?.unmount());
    renderer = null;
    expect(revoked).toEqual(['blob:img']);
  });

  it('stays idle with no handle', async () => {
    let calls = 0;
    let last!: UseFile;
    await render(client(async () => { calls++; return png(); }), () => {
      last = useFile(null);
      return null;
    });
    await settle();
    expect(last.phase).toBe('idle');
    expect(calls).toBe(0);
  });

  it('fails with the refusal', async () => {
    let last!: UseFile;
    const d = new MockDoor().always({ status: 404, body: { error: { code: 'not_found', message: 'no' } } });
    await render(client(d.fetch), () => {
      last = useFile('ab12');
      return null;
    });
    await settle();
    expect(last.phase).toBe('failed');
    expect(last.error).toBeInstanceOf(OrdoRefusal);
  });
});

describe('useFileInfo', () => {
  it('reads type and size from /meta and no bytes', async () => {
    const d = new MockDoor().always({ body: { mime: 'application/pdf', size: 900 } });
    let last!: UseFileInfo;
    await render(client(d.fetch), () => {
      last = useFileInfo('ab12');
      return null;
    });
    await settle();
    expect(last.info).toEqual({ type: 'application/pdf', size: 900 });
    expect(d.last.url).toBe('http://door.test/blob/ab12/meta');
  });
});

describe('useFileDownload', () => {
  it('reads the file and clicks a link named for it', async () => {
    const clicked: string[] = [];
    const a = { href: '', download: '', rel: '', click() { clicked.push(`${this.href}|${this.download}`); }, remove() {} };
    vi.stubGlobal('document', { createElement: () => a, body: { append() {} } });
    vi.stubGlobal('URL', { createObjectURL: () => 'blob:dl', revokeObjectURL() {} });
    let last!: UseFileDownload;
    await render(client(async () => png()), () => {
      last = useFileDownload();
      return null;
    });
    await act(async () => {
      await last.download('ab12', 'photo.png');
    });
    expect(clicked).toEqual(['blob:dl|photo.png']);
    expect(last.phase).toBe('done');
  });
});
