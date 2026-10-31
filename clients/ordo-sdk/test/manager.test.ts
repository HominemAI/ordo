/**
 * C5 of `plan-client-cache.md`: the sync manager.
 *
 * The door here keeps each feed **open**, so a test pushes frames when it
 * wants them and closes a stream to make the manager reconnect. That is what
 * lets the reconnect rules be asserted rather than described.
 */

import { LocalReplica, MemoryBackend, OrdoClient, SyncManager } from '../src/index.js';
import type { Observation, ObservedRows, OpSpec, SyncView } from '../src/index.js';
import { CountingBackend } from './counting.js';

/** One open feed the test drives. */
class Feed {
  readonly stream: ReadableStream<Uint8Array>;
  #controller!: ReadableStreamDefaultController<Uint8Array>;
  #done = false;

  constructor(signal?: AbortSignal | null) {
    const enc = new TextEncoder();
    this.stream = new ReadableStream<Uint8Array>({
      start: (c) => {
        this.#controller = c;
      },
    });
    this.push = (text: string) => {
      if (!this.#done) this.#controller.enqueue(enc.encode(text));
    };
    // The manager aborts a feed to drop or stop it, and the body has to end
    // for the op's loop to finish.
    signal?.addEventListener('abort', () => {
      if (this.#done) return;
      this.#done = true;
      this.#controller.error(signal.reason);
    });
  }

  readonly push: (text: string) => void;

  /** True once the body ended: closed by the door, or aborted by the manager. */
  get ended(): boolean {
    return this.#done;
  }

  close(): void {
    if (this.#done) return;
    this.#done = true;
    this.#controller.close();
  }
}

class LiveDoor {
  readonly urls: string[] = [];
  /** The `X-Ordo-Resume` each call carried, `null` when none. */
  readonly resumes: (string | null)[] = [];
  /** The `cache` mode each call asked for. */
  readonly caches: (RequestCache | undefined)[] = [];
  /** While set, each call waits for it before it answers. */
  gate: Promise<void> | null = null;
  readonly feeds: Feed[] = [];
  /** For each call, how many feeds were still open when it was made. */
  readonly openAtCall: number[] = [];
  readonly waits: number[] = [];
  #refusals: ({ status: number; json: unknown; retryAfter?: string } | null)[] = [];
  #waiting: (() => void)[] = [];

  /** Refuse the next call; `null` opens a feed instead. */
  refuse(status: number, code: string, retryAfter?: string): this {
    // `code` is the door's own field (`api-engine-design.md` §4). It matters
    // here: a refusal that decodes as `internal` is one the manager retries,
    // so these tests only mean what they say if the class survives the wire.
    this.#refusals.push({
      status,
      json: { error: { code, message: `${code} at the door`, op: 'std.tasks.list' } },
      ...(retryAfter !== undefined ? { retryAfter } : {}),
    });
    return this;
  }

  /** Let the next call open a feed, after the refusals queued before it. */
  open(): this {
    this.#refusals.push(null);
    return this;
  }

  readonly fetch: typeof globalThis.fetch = async (input, init) => {
    this.urls.push(String(input));
    this.openAtCall.push(this.feeds.filter((f) => !f.ended).length);
    this.resumes.push((init?.headers as Record<string, string> | undefined)?.['X-Ordo-Resume'] ?? null);
    this.caches.push(init?.cache);
    if (this.gate) await this.gate;
    const refusal = this.#refusals.length > 0 ? this.#refusals.shift() : null;
    if (refusal) {
      const headers: Record<string, string> = { 'Content-Type': 'application/json' };
      if (refusal.retryAfter !== undefined) headers['Retry-After'] = refusal.retryAfter;
      return new Response(JSON.stringify(refusal.json), { status: refusal.status, headers });
    }
    const feed = new Feed(init?.signal);
    this.feeds.push(feed);
    for (const w of this.#waiting.splice(0)) w();
    return new Response(feed.stream, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
  };

  /** The nth feed, once the manager has opened it. */
  async nth(n: number): Promise<Feed> {
    while (this.feeds.length <= n) await new Promise<void>((r) => this.#waiting.push(r));
    return this.feeds[n]!;
  }

  /** What `renewToken` answers, in turn; none set means the client cannot renew. */
  renewals: boolean[] | null = null;
  /** How many times the client was asked to renew its token. */
  renewed = 0;

  client(): OrdoClient {
    const renewals = this.renewals;
    return new OrdoClient({
      baseUrl: 'https://door.example/api',
      space: 'spc_test',
      token: 'tok',
      fetch: this.fetch,
      ...(renewals
        ? {
            renewToken: async () => {
              this.renewed += 1;
              return renewals.shift() ?? false;
            },
          }
        : {}),
    });
  }
}

const frame = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
const load = (rows: unknown[], position: number, complete = true, result?: string) =>
  frame('load', { entity: 'tasks', key: 'id', position, pin: null, rows, loaded: rows.length, complete, result });
const change = (position: number, upserts: unknown[], deletes: string[], result?: string) =>
  frame('change', { entity: 'tasks', key: 'id', position, pin: null, upserts, deletes, result });
const task = (n: number, title = `task ${n}`) => ({ id: `task_${n}`, title });

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

const mine: OpSpec = { ...tasks, op: 'myTasks' };
const byOwner: OpSpec = {
  ...tasks,
  op: 'tasksByOwner',
  params: [{ name: 'owner', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null }],
};

const replica = () => LocalReplica.open(new MemoryBackend(), 'spc_test', 'usr_1');
const rows = async (r: LocalReplica) => (await r.scan('tasks')).map((x) => [x.id, x.position]);

/** The first view that satisfies `p`. */
function when(manager: SyncManager, p: (view: SyncView) => boolean): Promise<SyncView> {
  if (p(manager.view)) return Promise.resolve(manager.view);
  return new Promise((resolve) => {
    const stop = manager.watch((view) => {
      if (!p(view)) return;
      stop();
      resolve(view);
    });
  });
}

/** A manager whose waits are instant and recorded, so backoff is assertable. */
function start(door: LiveDoor, r: LocalReplica, declarations: { spec: OpSpec; params?: Record<string, string> }[] = []) {
  return SyncManager.start(door.client(), r, declarations, {
    wait: async (ms) => {
      door.waits.push(ms);
    },
  });
}

describe('the sync manager', () => {
  it('syncs a declared op at once and says where the device stands', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = start(door, r, [{ spec: tasks }]);
    (await door.nth(0)).push(load([task(1), task(2)], 5));

    const view = await when(manager, (v) => v.complete);
    expect(view.position).toBe(5);
    expect(view.ops).toHaveLength(1);
    expect(view.ops[0]).toMatchObject({ key: 'std.tasks.list', loaded: 2, complete: true, stopped: null });
    expect(await rows(r)).toEqual([
      ['task_1', 5],
      ['task_2', 5],
    ]);
    await manager.stop();
  });

  it('advances screens only at a position every op has reached', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica());
    manager.sync(tasks);
    const first = await door.nth(0);
    manager.sync(mine);
    const second = await door.nth(1);

    first.push(load([task(1)], 7));
    second.push(load([task(2)], 3));
    // The screen sees 3: one op has not caught up, so 7 is not a common answer.
    expect((await when(manager, (v) => v.complete)).position).toBe(3);

    second.push(change(9, [task(2, 'edited')], []));
    expect((await when(manager, (v) => v.position === 7)).position).toBe(7);
    await manager.stop();
  });

  it('a reconnect cannot move the device, or the screen, backwards', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = start(door, r, [{ spec: tasks }]);
    (await door.nth(0)).push(load([task(1, 'new')], 9));
    await when(manager, (v) => v.position === 9);

    // The feed drops and the reload answers at a lower position.
    (await door.nth(0)).close();
    (await door.nth(1)).push(load([task(1, 'old')], 4));
    await when(manager, (v) => v.complete && v.ops[0]!.position === 4);

    expect(manager.view.position).toBe(9);
    expect(await rows(r)).toEqual([['task_1', 9]]);
    await manager.stop();
  });

  it('reconnects when a feed ends, and the reload releases what the server no longer returns', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = start(door, r, [{ spec: tasks }]);
    (await door.nth(0)).push(load([task(1), task(2)], 3));
    await when(manager, (v) => v.complete);

    (await door.nth(0)).close();
    // While the device was away, task_2 left this op's result.
    (await door.nth(1)).push(load([task(1)], 8));
    await when(manager, (v) => v.complete && v.ops[0]!.position === 8);

    expect(door.urls).toHaveLength(2);
    expect(await rows(r)).toEqual([['task_1', 8]]);
    await manager.stop();
  });

