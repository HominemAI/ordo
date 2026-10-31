/**
 * C4c of `plan-replica-feed.md`: the SDK keeping an op in sync on the device.
 *
 * The door is a fake that answers each call with a scripted event stream, so
 * the frames, their split across chunks, and a reconnect are all exact.
 */

import { LocalReplica, MemoryBackend, OrdoClient, OrdoLocalRefusal, OrdoRefusal, syncOp } from '../src/index.js';
import type { OpSpec, SyncState } from '../src/index.js';
import { q } from './harness.js';
import { CountingBackend } from './counting.js';

type Script = string[] | { status: number; json: unknown };

class FeedDoor {
  readonly calls: { url: string; headers: Record<string, string> }[] = [];
  readonly #scripts: Script[] = [];

  /** One script per expected call: the chunks of an event stream, or a refusal. */
  play(...scripts: Script[]): this {
    this.#scripts.push(...scripts);
    return this;
  }

  readonly fetch: typeof globalThis.fetch = async (input, init) => {
    this.calls.push({ url: String(input), headers: { ...(init?.headers as Record<string, string>) } });
    const next = this.#scripts.shift() ?? [];
    if (!Array.isArray(next)) {
      return new Response(JSON.stringify(next.json), {
        status: next.status,
        headers: { 'Content-Type': 'application/json' },
      });
    }
    const enc = new TextEncoder();
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        for (const chunk of next) c.enqueue(enc.encode(chunk));
        c.close();
      },
    });
    return new Response(body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
  };

  client(): OrdoClient {
    return new OrdoClient({ baseUrl: 'https://door.example/api', space: 'spc_test', token: 'tok', fetch: this.fetch });
  }
}

const frame = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
const load = (rows: unknown[], position: number, loaded: number, complete: boolean) =>
  frame('load', { entity: 'tasks', key: 'id', position, pin: null, rows, loaded, complete });
const change = (position: number, upserts: unknown[], deletes: string[]) =>
  frame('change', { entity: 'tasks', key: 'id', position, pin: null, upserts, deletes });
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

const replica = () => LocalReplica.open(new MemoryBackend(), 'spc_test', 'usr_1');
const held = async (r: LocalReplica) =>
  (await r.scan('tasks')).map((x) => [x.id, x.position, (x.row as { title: string }).title]);

