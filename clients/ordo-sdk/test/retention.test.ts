/**
 * Forgetting long-unused rows (`docs/plan-sdk-device.md` §6b). The replica
 * keeps a sealed last-used time per holder. When a device opens, holders
 * unused past the retention period (7 days by default) are let go as if
 * dropped, under the replica lock.
 */

import { IDBFactory } from 'fake-indexeddb';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  DEFAULT_ERASE_AFTER_MS,
  IndexedDbBackend,
  LocalReplica,
  MemoryBackend,
  OrdoClient,
  SyncManager,
  openDevice,
} from '../src/index.js';

const DAY = 24 * 60 * 60 * 1000;
const row = (id: string) => ({ entity: 'notes', id, row: { id } });

afterEach(() => vi.unstubAllGlobals());

describe('eraseUnused', () => {
  it('lets go of a holder unused past the period and keeps one used today', async () => {
    const r = await LocalReplica.open(new MemoryBackend(), 'spc', 'ada');
    const now = Date.now();
    await r.applyFrame('old', { position: 1, upserts: [row('n1'), row('shared')], result: { version: 'v', hash: 'h-old' } });
    await r.applyFrame('today', { position: 1, upserts: [row('n2'), row('shared')] });
    await r.touch('old', now - 8 * DAY);
    await r.touch('today', now);

    expect(await r.eraseUnused(now - DEFAULT_ERASE_AFTER_MS)).toEqual(['old']);
    expect(await r.holdings('old')).toEqual([]);
    expect(await r.get('notes', 'n1')).toBeUndefined();
    // A row another holder keeps stays.
    expect((await r.get('notes', 'shared'))?.row).toEqual({ id: 'shared' });
    expect(await r.holdings('today')).toHaveLength(2);
    expect(await r.resultHash('old', 'v')).toBeUndefined();
    expect(await r.lastUsed('old')).toBeUndefined();
    expect(await r.lastUsed('today')).toBe(now);
  });

  it('stamps a holder with no last-used time as now, keeps it before 7 days, erases it after', async () => {
    const backend = new MemoryBackend();
    const r = await LocalReplica.open(backend, 'spc', 'ada');
    await r.applyFrame('before-6b', { position: 1, upserts: [row('n1')] });
    expect(await r.lastUsed('before-6b')).toBeUndefined();

    const t0 = Date.now();
    expect(await r.eraseUnused(t0 - DEFAULT_ERASE_AFTER_MS)).toEqual([]);
    const stamp = await r.lastUsed('before-6b');
    expect(stamp).toBeGreaterThanOrEqual(t0);
    expect(await r.holdings('before-6b')).toHaveLength(1);

    // A later sweep does not stamp again: the marker says the scan ran.
    await r.applyFrame('later', { position: 1, upserts: [row('n2')] });
    await r.eraseUnused(t0 - DEFAULT_ERASE_AFTER_MS);
    expect(await r.lastUsed('later')).toBeUndefined();

    // Eight days on, the stamp is older than the period.
    expect(await r.eraseUnused(stamp! + 1)).toEqual(['before-6b']);
    expect(await r.holdings('before-6b')).toEqual([]);
    expect(await r.get('notes', 'n1')).toBeUndefined();
  });

  it('runs under the replica lock, so a touch from another tab waits and is seen', async () => {
    const tails = new Map<string, Promise<unknown>>();
    const names: string[] = [];
    const request = <T>(name: string, callback: () => Promise<T>): Promise<T> => {
      names.push(name);
      const run = (tails.get(name) ?? Promise.resolve()).then(callback, callback);
      tails.set(name, run.catch(() => undefined));
      return run;
    };
    vi.stubGlobal('navigator', { locks: { request } });
    const factory = new IDBFactory();
    const a = await LocalReplica.open(await IndexedDbBackend.open('ret', factory), 'spc', 'ada');
    const b = await LocalReplica.open(await IndexedDbBackend.open('ret', factory), 'spc', 'ada');
    await a.applyFrame('list', { position: 1, upserts: [row('n1')] });
    await a.touch('list', Date.now() - 8 * DAY);

    // Tab B notes the list in use while tab A sweeps: one goes first, whole.
    const [erased] = await Promise.all([a.eraseUnused(Date.now() - 7 * DAY), b.touch('list')]);
    expect(new Set(names)).toEqual(new Set([names[0]]));
    if (erased.length === 0) expect(await a.holdings('list')).toHaveLength(1);
    else expect(await b.lastUsed('list')).toBeGreaterThan(Date.now() - DAY);
  });
});

describe('the manager notes what it syncs', () => {
  it('on sync and on idle', async () => {
    const r = await LocalReplica.open(new MemoryBackend(), 'spc', 'ada');
    const never: typeof fetch = () => new Promise<Response>(() => undefined);
    const client = new OrdoClient({ baseUrl: 'https://door.example', space: 'spc', token: 't', fetch: never });
    const manager = SyncManager.start(client, r, [], { online: null, touchEveryMs: Infinity });
    const spec = {
      op: 'notes',
      version: 'v',
      method: 'GET',
      kind: 'read',
      terminal: ['fetch'],
      paged: false,
      singleRow: false,
      casParam: null,
      params: [],
      refusals: [],
      standingError: null,
      sync: { entity: 'notes', key: 'id' },
      refine: { tier: 'none' as const, fields: ['id'] },
      order: null,
    };
    const key = manager.sync(spec);
    for (let i = 0; i < 50 && (await r.lastUsed(key)) === undefined; i++) await new Promise((x) => setTimeout(x, 2));
    const first = await r.lastUsed(key);
    expect(first).toBeDefined();
    await new Promise((x) => setTimeout(x, 5));
    await manager.idle(key);
    for (let i = 0; i < 50 && (await r.lastUsed(key)) === first; i++) await new Promise((x) => setTimeout(x, 2));
    expect(await r.lastUsed(key)).toBeGreaterThan(first!);
    await manager.stop();
  });
});

describe('openDevice erases on open', () => {
  const client = new OrdoClient({ baseUrl: 'https://door.example', space: 'spc', token: 't', fetch: () => new Promise(() => undefined) });

  it('rows unused for 7 days go; eraseAfterMs is a setting', async () => {
    const backend = new MemoryBackend();
    const r = await LocalReplica.open(backend, 'spc', 'ada');
    await r.applyFrame('old', { position: 1, upserts: [row('n1')] });
    await r.applyFrame('recent', { position: 1, upserts: [row('n2')] });
    await r.touch('old', Date.now() - 8 * DAY);
    await r.touch('recent', Date.now() - 2 * DAY);

    // Kept with a longer setting.
    const kept = await openDevice({ client, space: 'spc', principal: 'ada', backend, eraseAfterMs: 30 * DAY, manager: { online: null } });
    expect(await kept.replica.holdings('old')).toHaveLength(1);
    await kept.close();
    // Infinity keeps everything.
    const forever = await openDevice({ client, space: 'spc', principal: 'ada', backend, eraseAfterMs: Infinity, manager: { online: null } });
    expect(await forever.replica.holdings('old')).toHaveLength(1);
    await forever.close();
    // The default, 7 days.
    const device = await openDevice({ client, space: 'spc', principal: 'ada', backend, manager: { online: null } });
    expect(await device.replica.holdings('old')).toEqual([]);
    expect(await device.replica.holdings('recent')).toHaveLength(1);
    await device.close();
  });
});