  it('a guard change removes rows from the device', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = start(door, r, [{ spec: tasks }]);
    const feed = await door.nth(0);
    feed.push(load([task(1), task(2)], 3));
    await when(manager, (v) => v.complete);

    // A membership was dropped: the rows it hid arrive as deletes.
    feed.push(change(6, [], ['task_1', 'task_2']));
    await when(manager, (v) => v.position === 6);

    expect(await rows(r)).toEqual([]);
    await manager.stop();
  });

  it('a standing refusal stops that op, without asking again', async () => {
    const door = new LiveDoor().refuse(403, 'access_denied');
    const manager = start(door, await replica(), [{ spec: tasks }]);
    const view = await when(manager, (v) => v.ops[0]?.stopped !== null);

    expect(view.ops[0]!.stopped?.detail.class).toBe('access_denied');
    expect(door.urls).toHaveLength(1);
    await manager.stop();
  });

  // A guest's window token names one grant, and a second accept replaces it
  // while the window stays open: the door refuses the held token mid-feed.
  // notes_app `the_page_keeps_notes_on_the_device_and_erases_a_closed_share`
  // failed on this before the manager asked the client for a new token.
  it('a refused token the client renews reconnects at once and keeps the rows', async () => {
    const door = new LiveDoor().open().open();
    door.renewals = [true];
    const r = await replica();
    const manager = start(door, r, [{ spec: tasks }]);
    const first = await door.nth(0);
    first.push(load([task(1)], 5));
    await when(manager, (v) => v.complete);
    first.push(frame('error', { error: { code: 'access_denied', message: 'op is not callable with this token' } }));
    first.close();

    (await door.nth(1)).push(load([task(1), task(2)], 6));
    const view = await when(manager, (v) => v.position === 6);
    expect(view.ops[0]!.stopped).toBeNull();
    expect(door.renewed).toBe(1);
    expect(door.waits).toEqual([]);
    expect((await rows(r)).map(([id]) => id)).toEqual(['task_1', 'task_2']);
    await manager.stop();
  });

  it('a refused token the door will not renew stops the op and releases its rows', async () => {
    const door = new LiveDoor().open();
    door.renewals = [false];
    const r = await replica();
    const manager = start(door, r, [{ spec: tasks }]);
    const first = await door.nth(0);
    first.push(load([task(1)], 5));
    await when(manager, (v) => v.complete);
    first.push(frame('error', { error: { code: 'access_denied', message: 'op is not callable with this token' } }));
    first.close();

    const view = await when(manager, (v) => v.ops[0]?.stopped !== null);
    expect(view.ops[0]!.stopped?.detail.class).toBe('access_denied');
    expect(door.renewed).toBe(1);
    expect(door.urls).toHaveLength(1);
    await manager.stop();
    expect(await rows(r)).toEqual([]);
  });

  it('a token refused again straight after a renewal stops the op, asking once', async () => {
    const door = new LiveDoor().refuse(403, 'access_denied').refuse(403, 'access_denied');
    door.renewals = [true, true];
    const manager = start(door, await replica(), [{ spec: tasks }]);
    const view = await when(manager, (v) => v.ops[0]?.stopped !== null);

    expect(view.ops[0]!.stopped?.detail.class).toBe('access_denied');
    expect(door.renewed).toBe(1);
    expect(door.urls).toHaveLength(2);
    await manager.stop();
  });

  it('an op that gave up does not freeze the ops still running', async () => {
    const door = new LiveDoor().refuse(403, 'access_denied').open();
    const manager = start(door, await replica());
    manager.sync(tasks);
    await when(manager, (v) => v.ops[0]?.stopped !== null);
    manager.sync(mine);
    (await door.nth(0)).push(load([task(1)], 5));

    const view = await when(manager, (v) => v.complete);
    expect(view.position).toBe(5);
    expect(view.ops.map((o) => o.stopped !== null)).toEqual([true, false]);
    await manager.stop();
  });

  it('a refusal worth another try reconnects, waiting longer each time', async () => {
    const door = new LiveDoor().refuse(500, 'internal').refuse(500, 'internal').open();
    const manager = start(door, await replica(), [{ spec: tasks }]);
    (await door.nth(0)).push(load([task(1)], 4));
    await when(manager, (v) => v.complete);

    expect(door.urls).toHaveLength(3);
    expect(door.waits).toEqual([250, 500]);
    expect(manager.view.ops[0]!.stopped).toBeNull();
    await manager.stop();
  });

  it('a busy door backs off for its Retry-After and resumes, never stopping the op', async () => {
    const door = new LiveDoor()
      .refuse(503, 'server_busy', '2')
      .refuse(503, 'tenant_not_ready', '1')
      .refuse(503, 'server_busy')
      .open();
    const manager = start(door, await replica(), [{ spec: tasks }]);
    (await door.nth(0)).push(load([task(1)], 4));
    const view = await when(manager, (v) => v.complete);

    expect(door.urls).toHaveLength(4);
    // The door's wait is a floor under the backoff (250, 500, 1000).
    expect(door.waits).toEqual([2000, 1000, 1000]);
    expect(view.ops[0]!.stopped).toBeNull();
    await manager.stop();
  });

  it('settled() waits for every op to have loaded at a common position', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica());
    manager.sync(tasks);
    const first = await door.nth(0);
    manager.sync(mine);
    const second = await door.nth(1);

    first.push(load([task(1)], 6));
    let settled: SyncView | null = null;
    const waiting = manager.settled().then((v) => (settled = v));
    await Promise.resolve();
    expect(settled).toBeNull();

    second.push(load([task(2)], 8));
    await waiting;
    expect(settled).toMatchObject({ complete: true, position: 6 });
    await manager.stop();
  });

  it('dropping an op releases the rows only it held', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = start(door, r);
    const a = manager.sync(tasks);
    (await door.nth(0)).push(load([task(1)], 3));
    const b = manager.sync(mine);
    (await door.nth(1)).push(load([task(1)], 3));
    await when(manager, (v) => v.complete && v.ops.length === 2);

    await manager.drop(a);
    expect(await rows(r)).toEqual([['task_1', 3]]);
    expect(manager.view.ops.map((o) => o.key)).toEqual([b]);

    await manager.drop(b);
    expect(await rows(r)).toEqual([]);
    // A dropped op does not reconnect.
    expect(door.urls).toHaveLength(2);
    await manager.stop();
  });

  it('one op synced at two sets of parameters holds its rows apart', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = start(door, r);
    const a = manager.sync(byOwner, { owner: 'mem_1' });
    (await door.nth(0)).push(load([task(1)], 3));
    const b = manager.sync(byOwner, { owner: 'mem_2' });
    (await door.nth(1)).push(load([task(1)], 3));
    await when(manager, (v) => v.complete && v.ops.length === 2);

    expect([a, b]).toEqual(['tasksByOwner?owner=mem_1', 'tasksByOwner?owner=mem_2']);
    expect(door.urls[0]).toContain('owner=mem_1');
    expect(door.urls[1]).toContain('owner=mem_2');

    // Dropping one leaves the row the other still holds.
    await manager.drop(a);
    expect(await rows(r)).toEqual([['task_1', 3]]);
    await manager.drop(b);
    expect(await rows(r)).toEqual([]);
    await manager.stop();
  });

  it('opens every feed past the browser cache, so a second feed on the same URL never waits on it', async () => {
    // Notes e2e, Firefox (2026-10-20): the space rides in a header, so two
    // tabs' feeds, or one op in two spaces, share a URL. Firefox made the
    // second wait on the HTTP cache entry the first, endless, stream held.
    const door = new LiveDoor();
    const manager = start(door, await replica(), [{ spec: tasks }, { spec: mine }]);
    await door.nth(1);
    expect(door.caches).toEqual(['no-store', 'no-store']);
    await manager.stop();
  });

  it('stop ends every feed and opens no more', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica(), [{ spec: tasks }, { spec: mine }]);
    await door.nth(1);
    await manager.stop();

    const opened = door.urls.length;
    await new Promise((r) => setTimeout(r, 5));
    expect(door.urls).toHaveLength(opened);
  });
});