describe('keeping an op in sync', () => {
  it('asks the door for the sync mode of live, with the ordinary headers', async () => {
    const door = new FeedDoor().play([load([], 1, 0, true)]);
    await syncOp(door.client(), await replica(), tasks);
    const call = door.calls[0]!;
    expect(q(call.url).get('live')).toBe('1');
    expect(q(call.url).get('sync')).toBe('1');
    expect(call.headers['X-Ordo-Space']).toBe('spc_test');
    expect(call.headers['Authorization']).toBe('Bearer tok');
    expect(call.headers['Accept']).toBe('text/event-stream');
  });

  it('reports progress, and says a load is partial until it completes', async () => {
    const door = new FeedDoor().play([load([task(1), task(2)], 3, 2, false), load([task(3)], 4, 3, true)]);
    const states: SyncState[] = [];
    await syncOp(door.client(), await replica(), tasks, {}, { onFrame: (s) => states.push(s) });
    expect(states.map((s) => [s.loaded, s.complete, s.position])).toEqual([
      [2, false, 3],
      [3, true, 4],
    ]);
  });

  it('a frame split one character per chunk arrives whole, and so do the frames after it (L2)', async () => {
    const stream = load([task(1), task(2)], 3, 2, false) + ':hb\n\n' + load([task(3)], 4, 3, true);
    const door = new FeedDoor().play([...stream]);
    const states: SyncState[] = [];
    await syncOp(door.client(), await replica(), tasks, {}, { onFrame: (s) => states.push(s) });
    expect(states.map((s) => [s.loaded, s.complete, s.position])).toEqual([
      [2, false, 3],
      [3, true, 4],
    ]);
  });

  it('a complete load is the whole result: a reload releases what it did not include', async () => {
    // The reconnect's one frame arrives split across two chunks.
    const reload = load([task(1), task(3, 'renamed')], 9, 2, true);
    const door = new FeedDoor().play(
      [load([task(1), task(2)], 3, 2, false), load([task(3)], 4, 3, true)],
      [reload.slice(0, 17), reload.slice(17)],
    );
    const r = await replica();
    await syncOp(door.client(), r, tasks);
    expect(await held(r)).toEqual([
      ['task_1', 3, 'task 1'],
      ['task_2', 3, 'task 2'],
      ['task_3', 4, 'task 3'],
    ]);
    const state = await syncOp(door.client(), r, tasks);
    expect(state).toMatchObject({ loaded: 2, complete: true, position: 9 });
    expect(await held(r)).toEqual([
      ['task_1', 9, 'task 1'],
      ['task_3', 9, 'renamed'],
    ]);
  });

  it('a change applies its upserts and deletes at its position', async () => {
    const door = new FeedDoor().play([
      load([task(1), task(2)], 3, 2, true),
      change(5, [task(2, 'edited'), task(4)], ['task_1']),
    ]);
    const r = await replica();
    const state = await syncOp(door.client(), r, tasks);
    expect(state.position).toBe(5);
    expect(await held(r)).toEqual([
      ['task_2', 5, 'edited'],
      ['task_4', 5, 'task 4'],
    ]);
  });

  it('a row held by two synced ops stays until neither holds it', async () => {
    const mine: OpSpec = { ...tasks, op: 'myTasks' };
    const door = new FeedDoor().play(
      [load([task(1)], 3, 1, true)],
      [load([task(1)], 3, 1, true), change(6, [], ['task_1'])],
      [load([], 7, 0, true)],
    );
    const r = await replica();
    await syncOp(door.client(), r, tasks);
    await syncOp(door.client(), r, mine);
    expect(await held(r)).toEqual([['task_1', 3, 'task 1']]);
    // The other op's reload no longer includes it, and nothing else holds it.
    await syncOp(door.client(), r, tasks);
    expect(await held(r)).toEqual([]);
  });

  it('a reload cannot move a row backwards', async () => {
    const door = new FeedDoor().play([load([task(1, 'new')], 9, 1, true)], [load([task(1, 'old')], 4, 1, true)]);
    const r = await replica();
    await syncOp(door.client(), r, tasks);
    await syncOp(door.client(), r, tasks);
    expect(await held(r)).toEqual([['task_1', 9, 'new']]);
  });

  it('an op that cannot be synced is refused before the wire', async () => {
    const door = new FeedDoor();
    await expect(syncOp(door.client(), await replica(), { ...tasks, sync: null })).rejects.toThrowError(
      OrdoLocalRefusal,
    );
    expect(door.calls).toHaveLength(0);
  });

  it("the door's refusal is the ordinary OrdoRefusal", async () => {
    const door = new FeedDoor().play({
      status: 400,
      json: { error: { code: 'not_live', message: 'op cannot be synced (no_anchor)', op: 'std.tasks.list' } },
    });
    await expect(syncOp(door.client(), await replica(), tasks)).rejects.toThrowError(OrdoRefusal);
  });

  it('an error frame ends the sync, and what was applied stays', async () => {
    const door = new FeedDoor().play([
      load([task(1)], 3, 1, true),
      frame('error', { error: { code: 'internal', message: 'the read failed', op: 'std.tasks.list' } }),
    ]);
    const r = await replica();
    await expect(syncOp(door.client(), r, tasks)).rejects.toThrowError(OrdoRefusal);
    expect(await held(r)).toEqual([['task_1', 3, 'task 1']]);
  });
});

describe('the generated table says which ops can be synced', () => {
  it('a list that projects its id can, keyed by it; one that does not cannot', async () => {
    const { SPECS } = await import('../src/generated/index.js');
    expect(SPECS['std.tasks.list'].sync).toEqual({ entity: 'tasks', key: 'id' });
    // `taskBoard` does not project its id, and since 2026-09-16 that no
    // longer stops it syncing: the executor attaches the anchor's id as
    // `$id` (`ui-gen.md` §1), so the device keys its rows by that.
    expect(SPECS.taskBoard.sync).toEqual({ entity: 'tasks', key: '$id' });
    expect(SPECS.specCount.sync).toBeNull();
    expect(SPECS.closeTask.sync).toBeNull();
  });
});

