import { afterEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_LIVE_PACE_MS, LiveHub, createPacer, effectivePaceMs, type LiveStatus } from '../src/live.js';
import { useOrdoRuntime } from '../src/hooks/context.js';
import { useOpPages } from '../src/hooks/useOpPages.js';
import { useOpQuery } from '../src/hooks/useOpQuery.js';
import { LiveDoor, mount, settle, until, type Mounted } from './live-door.js';

const rowsOf = (...titles: string[]) => ({
  body: { rows: titles.map((title, i) => ({ id: `task_${i + 1}`, title })) },
});
const TASKS = 'std.tasks.list';
const BOARDS = 'std.boards.list';
const title = (r: unknown) => (r as { title: string }).title;

let mounted: Mounted<unknown> | null = null;
afterEach(() => {
  mounted?.unmount();
  mounted = null;
});

describe('live: the hub and the stream it shares', () => {
  it('reopens the stream when the door stops answering for it, and adds every read again', async () => {
    // A stream belongs to the token that opened it. After the client's token
    // is replaced, the door answers `not_found` to an add on the old stream
    // while the old feed stays open. Before this the hub waited on that feed
    // forever and the new read never opened.
    const door = new LiveDoor();
    const hub = new LiveHub(door.client(), { backoff: () => 0 });
    const statuses = new Map<string, LiveStatus[]>();
    const subscriber = (name: string) => ({
      invalidate: () => undefined,
      status: (s: LiveStatus) => {
        statuses.set(name, [...(statuses.get(name) ?? []), s]);
      },
    });

    hub.subscribe(TASKS, {}, subscriber('tasks'));
    await until(() => door.live.length === 1, 'the stream to open');
    door.ready();
    await until(() => door.readsOf(TASKS).length === 1, 'the first read');
    expect(statuses.get('tasks')?.at(-1)).toEqual({ state: 'open' });

    door.forget(door.live[0]!.id);
    hub.subscribe(BOARDS, {}, subscriber('boards'));
    await until(() => door.live.length === 2, 'a second stream, under the current token');
    expect(door.live[0]!.done).toBe(true);
    door.ready();
    await until(() => door.readsOf(TASKS).length === 1 && door.readsOf(BOARDS).length === 1, 'both reads on it');
    expect(statuses.get('boards')?.at(-1)).toEqual({ state: 'open' });
    expect(statuses.get('tasks')?.at(-1)).toEqual({ state: 'open' });
    expect(hub.connected).toBe(true);
  });

  it('an error frame with a renewable token reconnects at once with the new one', async () => {
    let n = 0;
    const door = new LiveDoor();
    const token = { current: 'tok-1' };
    const client = door.client({
      token: async () => token.current,
      renewToken: async () => {
        n += 1;
        token.current = `tok-${n + 1}`;
        return true;
      },
    });
    const wait = vi.fn(async () => undefined);
    const hub = new LiveHub(client, { backoff: () => 60_000, wait });
    const seen: LiveStatus[] = [];
    hub.subscribe(TASKS, {}, { invalidate: () => undefined, status: (s) => seen.push(s) });
    await until(() => door.live.length === 1, 'the stream to open');
    door.ready();
    await until(() => door.readsOf(TASKS).length === 1, 'the read');
    door.refuse('access_denied', door.readsOf(TASKS)[0]!.key);
    await until(() => door.live.length === 2, 'a second stream');
    expect(n).toBe(1);
    expect(door.liveHeaders.map((h) => h['Authorization'])).toEqual(['Bearer tok-1', 'Bearer tok-2']);
    expect(wait).not.toHaveBeenCalledWith(60_000, expect.anything());
    door.ready();
    await until(() => door.readsOf(TASKS).length === 1, 'the read again');
    expect(seen.at(-1)).toEqual({ state: 'open' });
    // A read accepted under the new token clears the once-only rule: a later refusal asks again.
    door.refuse('access_denied');
    await until(() => door.live.length === 3, 'a third stream');
    expect(n).toBe(2);
  });

  it('an error frame renewal cannot fix is surfaced and not reconnected', async () => {
    const door = new LiveDoor();
    const renewToken = vi.fn(async () => false);
    const hub = new LiveHub(door.client({ renewToken }), { backoff: () => 0 });
    const seen: LiveStatus[] = [];
    hub.subscribe(TASKS, {}, { invalidate: () => undefined, status: (s) => seen.push(s) });
    await until(() => door.live.length === 1, 'the stream to open');
    door.ready();
    await until(() => door.readsOf(TASKS).length === 1, 'the read');
    door.refuse('access_denied', door.readsOf(TASKS)[0]!.key);
    await until(() => seen.at(-1)?.state === 'failed', 'the failure');
    const last = seen.at(-1);
    expect(last?.state === 'failed' && last.refusal.class).toBe('access_denied');
    expect(renewToken).toHaveBeenCalledTimes(1);
    await settle(20);
    expect(door.live.length).toBe(1);
    expect(hub.connected).toBe(false);
  });

  it('a network drop with no error frame still reconnects as before', async () => {
    const door = new LiveDoor();
    const renewToken = vi.fn(async () => true);
    const hub = new LiveHub(door.client({ renewToken }), { backoff: () => 0 });
    const seen: LiveStatus[] = [];
    hub.subscribe(TASKS, {}, { invalidate: () => undefined, status: (s) => seen.push(s) });
    await until(() => door.live.length === 1, 'the stream to open');
    door.ready();
    await until(() => door.readsOf(TASKS).length === 1, 'the read');
    door.live[0]!.close();
    await until(() => door.live.length === 2, 'a second stream');
    door.ready();
    await until(() => seen.at(-1)?.state === 'open' && door.readsOf(TASKS).length === 1, 'open again');
    expect(renewToken).not.toHaveBeenCalled();
  });

  it('a read refused server_busy is asked again, not failed (fix 25z)', async () => {
    const door = new LiveDoor().refuseRead(TASKS, 503, 'server_busy');
    const hub = new LiveHub(door.client(), { backoff: () => 0 });
    const seen: LiveStatus[] = [];
    hub.subscribe(TASKS, {}, { invalidate: () => undefined, status: (s) => seen.push(s) });
    await until(() => door.live.length === 1, 'the stream to open');
    door.ready();
    await until(() => seen.at(-1)?.state === 'open', 'the read, on the second try');
    expect(seen.some((s) => s.state === 'reconnecting')).toBe(true);
    expect(seen.some((s) => s.state === 'failed')).toBe(false);
  });

  it('a read refused tenant_not_ready is asked again, not failed (audit bug 3)', async () => {
    const door = new LiveDoor().refuseRead(TASKS, 503, 'tenant_not_ready');
    const hub = new LiveHub(door.client(), { backoff: () => 0 });
    const seen: LiveStatus[] = [];
    hub.subscribe(TASKS, {}, { invalidate: () => undefined, status: (s) => seen.push(s) });
    await until(() => door.live.length === 1, 'the stream to open');
    door.ready();
    await until(() => seen.at(-1)?.state === 'open', 'the read, on the second try');
    expect(seen.some((s) => s.state === 'failed')).toBe(false);
  });
});