describe('a refusal purges what that op synced (C7)', () => {
  it('releases the revoked op’s rows, and keeps what another op still holds', async () => {
    // Two ops hold task_1. The first is revoked on reconnect; its rows go,
    // and the row itself survives because the second op still holds it.
    const door = new LiveDoor().open().open().refuse(403, 'access_denied').refuse(403, 'access_denied');
    const r = await replica();
    const manager = start(door, r);

    manager.sync(tasks);
    const feedA = await door.nth(0);
    manager.sync(mine);
    const feedB = await door.nth(1);
    feedA.push(load([task(1)], 3));
    feedB.push(load([task(1)], 3));
    await when(manager, (v) => v.complete && v.ops.length === 2);
    expect(await rows(r)).toEqual([['task_1', 3]]);

    feedA.close();
    await when(manager, (v) => v.ops[0]!.stopped !== null);
    expect(manager.view.ops[0]!.stopped?.detail.class).toBe('access_denied');
    expect(await rows(r)).toEqual([['task_1', 3]]);

    feedB.close();
    await when(manager, (v) => v.ops[1]!.stopped !== null);
    expect(await rows(r)).toEqual([]);
    await manager.stop();
  });

  it('a refusal that is not about access keeps the rows', async () => {
    // An op renamed in a deploy answers `not_found`. Throwing away rows the
    // caller is still entitled to would be the wrong repair, so the op stops
    // and the rows stay.
    const door = new LiveDoor().open().refuse(404, 'not_found');
    const r = await replica();
    const manager = start(door, r, [{ spec: tasks }]);
    (await door.nth(0)).push(load([task(1)], 5));
    await when(manager, (v) => v.complete);

    (await door.nth(0)).close();
    await when(manager, (v) => v.ops[0]!.stopped !== null);

    expect(manager.view.ops[0]!.stopped?.detail.class).toBe('not_found');
    expect(await rows(r)).toEqual([['task_1', 5]]);
    await manager.stop();
  });
});

describe('a refined sync (R6)', () => {
  const titled = (t: string) => ({ where: [{ field: 'title', op: 'eq' as const, value: t }] });

  it('two filters on one op are two holders over one row store', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = start(door, r);
    const a = manager.sync(tasks, {}, titled('task 1'));
    (await door.nth(0)).push(load([task(1)], 3));
    const b = manager.sync(tasks, {}, titled('task 2'));
    (await door.nth(1)).push(load([task(2)], 3));
    await when(manager, (v) => v.complete && v.ops.length === 2);

    expect(a).not.toBe(b);
    expect(a.startsWith('std.tasks.list?$refine=')).toBe(true);
    // Each feed carried its own `where` to the door.
    const sent = door.urls.map((u) => JSON.parse(new URL(u).searchParams.get('$refine')!) as unknown);
    expect(sent).toEqual([titled('task 1'), titled('task 2')]);
    expect(await rows(r)).toEqual([
      ['task_1', 3],
      ['task_2', 3],
    ]);

    // The same op and filter declared again is the same sync.
    expect(manager.sync(tasks, {}, titled('task 1'))).toBe(a);

    // A row leaving one filter goes from that holder only.
    (await door.nth(0)).push(change(4, [], ['task_1']));
    await when(manager, (v) => v.ops.find((o) => o.key === a)?.position === 4);
    expect(await rows(r)).toEqual([['task_2', 3]]);
    await manager.drop(b);
    expect(await rows(r)).toEqual([]);
    await manager.stop();
  });

  it('a pick is refused when the sync is declared, before any connection', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica());
    expect(() => manager.sync(tasks, {}, { pick: ['title'] })).toThrowError(/refuses its `pick`/);
    expect(door.urls).toHaveLength(0);
    expect(manager.view.ops).toHaveLength(0);
    await manager.stop();
  });
});