describe('what a device must never mirror (C7)', () => {
  it('refuses `std.space.*` before the wire, however syncable it looks', async () => {
    // The permission map is syncable by shape — stored rows of one entity
    // with an id — so nothing upstream stops it. A stolen device must not
    // yield it, so the refusal is here.
    const door = new FeedDoor();
    const spaceOp: OpSpec = { ...tasks, op: 'std.space.members', sync: { entity: 'members', key: 'id' } };
    await expect(syncOp(door.client(), await replica(), spaceOp)).rejects.toThrowError(OrdoLocalRefusal);
    await expect(syncOp(door.client(), await replica(), spaceOp)).rejects.toThrowError(/never mirrored/);
    expect(door.calls).toHaveLength(0);
  });

  it('an ordinary op with a similar name still syncs', async () => {
    // The guard is a prefix on the platform surface, not a word filter.
    const door = new FeedDoor().play([load([], 1, 0, true)]);
    const mine: OpSpec = { ...tasks, op: 'spaceTasks' };
    await syncOp(door.client(), await replica(), mine);
    expect(door.calls).toHaveLength(1);
  });
});

describe('a reconnect says what it holds (C4d)', () => {
  const loadWithHash = (rows: unknown[], position: number, hash: string) =>
    frame('load', {
      entity: 'tasks',
      key: 'id',
      position,
      pin: null,
      rows,
      loaded: rows.length,
      complete: true,
      result: hash,
    });
  const resumedFrame = (position: number, hash: string, loaded: number) =>
    frame('load', {
      entity: 'tasks',
      key: 'id',
      position,
      pin: null,
      rows: [],
      loaded,
      complete: true,
      result: hash,
      resumed: true,
    });

  it('sends no hash on a first connection, and the stored one on the next', async () => {
    const door = new FeedDoor().play([loadWithHash([task(1)], 3, 'abc123')], [resumedFrame(7, 'abc123', 1)]);
    const r = await replica();

    await syncOp(door.client(), r, tasks);
    expect(door.calls[0]!.headers['X-Ordo-Resume']).toBeUndefined();

    await syncOp(door.client(), r, tasks);
    expect(door.calls[1]!.headers['X-Ordo-Resume']).toBe('abc123');
  });

  it('a resumed frame keeps every row, where a complete load would have released them', async () => {
    // The trap: a resume carries no rows BECAUSE nothing changed. Treating it
    // as "a complete load returned nothing" would release the whole op.
    const door = new FeedDoor().play(
      [loadWithHash([task(1), task(2)], 3, 'h1')],
      [resumedFrame(9, 'h1', 2)],
    );
    const r = await replica();
    await syncOp(door.client(), r, tasks);
    expect(await held(r)).toEqual([
      ['task_1', 3, 'task 1'],
      ['task_2', 3, 'task 2'],
    ]);

    const state = await syncOp(door.client(), r, tasks);
    expect(state.complete).toBe(true);
    // Still there, still at the positions they were read at.
    expect(await held(r)).toEqual([
      ['task_1', 3, 'task 1'],
      ['task_2', 3, 'task 2'],
    ]);
  });

  it('a reload that is not a resume still releases what it left out', async () => {
    const door = new FeedDoor().play(
      [loadWithHash([task(1), task(2)], 3, 'h1')],
      [loadWithHash([task(1)], 8, 'h2')],
    );
    const r = await replica();
    await syncOp(door.client(), r, tasks);
    await syncOp(door.client(), r, tasks);
    expect(await held(r)).toEqual([['task_1', 8, 'task 1']]);
  });

  it('a hash minted under another op version is not sent back', async () => {
    const door = new FeedDoor().play([loadWithHash([task(1)], 3, 'old-hash')], [loadWithHash([task(1)], 4, 'new-hash')]);
    const r = await replica();
    await syncOp(door.client(), r, tasks);

    // The op redeployed: same name, new version, so the stored hash is not
    // about this result any more.
    const redeployed: OpSpec = { ...tasks, version: 'fedcba9876543210' };
    await syncOp(door.client(), r, redeployed);
    expect(door.calls[1]!.headers['X-Ordo-Resume']).toBeUndefined();
  });
});

