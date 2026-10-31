import { describe, expect, it } from 'vitest';
import { advance, collectAll, emptyPageState, nextPage, sameRows } from '../src/pages.js';
import { MockDoor, client, refusal } from './harness.js';

const page = (rows: unknown[], next?: string) => ({
  body: next === undefined ? { rows } : { rows, next },
});

describe('cursor threading', () => {
  it('sends no cursor on page one, then the previous page’s `next`', async () => {
    const door = new MockDoor().program(
      page([{ id: 'task_1' }], 'cur-1'),
      page([{ id: 'task_2' }], 'cur-2'),
      page([{ id: 'task_3' }]),
    );
    const step = await collectAll(client(door), 'std.tasks.list');

    expect(door.requests.map((r) => r.headers['X-Ordo-Page'])).toEqual([undefined, 'cur-1', 'cur-2']);
    expect(step.status).toBe('page');
    if (step.status !== 'page') return;
    expect(step.state.rows).toHaveLength(3);
    expect(step.state.pagesFetched).toBe(3);
    expect(step.state.hasMore).toBe(false);
  });

  it('threads params alongside the cursor on every page', async () => {
    const door = new MockDoor().program(page([], 'cur-1'), page([]));
    await collectAll(client(door), 'openTasks', { params: { status: 'open' } });
    for (const r of door.requests) expect(r.query.get('status')).toBe('open');
  });

  it('threads a refinement alongside the cursor on every page', async () => {
    const door = new MockDoor().program(page([{ id: 'task_1' }], 'cur-1'), page([], 'cur-2'), page([]));
    const refine = { where: [{ field: 'status', op: 'eq' as const, value: 'open' }], pick: ['id'] };
    await collectAll(client(door), 'std.tasks.list', { refine });
    expect(door.requests).toHaveLength(3);
    for (const r of door.requests) {
      expect(JSON.parse(r.query.get('$refine') ?? 'null')).toEqual(refine);
    }
    expect(door.requests.map((r) => r.headers['X-Ordo-Page'])).toEqual([undefined, 'cur-1', 'cur-2']);
  });
});

describe('only the absence of `next` ends a listing (§7)', () => {
  it('keeps paging through empty pages', async () => {
    // A narrow op can return short and even empty pages with `next` still
    // set; the door is allowed to. A runtime that stopped here
    // would silently lose every row after the gap.
    const door = new MockDoor().program(
      page([], 'cur-1'),
      page([], 'cur-2'),
      page([{ id: 'task_9' }], 'cur-3'),
      page([]),
    );
    const step = await collectAll(client(door), 'std.tasks.list');
    expect(door.requests).toHaveLength(4);
    if (step.status !== 'page') throw new Error(step.status);
    expect(step.state.rows).toEqual([{ id: 'task_9' }]);
    expect(step.state.empty).toBe(false);
  });

  it('an empty page with `next` still present is not the empty state', () => {
    const mid = advance(emptyPageState(), {
      op: 'std.tasks.list',
      version: 'v',
      rows: [],
      next: 'cur-1',
      servedVersion: null,
      stale: false,
      replayed: false,
      pin: null,
    });
    expect(mid.empty).toBe(true);
    expect(mid.hasMore).toBe(true);
    // The empty state a UI renders is `empty && !hasMore` — a fact about the
    // whole iteration, never about one page.
    expect(mid.empty && !mid.hasMore).toBe(false);
  });

  it('advance appends in place and bumps the version on every page', () => {
    const resp = (rows: Array<{ id: string }>, next?: string) => ({
      op: 'std.tasks.list',
      version: 'v',
      rows,
      ...(next ? { next } : {}),
      servedVersion: null,
      stale: false,
      replayed: false,
      pin: null,
    });
    const s0 = emptyPageState<{ id: string }>();
    const s1 = advance(s0, resp([{ id: 'a' }], 'c1'));
    const s2 = advance(s1, resp([{ id: 'b' }], 'c2'));
    const s3 = advance(s2, resp([{ id: 'c' }]));
    // One array for the whole scroll: no page copies the rows before it.
    expect(s2.rows).toBe(s1.rows);
    expect(s3.rows).toBe(s1.rows);
    // React still sees every page: a new state object and a new version.
    expect([s1.version, s2.version, s3.version]).toEqual([1, 2, 3]);
    expect(s2).not.toBe(s1);
    expect(s3.rows.map((r) => r.id)).toEqual(['a', 'b', 'c']);
    expect(s3.pagesFetched).toBe(3);
  });

  it('advancing an older state copies, so two branches never share rows', () => {
    const resp = (id: string) => ({
      op: 'o',
      version: 'v',
      rows: [{ id }],
      servedVersion: null,
      stale: false,
      replayed: false,
      pin: null,
    });
    const s1 = advance(emptyPageState<{ id: string }>(), resp('a'));
    const s2 = advance(s1, resp('b'));
    const branch = advance(s1, resp('z'));
    expect(branch.rows).not.toBe(s2.rows);
    expect(s2.rows.map((r) => r.id)).toEqual(['a', 'b']);
    expect(branch.rows.map((r) => r.id)).toEqual(['a', 'z']);
  });

  it('the whole iteration producing nothing is the empty state', async () => {
    const door = new MockDoor().program(page([], 'cur-1'), page([]));
    const step = await collectAll(client(door), 'std.tasks.list');
    if (step.status !== 'page') throw new Error(step.status);
    expect(step.state.empty && !step.state.hasMore).toBe(true);
  });

  it('an empty-string `next` terminates, rather than re-fetching page one forever', async () => {
    const door = new MockDoor().program({ body: { rows: [{ id: 'a' }], next: '' } });
    const step = await collectAll(client(door), 'std.tasks.list');
    expect(door.requests).toHaveLength(1);
    if (step.status !== 'page') throw new Error(step.status);
    expect(step.state.hasMore).toBe(false);
  });

  it('refuses to advance past the end rather than restarting the listing', async () => {
    const door = new MockDoor().program(page([{ id: 'a' }]));
    const c = client(door);
    const first = await nextPage(c, 'std.tasks.list', emptyPageState());
    if (first.status !== 'page') throw new Error(first.status);
    expect(first.state.hasMore).toBe(false);
    // Calling again with a null cursor would silently re-fetch page one and
    // append every row a second time.
    const again = await nextPage(c, 'std.tasks.list', first.state);
    expect(door.requests).toHaveLength(2);
    expect(again.status).toBe('page');
  });
});