describe('observe: rows in memory for a watched handle (H1)', () => {
  /** The first snapshot of `obs` that satisfies `p`. */
  function seen(
    manager: SyncManager,
    key: string,
    p: (s: ObservedRows) => boolean,
    log?: ObservedRows[],
  ): { done: Promise<ObservedRows>; obs: Observation } {
    let resolve!: (s: ObservedRows) => void;
    const done = new Promise<ObservedRows>((r) => {
      resolve = r;
    });
    const obs = manager.observe(key, (s) => {
      log?.push(s);
      if (p(s)) resolve(s);
    });
    if (p(obs.snapshot())) resolve(obs.snapshot());
    return { done, obs };
  }

  const ids = (s: ObservedRows) => s.rows.map((r) => r.id);
  const marked = (f: number, i: number) => ({ id: `task_${String(f * 10 + i).padStart(4, '0')}`, title: `frame ${f}` });

  it('a load with an observer reads each row from the store at most once', async () => {
    const n = 2_000;
    const frames = Array.from({ length: n / 50 }, (_, f) =>
      frame('load', {
        entity: 'tasks',
        key: 'id',
        position: 2,
        pin: null,
        rows: Array.from({ length: 50 }, (_, i) => task(f * 50 + i + 1)),
        loaded: (f + 1) * 50,
        complete: f === n / 50 - 1,
      }),
    );
    const readsOf = async (observing: boolean) => {
      const door = new LiveDoor();
      const backend = new CountingBackend();
      const r = await LocalReplica.open(backend, 'spc_test', 'usr_1');
      const manager = start(door, r, [{ spec: tasks }]);
      const feed = await door.nth(0);
      let watched: Promise<ObservedRows> | null = null;
      if (observing) {
        const { done, obs } = seen(manager, 'std.tasks.list', (s) => s.filled);
        await done;
        watched = seen(manager, 'std.tasks.list', (s) => s.rows.length === n && s.state?.complete === true).done;
        obs.stop();
      }
      backend.reset();
      for (const f of frames) feed.push(f);
      await when(manager, (v) => v.complete);
      if (watched) expect((await watched).rows).toHaveLength(n);
      const reads = backend.keysRead;
      await manager.stop();
      return reads;
    };
    const alone = await readsOf(false);
    const observed = await readsOf(true);
    // The observer adds no read of the store once it is filled: the load's
    // own reads (one per row and its holders, in one read per frame) are all.
    expect(observed).toBe(alone);
    expect(alone).toBeLessThanOrEqual(2 * n);
  });

  for (const publishMs of [0, 100]) {
    it(`every snapshot is a whole number of frames (publishMs ${publishMs})`, async () => {
      const door = new LiveDoor();
      const manager = SyncManager.start(door.client(), await replica(), [{ spec: tasks }], {
        wait: async () => undefined,
        publishMs,
      });
      const feed = await door.nth(0);
      const log: ObservedRows[] = [];
      const { done, obs } = seen(manager, 'std.tasks.list', (s) => s.state?.complete === true, log);
      const total = 30;
      for (let f = 0; f < total; f++) {
        feed.push(
          frame('load', {
            entity: 'tasks',
            key: 'id',
            position: 1,
            pin: null,
            rows: Array.from({ length: 10 }, (_, i) => marked(f, i)),
            loaded: (f + 1) * 10,
            complete: f === total - 1,
          }),
        );
      }
      const last = await done;
      expect(last.rows).toHaveLength(total * 10);
      for (const s of log) {
        // The first k frames exactly: ten rows of each, in id order.
        const k = s.rows.length / 10;
        expect(Number.isInteger(k)).toBe(true);
        expect(ids(s)).toEqual(Array.from({ length: k * 10 }, (_, j) => marked(Math.floor(j / 10), j % 10).id));
        // Rows and `complete` come from one frame.
        if (s.state?.complete) expect(s.rows).toHaveLength(total * 10);
        if (s.rows.length < total * 10) expect(s.state?.complete).not.toBe(true);
      }
      // Each snapshot's changes take the one before it to it (I6).
      for (let i = 1; i < log.length; i++) {
        const [prev, s] = [log[i - 1]!, log[i]!];
        expect(s.version).toBe(prev.version + 1);
        expect(s.changes?.from).toBe(prev.version);
        const removed = new Set(s.changes!.removed);
        const rebuilt = new Set([...prev.rows.filter((r) => !removed.has(r)), ...s.changes!.added]);
        expect(rebuilt).toEqual(new Set(s.rows));
      }
      if (publishMs > 0) expect(log.length).toBeLessThan(total);
      obs.stop();
      await manager.stop();
    });
  }

  it('frames that arrive during the first fill are applied after it, whole', async () => {
    const door = new LiveDoor();
    const manager = SyncManager.start(door.client(), await replica(), [{ spec: tasks }], {
      wait: async () => undefined,
      publishMs: 0,
    });
    const feed = await door.nth(0);
    const push = (f: number, complete: boolean) =>
      feed.push(
        frame('load', {
          entity: 'tasks',
          key: 'id',
          position: 1,
          pin: null,
          rows: Array.from({ length: 10 }, (_, i) => marked(f, i)),
          loaded: (f + 1) * 10,
          complete,
        }),
      );
    for (let f = 0; f < 5; f++) push(f, false);
    await when(manager, (v) => v.ops[0]!.loaded === 50);
    const log: ObservedRows[] = [];
    const { done, obs } = seen(manager, 'std.tasks.list', (s) => s.state?.complete === true, log);
    // The fill is reading the store now; these land while it does.
    for (let f = 5; f < 10; f++) push(f, f === 9);
    const last = await done;
    expect(ids(last)).toEqual(Array.from({ length: 100 }, (_, j) => marked(Math.floor(j / 10), j % 10).id));
    for (const s of log) expect(s.rows.length % 10).toBe(0);
    obs.stop();
    await manager.stop();
  });

  it('a lower-position upsert leaves the snapshot alone', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica(), [{ spec: tasks }]);
    const { obs } = seen(manager, 'std.tasks.list', () => false);
    (await door.nth(0)).push(load([task(1, 'new')], 9));
    await when(manager, (v) => v.position === 9);
    // The feed drops and the reload answers at a lower position.
    (await door.nth(0)).close();
    (await door.nth(1)).push(load([task(1, 'old'), task(2)], 4));
    const after = await seen(manager, 'std.tasks.list', (s) => s.rows.length === 2 && s.state?.complete === true).done;
    expect(after.rows.map((r) => [r.id, r.position, (r.row as { title: string }).title])).toEqual([
      ['task_1', 9, 'new'],
      ['task_2', 4, 'task 2'],
    ]);
    obs.stop();
    await manager.stop();
  });

  it('an upsert through op A reaches op B’s snapshot', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica());
    manager.sync(tasks);
    const feedA = await door.nth(0);
    manager.sync(mine);
    const feedB = await door.nth(1);
    feedA.push(load([task(1), task(2)], 3));
    feedB.push(load([task(1)], 3));
    await when(manager, (v) => v.complete && v.ops.length === 2);
    const b = seen(manager, 'myTasks', (s) => (s.rows[0]?.row as { title?: string } | undefined)?.title === 'edited');
    await seen(manager, 'myTasks', (s) => s.filled).done;
    feedA.push(change(5, [task(1, 'edited')], []));
    const snap = await b.done;
    expect(ids(snap)).toEqual(['task_1']);
    expect(snap.rows[0]!.position).toBe(5);
    b.obs.stop();
    await manager.stop();
  });

  it('a change to one row keeps every other row object', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica(), [{ spec: tasks }]);
    const feed = await door.nth(0);
    const first = seen(manager, 'std.tasks.list', (s) => s.rows.length === 3 && s.state?.complete === true);
    feed.push(load([task(1), task(2), task(3)], 1));
    const before = await first.done;
    const next = seen(manager, 'std.tasks.list', (s) => s.state?.position === 2);
    feed.push(change(2, [task(2, 'edited')], []));
    const after = await next.done;
    expect(after.rows[0]).toBe(before.rows[0]);
    expect(after.rows[2]).toBe(before.rows[2]);
    expect(after.rows[1]).not.toBe(before.rows[1]);
    expect((after.rows[1]!.row as { title: string }).title).toBe('edited');
    first.obs.stop();
    next.obs.stop();
    await manager.stop();
  });

  it('a delete in a change frame leaves the snapshot, and a restart fills from the store', async () => {
    const door = new LiveDoor();
    const backend = new MemoryBackend();
    const r = await LocalReplica.open(backend, 'spc_test', 'usr_1');
    const manager = start(door, r, [{ spec: tasks }]);
    const feed = await door.nth(0);
    feed.push(load([task(1), task(2)], 1));
    await when(manager, (v) => v.complete);
    const gone = seen(manager, 'std.tasks.list', (s) => s.filled && ids(s).join() === 'task_2');
    feed.push(change(2, [], ['task_1']));
    await gone.done;
    gone.obs.stop();
    await manager.stop();

    // A new manager over the same store, the network away: it fills from disk.
    const offline = new LiveDoor().refuse(503, 'internal');
    const again = SyncManager.start(offline.client(), r, [{ spec: tasks }], { wait: () => new Promise(() => undefined) });
    const filled = await seen(again, 'std.tasks.list', (s) => s.filled).done;
    expect(ids(filled)).toEqual(['task_2']);
    await again.stop();
  });

  it('poison clears every observer before the purge starts', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = start(door, r);
    manager.sync(tasks);
    const feedA = await door.nth(0);
    manager.sync(mine);
    const feedB = await door.nth(1);
    feedA.push(load([task(1), task(2)], 3));
    feedB.push(load([task(3)], 3));
    const a = seen(manager, 'std.tasks.list', (s) => s.rows.length === 2);
    const b = seen(manager, 'myTasks', (s) => s.rows.length === 1);
    await Promise.all([a.done, b.done]);
    const purging = r.poison();
    // Before the purge has run a single step, memory is already empty.
    expect(a.obs.snapshot().rows).toEqual([]);
    expect(b.obs.snapshot().rows).toEqual([]);
    await purging;
    expect(await r.scan('tasks')).toEqual([]);
    a.obs.stop();
    b.obs.stop();
    await manager.stop();
  });

  it('a revoking refusal empties the snapshot at once, and a row another op holds stays there', async () => {
    const door = new LiveDoor().open().open().refuse(403, 'access_denied');
    const manager = start(door, await replica());
    manager.sync(tasks);
    const feedA = await door.nth(0);
    manager.sync(mine);
    const feedB = await door.nth(1);
    feedA.push(load([task(1), task(2)], 3));
    feedB.push(load([task(1)], 3));
    const a = seen(manager, 'std.tasks.list', (s) => s.rows.length === 2);
    const b = seen(manager, 'myTasks', (s) => s.rows.length === 1);
    await Promise.all([a.done, b.done]);
    const emptied = seen(manager, 'std.tasks.list', (s) => s.state?.stopped != null);
    feedA.close();
    const snap = await emptied.done;
    expect(snap.rows).toEqual([]);
    expect(snap.state?.stopped?.detail.class).toBe('access_denied');
    expect(ids(b.obs.snapshot())).toEqual(['task_1']);
    for (const o of [a, b, emptied]) o.obs.stop();
    await manager.stop();
  });

  it('a dropped handle shows nothing', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica(), [{ spec: tasks }]);
    const feed = await door.nth(0);
    const a = seen(manager, 'std.tasks.list', (s) => s.rows.length === 1);
    feed.push(load([task(1)], 1));
    await a.done;
    await manager.drop('std.tasks.list');
    expect(a.obs.snapshot().rows).toEqual([]);
    expect(a.obs.snapshot().state).toBeNull();
    a.obs.stop();
    await manager.stop();
  });
});