describe('a refined sync (R6)', () => {
  it('sends the where as $refine, and its hash stays with its own holder', async () => {
    const where = [{ field: 'title', op: 'eq' as const, value: 'task 1' }];
    const door = new FeedDoor().play(
      [
        frame('load', {
          entity: 'tasks',
          key: 'id',
          position: 3,
          pin: null,
          rows: [task(1)],
          loaded: 1,
          complete: true,
          result: 'h-open',
        }),
      ],
      [load([], 4, 0, true)],
    );
    const r = await replica();
    await syncOp(door.client(), r, tasks, {}, { refine: { where }, holder: 'refined' });
    expect(JSON.parse(q(door.calls[0]!.url).get('$refine')!)).toEqual({ where });

    // The unrefined op, under its own holder, offers no hash of the filtered result.
    await syncOp(door.client(), r, tasks);
    expect(door.calls[1]!.headers['X-Ordo-Resume']).toBeUndefined();
    expect(q(door.calls[1]!.url).get('$refine')).toBeNull();
  });

  it('a pick is refused before the wire, as the door would', async () => {
    const door = new FeedDoor();
    await expect(
      syncOp(door.client(), await replica(), tasks, {}, { refine: { pick: ['title'] } }),
    ).rejects.toBeInstanceOf(OrdoLocalRefusal);
    expect(door.calls).toHaveLength(0);
  });

  it('a where on a field the op does not offer is refused before the wire', async () => {
    const door = new FeedDoor();
    await expect(
      syncOp(door.client(), await replica(), tasks, {}, {
        refine: { where: [{ field: 'nope', op: 'eq', value: 'x' }] },
      }),
    ).rejects.toThrowError(/unknown refinement field/);
    expect(door.calls).toHaveLength(0);
  });
});

describe('a frame is one write (H4)', () => {
  it('one write transaction per frame, the resume hash included', async () => {
    const backend = new CountingBackend();
    const r = await LocalReplica.open(backend, 'spc_test', 'usr_1');
    const rows = Array.from({ length: 120 }, (_, i) => task(i + 1));
    const done = frame('load', {
      entity: 'tasks', key: 'id', position: 4, pin: null, rows: rows.slice(100), loaded: 120, complete: true, result: 'h1',
    });
    const door = new FeedDoor().play([load(rows.slice(0, 50), 4, 50, false), load(rows.slice(50, 100), 4, 100, false), done]);
    backend.reset();
    await syncOp(door.client(), r, tasks);
    expect(backend.writes).toBe(3);
    expect(await r.resultHash('std.tasks.list', tasks.version)).toBe('h1');
    expect(await r.scan('tasks')).toHaveLength(120);
  });

  it('a change frame and the completing release are one write each', async () => {
    const backend = new CountingBackend();
    const r = await LocalReplica.open(backend, 'spc_test', 'usr_1');
    await syncOp(new FeedDoor().play([load([task(1), task(2), task(3)], 1, 3, true)]).client(), r, tasks);
    backend.reset();
    // A reload that leaves out task 3 releases it in the completing frame, and
    // a change after it upserts one row and deletes another.
    await syncOp(
      new FeedDoor().play([load([task(1), task(2)], 2, 2, true), change(3, [task(4)], ['task_1'])]).client(),
      r,
      tasks,
    );
    expect(backend.writes).toBe(2);
    expect((await held(r)).map(([id]) => id)).toEqual(['task_2', 'task_4']);
  });

  it('onFrame hands over what the frame changed, as the replica holds it', async () => {
    const r = await replica();
    await r.put({ entity: 'tasks', id: 'task_2', position: 9, row: task(2, 'newer') });
    const deltas: unknown[] = [];
    await syncOp(
      new FeedDoor().play([load([task(1), task(2, 'older')], 5, 2, true), change(6, [], ['task_1'])]).client(),
      r,
      tasks,
      {},
      { onFrame: (_state, delta) => deltas.push(delta) },
    );
    expect(deltas).toEqual([
      {
        reset: true,
        // C3 refused the older task 2, so the delta carries the version held.
        upserts: [
          { entity: 'tasks', id: 'task_1', position: 5, row: task(1) },
          { entity: 'tasks', id: 'task_2', position: 9, row: task(2, 'newer') },
        ],
        releases: [],
        // The load completed, so the delta names everything the op now holds.
        members: [
          { entity: 'tasks', id: 'task_1' },
          { entity: 'tasks', id: 'task_2' },
        ],
      },
      { reset: false, upserts: [], releases: [{ entity: 'tasks', id: 'task_1', removed: true }] },
    ]);
  });
});
