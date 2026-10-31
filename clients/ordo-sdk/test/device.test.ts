/**
 * `openDevice` (`docs/plan-sdk-device.md` §5): the ready-made device.
 *
 * The door keeps each feed open, so a test pushes frames when it wants them.
 */

import { LocalReplica, MemoryBackend, OrdoClient, openDevice } from '../src/index.js';
import type { OpSpec, OrdoDevice } from '../src/index.js';

class Door {
  readonly urls: string[] = [];
  readonly feeds: { push(text: string): void; ended: () => boolean }[] = [];

  readonly fetch: typeof globalThis.fetch = async (input, init) => {
    this.urls.push(String(input));
    const enc = new TextEncoder();
    let controller!: ReadableStreamDefaultController<Uint8Array>;
    let done = false;
    const stream = new ReadableStream<Uint8Array>({ start: (c) => void (controller = c) });
    init?.signal?.addEventListener('abort', () => {
      if (done) return;
      done = true;
      controller.error(init.signal!.reason);
    });
    this.feeds.push({ push: (t) => !done && controller.enqueue(enc.encode(t)), ended: () => done });
    return new Response(stream, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
  };

  client(): OrdoClient {
    return new OrdoClient({ baseUrl: 'https://door.example/api', space: 'spc_test', token: 'tok', fetch: this.fetch });
  }

  async nth(n: number) {
    for (let i = 0; i < 400 && this.feeds.length <= n; i++) await new Promise((r) => setTimeout(r, 2));
    return this.feeds[n]!;
  }
}

const load = (rows: unknown[], position: number) =>
  `event: load\ndata: ${JSON.stringify({ entity: 'tasks', key: 'id', position, pin: null, rows, loaded: rows.length, complete: true })}\n\n`;

const tasks: OpSpec = {
  op: 'std.tasks.list',
  version: '0123456789abcdef',
  method: 'GET',
  kind: 'read',
  terminal: ['fetch', 'live', 'pages'],
  paged: true,
  singleRow: false,
  casParam: null,
  params: [],
  refusals: [],
  standingError: null,
  sync: { entity: 'tasks', key: 'id' },
  refine: { tier: 'none', fields: ['id', 'title'] },
  order: { keys: [{ path: 'id', desc: false }], stated: true },
};

async function loaded(device: OrdoDevice, door: Door): Promise<void> {
  device.manager.sync(tasks);
  (await door.nth(0)).push(load([{ id: 'task_1', title: 'one' }], 3));
  await device.manager.settled();
}

describe('openDevice', () => {
  it('opens the replica for one person in one space, apart from another person', async () => {
    const backend = new MemoryBackend();
    const door = new Door();
    const ada = await openDevice({ client: door.client(), space: 'spc_test', principal: 'ada', backend, manager: { online: null } });
    await loaded(ada, door);
    expect(ada.space).toBe('spc_test');
    expect(ada.principal).toBe('ada');
    expect(await ada.replica.scan('tasks')).toHaveLength(1);
    // Bo on the same backend sees none of Ada's rows.
    const bo = await LocalReplica.open(backend, 'spc_test', 'bo');
    expect(await bo.scan('tasks')).toEqual([]);
    await ada.close();
  });

  it('close() stops syncing and keeps the rows', async () => {
    const backend = new MemoryBackend();
    const door = new Door();
    const device = await openDevice({ client: door.client(), space: 'spc_test', principal: 'ada', backend, manager: { online: null } });
    await loaded(device, door);
    await device.close();
    expect((await door.nth(0)).ended()).toBe(true);
    const again = await LocalReplica.open(backend, 'spc_test', 'ada');
    expect(await again.scan('tasks')).toHaveLength(1);
  });

  it('purge() stops every feed, then erases what was held', async () => {
    const backend = new MemoryBackend();
    const door = new Door();
    const device = await openDevice({ client: door.client(), space: 'spc_test', principal: 'ada', backend, manager: { online: null } });
    await loaded(device, door);
    await device.purge();
    expect((await door.nth(0)).ended()).toBe(true);
    expect(await backend.keys('')).toEqual([]);
  });

  it('an online event nudges every feed; there is no focus reload and no timer', async () => {
    const door = new Door();
    const online = new EventTarget();
    const device = await openDevice({
      client: door.client(),
      space: 'spc_test',
      principal: 'ada',
      backend: new MemoryBackend(),
      manager: { online },
    });
    await loaded(device, door);
    globalThis.dispatchEvent?.(new Event('focus'));
    await new Promise((r) => setTimeout(r, 20));
    expect(door.urls).toHaveLength(1);
    online.dispatchEvent(new Event('online'));
    await door.nth(1);
    expect(door.urls).toHaveLength(2);
    await device.close();
  });

  it('reconnect() reopens the feed on the same manager', async () => {
    const door = new Door();
    const device = await openDevice({
      client: door.client(),
      space: 'spc_test',
      principal: 'ada',
      backend: new MemoryBackend(),
      manager: { online: null },
    });
    await loaded(device, door);
    const manager = device.manager;
    const done = device.reconnect();
    (await door.nth(1)).push(load([{ id: 'task_1', title: 'one' }], 4));
    await done;
    expect(device.manager).toBe(manager);
    expect((await door.nth(0)).ended()).toBe(true);
    await device.close();
  });

  it('with no backend it opens IndexedDB, and says so where there is none', async () => {
    const door = new Door();
    await expect(openDevice({ client: door.client(), space: 's', principal: 'p' })).rejects.toThrow(/IndexedDB/);
  });
});
