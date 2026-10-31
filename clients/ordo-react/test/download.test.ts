/**
 * A person's own download (owner ruling 2026-10-22): the client's one call,
 * and the hook that holds its state.
 */

import { describe, expect, it } from 'vitest';
import { act, create } from 'react-test-renderer';
import { createElement, type ReactNode } from 'react';
import { OrdoProvider } from '../src/hooks/context.js';
import { useOwnDownload, type UseOwnDownload } from '../src/hooks/useOwnDownload.js';
import { OrdoRefusal } from '../src/refusal.js';
import { downloadName } from '../src/client.js';
import { MockDoor, client, refusal } from './harness.js';

describe('downloadOwn', () => {
  it('asks /download with the token and the space', async () => {
    const door = new MockDoor().program({
      status: 200,
      body: 'zip',
      headers: { 'Content-Disposition': 'attachment; filename="download-2026-10-22.zip"' },
    });
    const got = await client(door).downloadOwn();
    expect(door.last.url).toBe('http://door.test/download');
    expect(door.last.headers['Authorization']).toBe('Bearer tok-abc');
    expect(door.last.headers['X-Ordo-Space']).toBe('spc_test');
    expect(got.name).toBe('download-2026-10-22.zip');
  });

  it('throws the refusal that names how to turn it on', async () => {
    const door = new MockDoor().program({
      status: 403,
      body: refusal('access_denied', 'the app developer turns it on by marking read ops `download`'),
    });
    const err = await client(door).downloadOwn().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoRefusal);
    expect(String(err)).toContain('`download`');
  });

  it('reads the name from the header, or falls back', () => {
    expect(downloadName(null)).toBe('download.zip');
  });
});

describe('useOwnDownload', () => {
  it('holds busy and the error, and answers null on a refusal', async () => {
    const door = new MockDoor().program({ status: 403, body: refusal('access_denied', 'off') });
    let hook!: UseOwnDownload;
    function Probe(): ReactNode {
      hook = useOwnDownload();
      return null;
    }
    await act(async () => {
      create(createElement(OrdoProvider, { client: client(door) }, createElement(Probe)));
    });
    expect(hook.busy).toBe(false);
    let answer: unknown = 'unset';
    await act(async () => {
      answer = await hook.download();
    });
    expect(answer).toBeNull();
    expect(hook.error).toBeInstanceOf(OrdoRefusal);
    expect(hook.busy).toBe(false);
  });
});