/**
 * Notes e2e, test 7: a live filtered list (`notesTagged($tag)`) kept a note
 * after an edit took the tag off. The row's fields followed the edit, through
 * the unfiltered op's frames, but its place in the filtered answer did not.
 *
 * Two causes, each with a test here:
 * - a manager that replaced another filled its memory from the store while
 *   the old one was still writing, so a row the old one released stayed in
 *   memory; a completed load now sets memory to exactly what it holds;
 * - change frames left the stored result hash at the last load's, so a
 *   reconnect could resume onto rows the device no longer matched.
 */
describe('a filtered live read keeps its membership', () => {
  const tagged: OpSpec = {
    ...tasks,
    op: 'tasksTagged',
    params: [{ name: 'tag', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null }],
  };
  const key = 'tasksTagged?tag=work';

  function watch(manager: SyncManager, handle: string) {
    const obs = manager.observe(handle, () => undefined);
    const until = async (p: (s: ObservedRows) => boolean): Promise<ObservedRows> => {
      for (let i = 0; i < 400; i++) {
        const s = obs.snapshot();
        if (p(s)) return s;
        await new Promise((r) => setTimeout(r, 5));
      }
      throw new Error(`never: ${JSON.stringify(obs.snapshot().rows.map((r) => r.id))}`);
    };
    return { obs, until };
  }
  const idsOf = (s: ObservedRows) => s.rows.map((r) => r.id).join();
  const titleOf = (s: ObservedRows, id: string) =>
    (s.rows.find((r) => r.id === id)?.row as { title?: string } | undefined)?.title;

  it('a row that stops matching leaves the live result, and the unfiltered op keeps its new fields', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica());
    manager.sync(tasks);
    const all = await door.nth(0);
    manager.sync(tagged, { tag: 'work' });
    const work = await door.nth(1);
    all.push(load([task(1, 'work'), task(2)], 3));
    work.push(load([task(1, 'work')], 3));
    const a = watch(manager, 'std.tasks.list');
    const t = watch(manager, key);
    await t.until((s) => idsOf(s) === 'task_1');

    // The edit takes the tag off: the unfiltered op upserts the row, the
    // filtered op deletes it.
    all.push(change(4, [task(1, 'no tag')], []));
    work.push(change(4, [], ['task_1']));
    await t.until((s) => s.rows.length === 0);
    const after = await a.until((s) => titleOf(s, 'task_1') === 'no tag');
    expect(idsOf(after)).toBe('task_1,task_2');
    a.obs.stop();
    t.obs.stop();
    await manager.stop();
  });

  it('a row that starts matching joins the live result', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica());
    manager.sync(tasks);
    const all = await door.nth(0);
    manager.sync(tagged, { tag: 'work' });
    const work = await door.nth(1);
    all.push(load([task(1), task(2)], 3));
    work.push(load([], 3));
    const t = watch(manager, key);
    await t.until((s) => s.filled && s.state?.complete === true);

    all.push(change(4, [task(2, 'work')], []));
    work.push(change(4, [task(2, 'work')], []));
    const joined = await t.until((s) => idsOf(s) === 'task_2');
    expect(titleOf(joined, 'task_2')).toBe('work');
    t.obs.stop();
    await manager.stop();
  });

  it('under fast successive edits the live result ends at the last one', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = SyncManager.start(door.client(), r, [], { publishMs: 100 });
    manager.sync(tagged, { tag: 'work' });
    const work = await door.nth(0);
    work.push(load([task(1, 'work')], 3));
    const t = watch(manager, key);
    await t.until((s) => idsOf(s) === 'task_1');

    // Task 1 goes off, on, off, on, off and task 2 on, off, on, with no
    // pause: every frame lands inside one publishing interval.
    work.push(change(4, [], ['task_1']));
    work.push(change(5, [task(1, 'work again'), task(2, 'work')], []));
    work.push(change(6, [], ['task_1', 'task_2']));
    work.push(change(7, [task(1, 'work 3')], []));
    work.push(change(8, [task(2, 'work 4')], ['task_1']));
    const last = await t.until((s) => s.state?.position === 8 && idsOf(s) === 'task_2');
    expect(titleOf(last, 'task_2')).toBe('work 4');
    // The store agrees with memory.
    expect((await r.holdings(key)).map((h) => h.id)).toEqual(['task_2']);
    t.obs.stop();
    await manager.stop();
  });

  it('a row another writer released between the fill and the load leaves memory', async () => {
    // Two managers on one replica: the page's old one, still stopping, and
    // the new one that replaced it. The new one fills its memory from the
    // store before the old one's last frame lands.
    const r = await replica();
    const oldDoor = new LiveDoor();
    const old = start(oldDoor, r);
    old.sync(tasks);
    const oldAll = await oldDoor.nth(0);
    old.sync(tagged, { tag: 'work' });
    const oldWork = await oldDoor.nth(1);
    oldAll.push(load([task(1, 'work'), task(2)], 3));
    oldWork.push(load([task(1, 'work')], 3));
    await when(old, (v) => v.complete && v.ops.length === 2);

    const newDoor = new LiveDoor();
    let open!: () => void;
    newDoor.gate = new Promise((resolve) => {
      open = resolve;
    });
    const next = start(newDoor, r);
    next.sync(tasks);
    next.sync(tagged, { tag: 'work' });
    const t = watch(next, key);
    // The fill has read the store: the row is there.
    await t.until((s) => s.filled && idsOf(s) === 'task_1');

    // The old manager hears the edit that took the tag off, and lets go.
    oldAll.push(change(4, [task(1, 'no tag')], []));
    oldWork.push(change(4, [], ['task_1']));
    await when(old, (v) => v.position === 4);
    await old.stop();

    // The new manager's feeds answer now, with the result after the edit.
    open();
    (await newDoor.nth(0)).push(load([task(1, 'no tag'), task(2)], 4));
    (await newDoor.nth(1)).push(load([], 4));
    const settled = await t.until((s) => s.state?.complete === true && s.rows.length === 0);
    expect(settled.rows).toEqual([]);
    expect(await r.holdings(key)).toEqual([]);
    t.obs.stop();
    await next.stop();
  });

  it('a change frame keeps the stored result hash, so a reconnect resumes on what the device holds', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica());
    manager.sync(tagged, { tag: 'work' });
    const first = await door.nth(0);
    first.push(load([], 3, true, 'h-empty'));
    first.push(change(4, [task(1, 'work')], [], 'h-one'));
    await when(manager, (v) => v.position === 4);
    first.close();
    await door.nth(1);
    // Not the load's 'h-empty': the server would resume on that once task 1
    // lost the tag again, and the device would keep it.
    expect(door.resumes).toEqual([null, 'h-one']);
    await manager.stop();
  });

  it('a change frame without a hash forgets the stored one, so the next connection loads in full', async () => {
    const door = new LiveDoor();
    const manager = start(door, await replica());
    manager.sync(tagged, { tag: 'work' });
    const first = await door.nth(0);
    first.push(load([], 3, true, 'h-empty'));
    first.push(change(4, [task(1, 'work')], []));
    await when(manager, (v) => v.position === 4);
    first.close();
    await door.nth(1);
    expect(door.resumes).toEqual([null, null]);
    await manager.stop();
  });
});

