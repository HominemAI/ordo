/**
 * A person's own download: one call, `GET /download`, with the token and the
 * space, answered as the zip and the name the door gave it.
 */

import { describe, expect, it } from 'vitest';

import { OrdoClient, OrdoLocalRefusal, OrdoRefusal, downloadName } from '../src/index.js';

interface Seen {
  url: string;
  headers: Record<string, string>;
}

function door(response: () => Response): { fetch: typeof fetch; seen: Seen[] } {
  const seen: Seen[] = [];
  const f = (async (url: string, init?: RequestInit) => {
    seen.push({ url, headers: { ...(init?.headers as Record<string, string>) } });
    return response();
  }) as unknown as typeof fetch;
  return { fetch: f, seen };
}

const client = (f: typeof fetch, token?: string) =>
  new OrdoClient({ baseUrl: 'https://door.example/api/', space: 'spc_test', token, fetch: f });

describe('downloadOwn', () => {
  it('sends the token and the space to /download and answers the zip and its name', async () => {
    const zip = new Uint8Array([0x50, 0x4b, 0x05, 0x06]);
    const d = door(
      () =>
        new Response(zip, {
          status: 200,
          headers: {
            'Content-Type': 'application/zip',
            'Content-Disposition': 'attachment; filename="download-2026-10-22.zip"',
          },
        }),
    );
    const got = await client(d.fetch, 'tok').downloadOwn();
    expect(d.seen[0]!.url).toBe('https://door.example/api/download');
    expect(d.seen[0]!.headers['Authorization']).toBe('Bearer tok');
    expect(d.seen[0]!.headers['X-Ordo-Space']).toBe('spc_test');
    expect(got.name).toBe('download-2026-10-22.zip');
    expect(new Uint8Array(await got.blob.arrayBuffer())).toEqual(zip);
  });

  it('throws the door refusal, which says how the app developer turns it on', async () => {
    const message = "a person's own download is off. The app developer turns it on by marking read ops `download`";
    const d = door(
      () =>
        new Response(JSON.stringify({ error: { code: 'access_denied', message, op: 'download' } }), {
          status: 403,
          headers: { 'Content-Type': 'application/json' },
        }),
    );
    const err = await client(d.fetch, 'tok').downloadOwn().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoRefusal);
    expect((err as OrdoRefusal).class).toBe('access_denied');
    expect((err as OrdoRefusal).message).toContain('`download`');
  });

  it('refuses locally with no token, sending nothing', async () => {
    const d = door(() => new Response(null, { status: 500 }));
    const err = await client(d.fetch).downloadOwn().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoLocalRefusal);
    expect(d.seen).toHaveLength(0);
  });

  it('reads the file name, with a plain fallback', () => {
    expect(downloadName('attachment; filename="download-2026-10-22.zip"')).toBe('download-2026-10-22.zip');
    expect(downloadName(null)).toBe('download.zip');
    expect(downloadName('attachment; filename="../x"')).toBe('download.zip');
  });
});