describe('live: a read hook follows the feed', () => {
  it('refetches on an invalidate event and not otherwise', async () => {
    const door = new LiveDoor().reply(rowsOf('one'), rowsOf('one'), rowsOf('one', 'two'));
    const m = await mount(() => useOpQuery(TASKS), {
      client: door.client(),
      live: { paceMs: 0 },
    });
    mounted = m;

    // Since I8 the hook fetches at once, beside the subscribe, and fetches
    // once more when its read opens (the first fetch began before the open).
    await until(() => door.live.length === 1, 'the live stream to open');
    expect(new URL(door.live[0]!.url).pathname).toBe('/live');
    await until(() => door.calls.length === 1, 'the first fetch');

    door.ready();
    await until(() => door.readsOf(TASKS).length === 1, 'the read to be added');
    await until(() => door.calls.length === 2, 'the fetch on open');
    await until(() => m.result.current.rows.length === 1, 'the first rows');
    await until(() => m.result.current.live?.state === 'open');
    expect(m.result.current.live).toEqual({ state: 'open' });
    // The read rides with the client's space and the op's version pin.
    expect(door.readsOf(TASKS)[0]!.headers['X-Ordo-Space']).toBe('spc_test');

    // Not otherwise: a heartbeat, and time passing.
    door.live[0]!.push(':hb\n\n');
    await settle(20);
    expect(door.calls).toHaveLength(2);

    door.invalidate(TASKS);
    await until(() => door.calls.length === 3, 'the refetch');
    await until(() => m.result.current.rows.length === 2, 'the new rows');
    // A refetch of what is on screen is not a loading state.
    expect(m.result.current.loading).toBe(false);
  });

  it('is off unless the provider or the hook turns it on', async () => {
    const door = new LiveDoor().reply(rowsOf('one'));
    const m = await mount(() => useOpQuery(TASKS), { client: door.client() });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'rows');
    expect(door.live).toHaveLength(0);
    expect(m.result.current.live).toBeNull();
  });

  it('a hook can turn it on for itself, and off under a provider that has it on', async () => {
    const door = new LiveDoor().reply(rowsOf('one'));
    const on = await mount(() => useOpQuery(TASKS, { live: true }), { client: door.client() });
    await until(() => door.live.length === 1, 'a stream for the opted-in hook');
    on.unmount();

    const off = await mount(() => useOpQuery(TASKS, { live: false }), {
      client: door.client(),
      live: true,
    });
    mounted = off;
    await until(() => off.result.current.rows.length === 1, 'rows');
    expect(door.live).toHaveLength(1);
  });

  it('a burst of invalidations is one refetch', async () => {
    const door = new LiveDoor().reply(rowsOf('one'), rowsOf('two'));
    const m = await mount(() => useOpQuery(TASKS), {
      client: door.client(),
      live: { paceMs: 30 },
    });
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready();
    await until(() => door.calls.length === 1);
    door.invalidate(TASKS, 0, 3);
    await settle(80);
    expect(door.calls).toHaveLength(2);
  });

  it("the provider's pace is a floor: a hook asking for less still gets one refetch per burst", async () => {
    const door = new LiveDoor().reply(rowsOf('one'), rowsOf('two'));
    const m = await mount(() => useOpQuery(TASKS, { livePaceMs: 0 }), {
      client: door.client(),
      live: { paceMs: 30 },
    });
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready();
    await until(() => door.calls.length === 1);
    door.invalidate(TASKS, 0, 3);
    await settle(80);
    expect(door.calls).toHaveLength(2);
  });

  it('a stream that reopens adds its reads again and refetches, because signals may have been missed', async () => {
    const door = new LiveDoor().reply(rowsOf('one'), rowsOf('two'));
    const m = await mount(() => useOpQuery(TASKS), {
      client: door.client(),
      live: { paceMs: 0, backoff: () => 0 },
    });
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready(0);
    await until(() => door.calls.length === 1);

    door.live[0]!.close();
    await until(() => door.live.length === 2, 'the stream to reopen');
    door.ready(1);
    await until(() => door.readsOf(TASKS).length === 1, 'the read to be added to the new stream');
    await until(() => door.calls.length === 2, 'the refetch after reconnect');
  });

  it('a stream is closed at the door when its last hook unmounts', async () => {
    const door = new LiveDoor().reply(rowsOf('one'));
    const a = await mount(() => useOpQuery(TASKS), { client: door.client(), live: true });
    await until(() => door.live.length === 1);
    a.unmount();
    await until(() => door.live.every((s) => s.done), 'the stream to close');
  });

  it('a read the door refuses does not stall the read', async () => {
    const door = new LiveDoor().reply(rowsOf('one')).refuseRead(TASKS, 400, 'not_live');
    const m = await mount(() => useOpQuery(TASKS), { client: door.client(), live: true });
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready();
    await until(() => m.result.current.rows.length === 1, 'rows despite the refusal');
    await until(() => m.result.current.live?.state === 'unsupported', 'the refusal');
  });

  it('a stream the door refuses does not stall the read', async () => {
    const door = new LiveDoor().reply(rowsOf('one')).refuseNextStream(403, 'access_denied');
    const m = await mount(() => useOpQuery(TASKS), { client: door.client(), live: true });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'rows despite the refusal');
    expect(m.result.current.live?.state).toBe('failed');
  });

  it('an op that is not live-capable is never subscribed', async () => {
    // `std.tasks.create` is a write: `call.live` is false and the door would
    // answer `not_live`, so the hook must not open a stream for it.
    const door = new LiveDoor().reply(rowsOf('one'));
    const m = await mount(() => useOpQuery('std.tasks.create', { enabled: false, live: true }), {
      client: door.client(),
    });
    mounted = m;
    await settle(10);
    expect(door.live).toHaveLength(0);
    expect(m.result.current.live).toBeNull();
  });

  it('useOpPages reloads the pages on screen on an invalidate', async () => {
    const door = new LiveDoor().reply(
      { body: { rows: [{ id: 'task_1', title: 'a' }], next: 'c1' } },
      { body: { rows: [{ id: 'task_2', title: 'b' }] } },
      // After the invalidate: the same two pages, read again from the top.
      { body: { rows: [{ id: 'task_1', title: 'a2' }], next: 'c1' } },
      { body: { rows: [{ id: 'task_2', title: 'b2' }] } },
    );
    const m = await mount(() => useOpPages(TASKS), { client: door.client(), live: { paceMs: 0 } });
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready();
    await until(() => m.result.current.rows.length === 1);
    m.result.current.more();
    await until(() => m.result.current.rows.length === 2);

    door.invalidate(TASKS);
    await until(() => door.calls.length === 4, 'both pages to be read again');
    await until(() => title(m.result.current.rows[0]) === 'a2');
    expect(m.result.current.rows.map(title)).toEqual(['a2', 'b2']);
    expect(m.result.current.hasMore).toBe(false);
  });
});