describe('a stale cursor is a restart, not a data conflict', () => {
  it('version_skew drops the cursor and asks for page one', async () => {
    // §8: keyset columns may no longer mean the same thing, so the client
    // restarts from page one and the token is not migrated. Nothing about
    // the *rows* disagreed, which is why this must not look like a cas_miss.
    const door = new MockDoor().program(page([{ id: 'a' }], 'cur-1'), {
      status: 409,
      body: refusal('version_skew', 'page token minted under a different op version'),
    });
    const step = await collectAll(client(door), 'std.tasks.list');

    expect(step.status).toBe('restart');
    if (step.status !== 'restart') return;
    expect(step.refusal.class).toBe('version_skew');
    // The state handed back is a fresh one: continuing from the old cursor is
    // exactly what the server just refused.
    expect(step.state.cursor).toBeNull();
    expect(step.state.rows).toEqual([]);
    expect(step.state.hasMore).toBe(true);
  });

  it('invalid_cursor restarts too — a forged or rotated token, same repair', async () => {
    const door = new MockDoor().program({ status: 400, body: refusal('invalid_cursor', 'undecodable') });
    const step = await collectAll(client(door), 'std.tasks.list');
    expect(step.status).toBe('restart');
  });

  it('any other refusal keeps the accumulated rows and surfaces typed', async () => {
    const door = new MockDoor().program(page([{ id: 'a' }], 'cur-1'), {
      status: 403,
      body: refusal('access_denied', 'no'),
    });
    const step = await collectAll(client(door), 'std.tasks.list');
    expect(step.status).toBe('refused');
    if (step.status !== 'refused') return;
    expect(step.refusal.class).toBe('access_denied');
    expect(step.state.rows).toEqual([{ id: 'a' }]);
  });
});

describe('runaway guard', () => {
  it('maxPages stops an unbounded loop without pretending the listing ended', async () => {
    const door = new MockDoor().always(page([{ id: 'x' }], 'forever'));
    const step = await collectAll(client(door), 'std.tasks.list', { maxPages: 5 });
    expect(door.requests).toHaveLength(5);
    if (step.status !== 'page') throw new Error(step.status);
    // `hasMore` stays true: the runtime stopped, the listing did not end.
    expect(step.state.hasMore).toBe(true);
  });
});

describe('the client’s own page generator', () => {
  it('yields each page and stops on a missing `next`', async () => {
    const door = new MockDoor().program(page([{ n: 1 }], 'c1'), page([], 'c2'), page([{ n: 2 }]));
    const seen: number[] = [];
    for await (const res of client(door).pageAll<{ n: number }>('std.tasks.list')) {
      seen.push(res.rows.length);
    }
    expect(seen).toEqual([1, 0, 1]);
  });
});

describe('a long walk (M4, L4)', () => {
  const pages = (n: number) =>
    Array.from({ length: n }, (_, i) => page([{ id: `task_${i}` }], i < n - 1 ? `cur-${i}` : undefined));

  it('reports the rows so far every few pages, then the whole', async () => {
    const door = new MockDoor().program(...pages(7));
    const seen: number[] = [];
    const step = await collectAll(client(door), 'std.tasks.list', {
      publishEvery: 3,
      onProgress: (s) => seen.push(s.rows.length),
    });
    expect(seen).toEqual([3, 6]);
    if (step.status !== 'page') throw new Error(step.status);
    expect(step.state.rows.map((r) => (r as { id: string }).id)).toEqual(pages(7).map((_, i) => `task_${i}`));
  });

  it('a signal ends the walk with what it has', async () => {
    const controller = new AbortController();
    const door = new MockDoor().program(...pages(5));
    let n = 0;
    const step = await collectAll(client(door), 'std.tasks.list', {
      signal: controller.signal,
      publishEvery: 1,
      onProgress: () => {
        n += 1;
        if (n === 2) controller.abort();
      },
    });
    if (step.status !== 'page') throw new Error(step.status);
    expect(step.state.rows).toHaveLength(2);
    expect(door.requests).toHaveLength(2);
  });

  it('sameRows: equal field for field, in order', () => {
    expect(sameRows([{ id: 'a', n: 1, j: { x: [1] } }], [{ id: 'a', n: 1, j: { x: [1] } }])).toBe(true);
    expect(sameRows([{ id: 'a', n: 1 }], [{ id: 'a', n: 2 }])).toBe(false);
    expect(sameRows([{ id: 'a' }, { id: 'b' }], [{ id: 'b' }, { id: 'a' }])).toBe(false);
    expect(sameRows([{ id: 'a' }], [{ id: 'a', extra: null }])).toBe(false);
  });
});