describe('reconnecting by itself', () => {
  /** A manager that hears `online` on `target`, with waits that never end unless woken. */
  const listening = (door: LiveDoor, r: LocalReplica, target: EventTarget | null) =>
    SyncManager.start(door.client(), r, [{ spec: tasks }], { wait: () => new Promise(() => undefined), online: target });

  it('nudge() closes an open feed and loads again at once, keeping the rows meanwhile', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = listening(door, r, null);
    (await door.nth(0)).push(load([task(1)], 5));
    await when(manager, (v) => v.complete);

    manager.nudge();
    const second = await door.nth(1);
    expect(await rows(r)).toEqual([['task_1', 5]]);
    second.push(load([task(1), task(2)], 6));
    await when(manager, (v) => v.position === 6);
    expect(await rows(r)).toEqual([
      ['task_1', 6],
      ['task_2', 6],
    ]);
    await manager.stop();
  });

  it('nudge() ends the wait before a retry', async () => {
    const door = new LiveDoor().refuse(500, 'internal').open();
    const manager = listening(door, await replica(), null);
    // The first call is refused with a retryable class, and the wait never ends on its own.
    await when(manager, (v) => (v.ops[0]?.attempts ?? 0) === 1);
    await new Promise((r) => setTimeout(r, 10));
    expect(door.feeds).toHaveLength(0);
    manager.nudge();
    await door.nth(0);
    await manager.stop();
  });

  it('an online event nudges every feed, and stop() stops listening', async () => {
    const door = new LiveDoor();
    const target = new EventTarget();
    const manager = listening(door, await replica(), target);
    (await door.nth(0)).push(load([task(1)], 5));
    await when(manager, (v) => v.complete);

    target.dispatchEvent(new Event('online'));
    (await door.nth(1)).push(load([task(1)], 6));
    await when(manager, (v) => v.position === 6);

    await manager.stop();
    const before = door.feeds.length;
    target.dispatchEvent(new Event('online'));
    await new Promise((r) => setTimeout(r, 10));
    expect(door.feeds).toHaveLength(before);
  });

  it('a feed the door refused for good stays stopped when nudged', async () => {
    const door = new LiveDoor().refuse(403, 'access_denied');
    const manager = listening(door, await replica(), null);
    await when(manager, (v) => v.ops[0] !== undefined && v.ops[0].stopped !== null);
    manager.nudge();
    await new Promise((r) => setTimeout(r, 10));
    expect(door.urls).toHaveLength(1);
    await manager.stop();
  });

  it('says why a connection failed, until a frame arrives', async () => {
    const cuts: typeof globalThis.fetch = async () => {
      throw new TypeError('Failed to fetch');
    };
    const client = new OrdoClient({ baseUrl: 'https://door.example/api', space: 's', token: 't', fetch: cuts });
    const manager = SyncManager.start(client, await replica(), [{ spec: tasks }], {
      wait: () => new Promise(() => undefined),
      online: null,
    });
    const view = await when(manager, (v) => v.ops[0]?.network != null);
    expect(view.ops[0]?.network).toMatchObject({ kind: 'unreachable' });
    await manager.stop();
  });
});