/** Two read hooks under one provider: `tasks` and `boards`. */
function both(options: { tasks?: { livePaceMs?: number }; boards?: { livePaceMs?: number } } = {}) {
  return () => ({
    tasks: useOpQuery(TASKS, options.tasks ?? {}),
    boards: useOpQuery(BOARDS, options.boards ?? {}),
  });
}

const callsTo = (door: LiveDoor, op: string) => door.calls.filter((c) => c.url.includes(`/op/${op}`)).length;

describe('live: one stream per client (27s)', () => {
  it('two hooks on two reads share one stream, and each hears only its own signal', async () => {
    const door = new LiveDoor();
    const m = await mount(both(), { client: door.client(), live: { paceMs: 0 } });
    mounted = m;
    await until(() => door.live.length === 1, 'the stream to open');
    door.ready();
    await until(() => door.readsOf(TASKS).length === 1 && door.readsOf(BOARDS).length === 1, 'both reads');
    // Each read fetched once at mount and once more on its open.
    await until(() => callsTo(door, TASKS) === 2 && callsTo(door, BOARDS) === 2, 'both first fetches');
    // One stream, two reads under two keys.
    expect(door.live).toHaveLength(1);
    expect(door.readsOf(TASKS)[0]!.key).not.toBe(door.readsOf(BOARDS)[0]!.key);

    door.invalidate(TASKS);
    await until(() => callsTo(door, TASKS) === 3, 'the tasks refetch');
    await settle(20);
    expect(callsTo(door, BOARDS)).toBe(2);

    door.invalidate(BOARDS);
    await until(() => callsTo(door, BOARDS) === 3, 'the boards refetch');
    await settle(20);
    expect(callsTo(door, TASKS)).toBe(3);
  });

  it('two hooks on the same read share one read on the stream', async () => {
    const door = new LiveDoor();
    const m = await mount(
      () => ({ a: useOpQuery(TASKS), b: useOpQuery(TASKS) }),
      { client: door.client(), live: { paceMs: 0 } },
    );
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready();
    await until(() => m.result.current.a.live?.state === 'open' && m.result.current.b.live?.state === 'open');
    expect(door.adds).toHaveLength(1);
  });

  it('a refused read leaves the other hooks following', async () => {
    const door = new LiveDoor().refuseRead(BOARDS, 403, 'access_denied');
    const m = await mount(both(), { client: door.client(), live: { paceMs: 0 } });
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready();
    await until(() => m.result.current.boards.live?.state === 'failed', 'the refusal');
    await until(() => callsTo(door, TASKS) === 2);
    await until(() => m.result.current.tasks.live?.state === 'open');
    expect(m.result.current.tasks.live).toEqual({ state: 'open' });
    expect(door.live[0]!.done).toBe(false);

    door.invalidate(TASKS);
    await until(() => callsTo(door, TASKS) === 3, 'the tasks refetch');
  });

  it('the last hook on a read leaving drops the read; the stream stays for the rest', async () => {
    const door = new LiveDoor();
    let showBoards = true;
    const m = await mount(
      () => ({
        tasks: useOpQuery(TASKS),
        boards: useOpQuery(BOARDS, { enabled: showBoards }),
      }),
      { client: door.client(), live: { paceMs: 0 } },
    );
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready();
    await until(() => door.readsOf(BOARDS).length === 1 && door.readsOf(TASKS).length === 1);

    // A disabled hook stops following, which is the same as leaving.
    showBoards = false;
    door.invalidate(TASKS);
    await until(() => door.readsOf(BOARDS).length === 0, 'the boards read to be dropped');
    expect(door.readsOf(TASKS)).toHaveLength(1);
    expect(door.live[0]!.done).toBe(false);
  });

  it('pacing stays per hook on a shared stream', async () => {
    const door = new LiveDoor();
    const m = await mount(both({ boards: { livePaceMs: 1000 } }), {
      client: door.client(),
      live: { paceMs: 0 },
    });
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready();
    // Each read fetched at mount and once more on its open.
    await until(() => callsTo(door, TASKS) === 2 && callsTo(door, BOARDS) === 2);

    // A first signal each: both refetch at once (the trailing edge of an
    // idle pacer fires straight away).
    door.invalidate(TASKS);
    door.invalidate(BOARDS);
    await until(() => callsTo(door, TASKS) === 3 && callsTo(door, BOARDS) === 3);
    // A second signal each: tasks refetches now, boards waits out its gap.
    door.invalidate(TASKS);
    door.invalidate(BOARDS);
    await until(() => callsTo(door, TASKS) === 4, 'the tasks refetch');
    await settle(50);
    expect(callsTo(door, BOARDS)).toBe(3);
  });
});

describe('live with a refinement', () => {
  const refineOf = (url: string): unknown => JSON.parse(new URL(url).searchParams.get('$refine') ?? 'null');

  it('two hooks on one op with different refinements are two reads, one signal, never one result', async () => {
    const door = new LiveDoor();
    let version = 1;
    // The door answers from the refinement it is sent.
    door.answer = (url) => {
      const status = (JSON.parse(url.searchParams.get('$refine') ?? '{}') as { where?: { value: string }[] })
        .where?.[0]?.value;
      return { rows: [{ id: `task_${status}`, title: `${status} v${version}`, status }] };
    };
    const m = await mount(
      () => ({
        open: useOpQuery(TASKS, { refine: { where: [{ field: 'status', op: 'eq', value: 'open' }] } }),
        done: useOpQuery(TASKS, { refine: (v) => v.status!.eq('done') }),
      }),
      { client: door.client(), live: { paceMs: 0 } },
    );
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready();
    await until(() => door.calls.length === 2, 'each hook fetches with its own refinement');
    await until(() => m.result.current.open.rows.length === 1 && m.result.current.done.rows.length === 1);
    expect(title(m.result.current.open.rows[0])).toBe('open v1');
    expect(title(m.result.current.done.rows[0])).toBe('done v1');
    // Each refinement rides with its own read on the stream (R6), so the door
    // checks it. Invalidation stays per op: a refinement changes which rows
    // an answer shows, never which writes can change it, so one write
    // signals both reads.
    await until(() => door.readsOf(TASKS).length === 2, 'one read per refinement');
    expect(door.readsOf(TASKS).map((r) => r.refine)).toEqual([
      { where: [{ field: 'status', op: 'eq', value: 'open' }] },
      { where: [{ field: 'status', op: 'eq', value: 'done' }] },
    ]);

    version = 2;
    door.invalidate(TASKS);
    await until(() => door.calls.length === 4, 'both hooks refetch');
    await until(() => title(m.result.current.done.rows[0]) === 'done v2' && title(m.result.current.open.rows[0]) === 'open v2');
    const sent = door.calls.slice(2).map((c) => refineOf(c.url));
    expect(sent).toContainEqual({ where: [{ field: 'status', op: 'eq', value: 'open' }] });
    expect(sent).toContainEqual({ where: [{ field: 'status', op: 'eq', value: 'done' }] });
  });

  it('a new callback each render is not a new refinement', async () => {
    const door = new LiveDoor().reply(rowsOf('one'));
    const m = await mount(() => useOpQuery(TASKS, { refine: (v) => [v.title!.ne('x')], pick: ['id', 'title'] }), {
      client: door.client(),
    });
    mounted = m;
    await until(() => m.result.current.rows.length === 1);
    await m.rerender();
    await m.rerender();
    await settle(20);
    expect(door.calls).toHaveLength(1);
    expect(refineOf(door.calls[0]!.url)).toEqual({
      where: [{ field: 'title', op: 'ne', value: 'x' }],
      pick: ['id', 'title'],
    });
  });

  it('a refined page reload fetches the same pages, each with the refinement', async () => {
    const door = new LiveDoor().reply(
      { body: { rows: [{ id: 'task_1', title: 'a' }], next: 'c1' } },
      { body: { rows: [{ id: 'task_2', title: 'b' }] } },
      { body: { rows: [{ id: 'task_1', title: 'a2' }], next: 'c1' } },
      { body: { rows: [{ id: 'task_2', title: 'b2' }] } },
    );
    const m = await mount(
      () => useOpPages(TASKS, { refine: (v) => v.status!.eq('open'), pick: ['id', 'title'] }),
      { client: door.client(), live: { paceMs: 0 } },
    );
    mounted = m;
    await until(() => door.live.length === 1);
    door.ready();
    await until(() => m.result.current.rows.length === 1);
    m.result.current.more();
    await until(() => m.result.current.rows.length === 2);

    door.invalidate(TASKS);
    await until(() => door.calls.length === 4, 'both pages to be read again');
    await until(() => title(m.result.current.rows[0]) === 'a2');
    const want = { where: [{ field: 'status', op: 'eq', value: 'open' }], pick: ['id', 'title'] };
    for (const c of door.calls) expect(refineOf(c.url)).toEqual(want);
  });
});