describe('reconnect(): reopening every feed in place (plan-sdk-device.md 5)', () => {
  const tagged: OpSpec = {
    ...tasks,
    op: 'tasksTagged',
    params: [{ name: 'tag', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null }],
  };
  const quiet = (door: LiveDoor, r: LocalReplica) =>
    SyncManager.start(door.client(), r, [], { wait: () => new Promise(() => undefined), online: null, publishMs: 0 });
  const tick = () => new Promise((r) => setTimeout(r, 10));

  it('opens a new feed per key only after the old one ended, and resolves when each answered', async () => {
    const door = new LiveDoor();
    const manager = quiet(door, await replica());
    manager.sync(tasks);
    manager.sync(mine);
    (await door.nth(0)).push(load([task(1)], 5));
    (await door.nth(1)).push(load([task(2)], 5));
    await when(manager, (v) => v.complete);

    let resolved = false;
    const done = manager.reconnect().then(() => {
      resolved = true;
    });
    const [a, b] = [await door.nth(2), await door.nth(3)];
    // Both old feeds had ended before either new call: never two per key.
    expect(door.openAtCall).toEqual([0, 1, 0, 1]);
    a.push(load([task(1)], 6));
    await tick();
    // One feed answered; the other has not, so the call is still waiting.
    expect(resolved).toBe(false);
    b.push(load([task(2)], 6));
    await done;
    expect(door.feeds).toHaveLength(4);
    await manager.stop();
  });

  it('keeps the same manager, so a screen keeps counting across a reconnect', async () => {
    const door = new LiveDoor();
    const manager = quiet(door, await replica());
    const key = manager.sync(tasks);
    (await door.nth(0)).push(load([task(1)], 5));
    const obs = manager.observe(key, () => undefined);
    await when(manager, (v) => v.complete);
    while (obs.snapshot().rows.length === 0) await tick();
    const before = obs.snapshot().version;
    const done = manager.reconnect();
    (await door.nth(1)).push(load([task(1), task(2)], 6));
    await done;
    while (obs.snapshot().rows.length !== 2) await tick();
    expect(obs.snapshot().version).toBeGreaterThan(before);
    expect(obs.snapshot().rows.map((r) => r.id)).toEqual(['task_1', 'task_2']);
    obs.stop();
    await manager.stop();
  });

  it('an edit between the abort and the reopen leaves a filtered handle exactly matching', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = quiet(door, r);
    const all = manager.sync(tasks);
    const allFeed = await door.nth(0);
    const work = manager.sync(tagged, { tag: 'work' });
    const workFeed = await door.nth(1);
    allFeed.push(load([task(1, 'work'), task(2)], 3));
    workFeed.push(load([task(1, 'work')], 3));
    const obs = manager.observe(work, () => undefined);
    await when(manager, (v) => v.complete);
    while (obs.snapshot().rows.length !== 1) await tick();

    // The tag comes off task 1 while the feeds are being reopened.
    const done = manager.reconnect();
    const [allAgain, workAgain] = [await door.nth(2), await door.nth(3)];
    allAgain.push(load([task(1, 'no tag'), task(2)], 4));
    workAgain.push(load([], 4));
    await done;
    await when(manager, (v) => v.complete && v.position === 4);
    while (obs.snapshot().rows.length !== 0) await tick();
    expect(obs.snapshot().rows).toEqual([]);
    expect(await r.holdings(work)).toEqual([]);
    expect(await r.holdings(all)).toHaveLength(2);
    obs.stop();
    await manager.stop();
  });

  it('a stopped op stays stopped, and the call still resolves', async () => {
    const door = new LiveDoor().refuse(403, 'access_denied');
    const manager = quiet(door, await replica());
    manager.sync(tasks);
    await when(manager, (v) => v.ops[0]?.stopped != null);
    await manager.reconnect();
    await tick();
    expect(door.urls).toHaveLength(1);
    expect(manager.view.ops[0]?.stopped?.detail.class).toBe('access_denied');
    await manager.stop();
  });

  it('overlapping calls share one promise and open one feed per key', async () => {
    const door = new LiveDoor();
    const manager = quiet(door, await replica());
    manager.sync(tasks);
    manager.sync(mine);
    (await door.nth(0)).push(load([task(1)], 5));
    (await door.nth(1)).push(load([task(2)], 5));
    await when(manager, (v) => v.complete);
    const a = manager.reconnect();
    const b = manager.reconnect();
    expect(b).toBe(a);
    (await door.nth(2)).push(load([task(1)], 6));
    (await door.nth(3)).push(load([task(2)], 6));
    await a;
    await tick();
    expect(door.feeds).toHaveLength(4);
    await manager.stop();
  });

  it('after a complete load it sends X-Ordo-Resume, and an unchanged result moves no rows', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = quiet(door, r);
    const key = manager.sync(tasks);
    (await door.nth(0)).push(load([task(1), task(2)], 5, true, 'h-two'));
    await when(manager, (v) => v.complete);
    const before = await rows(r);
    const done = manager.reconnect();
    (await door.nth(1)).push(
      frame('load', {
        entity: 'tasks',
        key: 'id',
        position: 7,
        pin: null,
        rows: [],
        loaded: 2,
        complete: true,
        result: 'h-two',
        resumed: true,
      }),
    );
    await done;
    await when(manager, (v) => v.complete && v.ops[0]?.position === 7);
    expect(door.resumes).toEqual([null, 'h-two']);
    expect(await rows(r)).toEqual(before);
    expect(await r.holdings(key)).toHaveLength(2);
    await manager.stop();
  });

  it('a feed waiting to retry stops waiting, and a stopped manager resolves at once', async () => {
    const door = new LiveDoor().refuse(500, 'internal').open();
    const manager = quiet(door, await replica());
    manager.sync(tasks);
    await when(manager, (v) => (v.ops[0]?.attempts ?? 0) === 1);
    const done = manager.reconnect();
    (await door.nth(0)).push(load([task(1)], 5));
    await done;
    await manager.stop();
    await manager.reconnect();
  });
});