describe('revocation on the network path', () => {
  it('a revoking refusal empties the hook, not just sets the refusal', async () => {
    const door = new LiveDoor().reply(rowsOf('one'), {
      status: 403,
      body: { error: { code: 'access_denied', message: 'no' } },
    });
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: door.client() });
    mounted = m;
    await until(() => m.result.current.rows.length === 1);
    m.result.current.refetch();
    await until(() => m.result.current.refusal !== null);
    expect(m.result.current.refusal?.class).toBe('access_denied');
    expect(m.result.current.rows).toEqual([]);
  });
});

describe('pacing', () => {
  it('spaces fires at least the interval apart, and absorbs asks while one is pending', () => {
    vi.useFakeTimers();
    try {
      const fire = vi.fn();
      const pacer = createPacer(fire, 100, {
        now: () => Date.now(),
        set: (fn, ms) => setTimeout(fn, ms),
        clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
      });
      pacer.request();
      pacer.request();
      vi.advanceTimersByTime(0);
      expect(fire).toHaveBeenCalledTimes(1);
      pacer.request();
      vi.advanceTimersByTime(99);
      expect(fire).toHaveBeenCalledTimes(1);
      vi.advanceTimersByTime(1);
      expect(fire).toHaveBeenCalledTimes(2);
      pacer.request();
      pacer.cancel();
      vi.advanceTimersByTime(1000);
      expect(fire).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  /**
   * A door that signals every 10 ms for `forMs`, against a pacer at `paceMs`.
   * Returns how many refetches that produced.
   */
  function refetchesUnder(paceMs: number, forMs: number): number {
    vi.useFakeTimers();
    try {
      const fire = vi.fn();
      const pacer = createPacer(fire, paceMs, {
        now: () => Date.now(),
        set: (fn, ms) => setTimeout(fn, ms),
        clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
      });
      for (let t = 0; t < forMs; t += 10) {
        pacer.request();
        vi.advanceTimersByTime(10);
      }
      return fire.mock.calls.length;
    } finally {
      vi.useRealTimers();
    }
  }

  it('a steady burst of invalidations is one refetch per gap (27n: at least a quarter second)', () => {
    expect(DEFAULT_LIVE_PACE_MS).toBe(250);
    // 2 s of signals every 10 ms: a fire at 0, then one every 250 ms.
    expect(refetchesUnder(DEFAULT_LIVE_PACE_MS, 2000)).toBe(1 + 2000 / 250);
  });

  it('the setting changes the gap', () => {
    expect(refetchesUnder(100, 2000)).toBe(1 + 2000 / 100);
    expect(refetchesUnder(500, 2000)).toBe(1 + 2000 / 500);
  });

  it("the default applies when the app sets nothing, and the app's value when it does", async () => {
    const door = new LiveDoor();
    const on = await mount(() => useOrdoRuntime().paceMs, { client: door.client(), live: true });
    expect(on.result.current).toBe(DEFAULT_LIVE_PACE_MS);
    on.unmount();

    const config = await mount(() => useOrdoRuntime().paceMs, { client: door.client(), live: {} });
    expect(config.result.current).toBe(DEFAULT_LIVE_PACE_MS);
    config.unmount();

    const set = await mount(() => useOrdoRuntime().paceMs, { client: door.client(), live: { paceMs: 40 } });
    expect(set.result.current).toBe(40);
    set.unmount();
  });

  it("a hook may wait longer than the provider's pace, never less", () => {
    expect(effectivePaceMs(250, undefined)).toBe(250);
    expect(effectivePaceMs(250, 1000)).toBe(1000);
    expect(effectivePaceMs(250, 0)).toBe(250);
    expect(effectivePaceMs(30, 10)).toBe(30);
  });
});

describe('a refinement on the stream (R6)', () => {
  const open = { where: [{ field: 'status', op: 'eq' as const, value: 'open' }] };

  it('the hub adds a refined read with its refinement, and an unrefined one apart', async () => {
    const door = new LiveDoor();
    const hub = new LiveHub(door.client(), { backoff: () => 0 });
    const quiet = { invalidate: () => undefined, status: () => undefined };
    const stopA = hub.subscribe(TASKS, {}, quiet, open);
    const stopB = hub.subscribe(TASKS, {}, quiet);
    // The same refinement again is the same read.
    const stopC = hub.subscribe(TASKS, {}, quiet, { where: [{ field: 'status', op: 'eq', value: 'open' }] });
    await until(() => door.live.length === 1);
    door.ready();
    await until(() => door.readsOf(TASKS).length === 2, 'two reads');
    expect(hub.size).toBe(2);
    expect(door.readsOf(TASKS).map((r) => r.refine ?? null)).toEqual([open, null]);
    stopA();
    stopB();
    stopC();
  });

  it('both kinds of stream are opened past the browser cache', async () => {
    // Notes e2e, Firefox: a stream the HTTP cache writes holds the entry for
    // its URL, and the next request for that URL (another tab) waits on it.
    const door = new LiveDoor();
    const controller = new AbortController();
    await door.client().openLive(TASKS, { signal: controller.signal });
    await door.client().openLiveStream({ signal: controller.signal });
    expect(door.single[0]!.cache).toBe('no-store');
    expect(door.live[0]!.cache).toBe('no-store');
    controller.abort();
  });

  it('the single-read stream sends the refinement as $refine', async () => {
    const door = new LiveDoor();
    const controller = new AbortController();
    await door.client().openLive(TASKS, { refine: open, signal: controller.signal });
    expect(door.single).toHaveLength(1);
    const sent = new URL(door.single[0]!.url, 'http://door.test').searchParams.get('$refine');
    expect(JSON.parse(sent!)).toEqual(open);
    controller.abort();
  });
});

describe('live: a read does not wait for its stream (M3)', () => {
  it('a live read fetches before its stream is ready', async () => {
    const door = new LiveDoor().reply(rowsOf('one'));
    const m = await mount(() => useOpQuery(TASKS), { client: door.client(), live: { paceMs: 0 } });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'rows with the stream still opening');
    expect(door.live).toHaveLength(1);
    expect(door.reads).toHaveLength(0);
    expect(m.result.current.loading).toBe(false);
    expect(m.result.current.live).toEqual({ state: 'connecting' });
  });

  it('a stream that never says ready still shows rows, then reconnects after the deadline', async () => {
    const door = new LiveDoor().reply(rowsOf('one'));
    const m = await mount(() => useOpQuery(TASKS), {
      client: door.client(),
      live: { paceMs: 0, connectMs: 40, backoff: () => 5 },
    });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'rows');
    expect(m.result.current.live).toEqual({ state: 'connecting' });
    await until(() => m.result.current.live?.state === 'reconnecting', 'the deadline');
    expect(door.live[0]!.done).toBe(true);
    // The next try opens a stream, and this time it says ready.
    await until(() => door.live.length === 2, 'a second stream');
    door.ready();
    await until(() => m.result.current.live?.state === 'open', 'the read open');
    // First open after the rows were fetched: one more fetch.
    await until(() => door.calls.length === 2, 'the fetch on open');
  });

  it('a change between the first fetch and the open is shown', async () => {
    const door = new LiveDoor().reply(rowsOf('one'), rowsOf('one', 'two'));
    const m = await mount(() => useOpQuery(TASKS), { client: door.client(), live: { paceMs: 0 } });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'the first rows');
    // Someone wrote before the read was on the stream; no signal will come.
    door.ready();
    await until(() => m.result.current.rows.length === 2, 'the change made before the open');
    expect(door.calls).toHaveLength(2);
  });

  it('useOpPages fetches its first page before the stream is ready, and again on open', async () => {
    const door = new LiveDoor().reply(rowsOf('a'), rowsOf('a', 'b'));
    const m = await mount(() => useOpPages(TASKS), { client: door.client(), live: { paceMs: 0 } });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'the first page with the stream opening');
    expect(door.reads).toHaveLength(0);
    door.ready();
    await until(() => m.result.current.rows.length === 2, 'the pages read again on open');
    expect(door.calls).toHaveLength(2);
  });
});