describe('idle(): a sync nobody uses (plan-sdk-device.md 6)', () => {
  const quiet = (door: LiveDoor, r: LocalReplica) =>
    SyncManager.start(door.client(), r, [], { wait: () => new Promise(() => undefined), online: null, publishMs: 0 });
  const resumed = (position: number, hash: string, loaded: number) =>
    frame('load', { entity: 'tasks', key: 'id', position, pin: null, rows: [], loaded, complete: true, result: hash, resumed: true });

  it('ends the feed and leaves the view, and keeps the rows and the hash', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = quiet(door, r);
    manager.sync(tasks);
    const key = manager.sync(mine);
    (await door.nth(0)).push(load([task(1)], 5, true, 'h-all'));
    const feed = await door.nth(1);
    feed.push(load([task(2)], 5, true, 'h-mine'));
    await when(manager, (v) => v.complete);

    await manager.idle(key);
    expect(feed.ended).toBe(true);
    expect(manager.view.ops.map((o) => o.key)).toEqual(['std.tasks.list']);
    expect(await r.holdings(key)).toEqual([{ entity: 'tasks', id: 'task_2' }]);
    expect(await r.resultHash(key, mine.version)).toBe('h-mine');
    await manager.stop();
  });

  it('declaring it again resumes on the stored hash and moves no rows', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = quiet(door, r);
    const key = manager.sync(tasks);
    (await door.nth(0)).push(load([task(1), task(2)], 5, true, 'h-two'));
    await when(manager, (v) => v.complete);
    await manager.idle(key);
    const before = await rows(r);

    manager.sync(tasks);
    (await door.nth(1)).push(resumed(8, 'h-two', 2));
    await when(manager, (v) => v.complete && v.ops[0]?.position === 8);
    expect(door.resumes).toEqual([null, 'h-two']);
    expect(await rows(r)).toEqual(before);
    expect(await r.holdings(key)).toHaveLength(2);
    await manager.stop();
  });

  it('an edit while idle that took a row out: the resume misses, a full load runs, and the row leaves', async () => {
    const door = new LiveDoor();
    const r = await replica();
    const manager = quiet(door, r);
    const key = manager.sync(tasks);
    (await door.nth(0)).push(load([task(1), task(2)], 5, true, 'h-two'));
    await when(manager, (v) => v.complete);
    await manager.idle(key);

    manager.sync(tasks);
    // The server's hash differs, so it sends the whole result: task 2 is gone.
    (await door.nth(1)).push(load([task(1)], 9, true, 'h-one'));
    await when(manager, (v) => v.complete && v.ops[0]?.position === 9);
    expect(door.resumes).toEqual([null, 'h-two']);
    expect(await r.holdings(key)).toEqual([{ entity: 'tasks', id: 'task_1' }]);
    expect(await rows(r)).toEqual([['task_1', 9]]);
    await manager.stop();
  });

  it('two managers on one backend: A idles, B keeps its feed and its rows', async () => {
    const r = await replica();
    const doorA = new LiveDoor();
    const doorB = new LiveDoor();
    const a = quiet(doorA, r);
    const b = quiet(doorB, r);
    const key = a.sync(tasks);
    b.sync(tasks);
    (await doorA.nth(0)).push(load([task(1)], 5, true, 'h'));
    const feedB = await doorB.nth(0);
    feedB.push(load([task(1)], 5, true, 'h'));
    await when(a, (v) => v.complete);
    await when(b, (v) => v.complete);

    await a.idle(key);
    expect(feedB.ended).toBe(false);
    expect(b.view.ops).toHaveLength(1);
    expect(await r.holdings(key)).toHaveLength(1);
    feedB.push(change(6, [task(3)], []));
    await when(b, (v) => v.position === 6);
    expect(await r.holdings(key)).toHaveLength(2);
    await a.stop();
    await b.stop();
  });

  it('the common position can rise when an idle op leaves, and never falls', async () => {
    const door = new LiveDoor();
    const manager = quiet(door, await replica());
    manager.sync(tasks);
    const behind = manager.sync(mine);
    (await door.nth(0)).push(load([task(1)], 9));
    (await door.nth(1)).push(load([task(2)], 4));
    await when(manager, (v) => v.complete);
    expect(manager.view.position).toBe(4);
    await manager.idle(behind);
    expect(manager.view.position).toBe(9);
    await manager.stop();
  });
});
