/**
 * One stream per tab (`docs/plan-sdk-device.md` §7a): the tabs on one
 * replica elect a leader, which runs the only feeds.
 *
 * Each "tab" here is an `openDevice` on one shared backend, as tabs share
 * IndexedDB. They elect over Node's own `navigator.locks` and
 * `BroadcastChannel`, which behave as the browser's do within one process.
 * Each test uses its own store name, so tests never share a leader.
 */

import { MemoryBackend, OrdoClient, openDevice, type OpSpec, type OrdoDevice, type TabEnvironment } from '../src/index.js';
import { pageTabs } from '../src/replica/tabs.js';

/**
 * Node has `navigator.locks` from 22 on. The packages support Node 18 and
 * CI runs 20, where these tests have nothing to elect over and are skipped.
 * The fallback with neither is tested on every version, below.
 */
const hasLocks = typeof navigator !== 'undefined' && navigator.locks !== undefined;
const itTabs = it.skipIf(!hasLocks);

class Door {
  readonly urls: string[] = [];
  readonly resumes: (string | null)[] = [];
  readonly feeds: { url: string; push(text: string): void; ended: () => boolean; close(): void }[] = [];

  readonly fetch: typeof globalThis.fetch = async (input, init) => {
    const url = String(input);
    this.urls.push(url);
    this.resumes.push((init?.headers as Record<string, string> | undefined)?.['X-Ordo-Resume'] ?? null);
    const enc = new TextEncoder();
    let controller!: ReadableStreamDefaultController<Uint8Array>;
    let done = false;
    const stream = new ReadableStream<Uint8Array>({ start: (c) => void (controller = c) });
    init?.signal?.addEventListener('abort', () => {
      if (done) return;
      done = true;
      controller.error(init.signal!.reason);
    });
    this.feeds.push({
      url,
      push: (t) => !done && controller.enqueue(enc.encode(t)),
      ended: () => done,
      close: () => {
        if (done) return;
        done = true;
        controller.close();
      },
    });
    return new Response(stream, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
  };

  client(): OrdoClient {
    return new OrdoClient({ baseUrl: 'https://door.example/api', space: 'spc_test', token: 'tok', fetch: this.fetch });
  }

  open(): Door['feeds'] {
    return this.feeds.filter((f) => !f.ended());
  }

  async nth(n: number) {
    await until(() => this.feeds.length > n, `feed ${n}`);
    return this.feeds[n]!;
  }
}

async function until(p: () => boolean | Promise<boolean>, what = 'condition'): Promise<void> {
  for (let i = 0; i < 400; i++) {
    if (await p()) return;
    await new Promise((r) => setTimeout(r, 5));
  }
  throw new Error(`timed out waiting for ${what}`);
}

const frame = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
const load = (rows: unknown[], position: number, result?: string) =>
  frame('load', { entity: 'tasks', key: 'id', position, pin: null, rows, loaded: rows.length, complete: true, result });
const resumed = (position: number, hash: string, loaded: number) =>
  frame('load', { entity: 'tasks', key: 'id', position, pin: null, rows: [], loaded, complete: true, result: hash, resumed: true });
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
const tagged: OpSpec = {
  ...tasks,
  op: 'tasksTagged',
  params: [{ name: 'tag', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null }],
};

let stores = 0;
const opened: OrdoDevice[] = [];
afterEach(async () => {
  await Promise.allSettled(opened.splice(0).map((d) => d.close()));
});

/** Tabs on one backend and one store, against one door. */
function browser(env: () => TabEnvironment | null = pageTabs, idleMs = 30) {
  const backend = new MemoryBackend();
  const store = `tabs-test-${++stores}-${Date.now()}`;
  const door = new Door();
  const tab = async () => {
    const d = await openDevice({
      client: door.client(),
      space: 'spc_test',
      principal: 'ada',
      backend,
      store,
      tabs: 'share',
      tabEnvironment: env(),
      idleMs,
      manager: { online: null, publishMs: 0 },
    });
    opened.push(d);
    return d;
  };
  return { door, tab };
}

function ids(d: OrdoDevice, key: string) {
  const obs = d.manager.observe(key, () => undefined);
  return {
    now: () => obs.snapshot().rows.map((r) => r.id).join(),
    stop: () => obs.stop(),
  };
}

describe('tabs share one set of feeds', () => {
  itTabs('two tabs: the leader opens the one feed, and both screens match', async () => {
    const { door, tab } = browser();
    const a = await tab();
    const b = await tab();
    expect(a.leading).toBe(true);
    expect(b.leading).toBe(false);
    const key = a.manager.sync(tasks);
    b.manager.sync(tasks);
    (await door.nth(0)).push(load([task(1), task(2)], 3, 'h'));
    const sa = ids(a, key);
    const sb = ids(b, key);
    await until(() => sa.now() === 'task_1,task_2' && sb.now() === 'task_1,task_2', 'both screens');
    await until(() => b.manager.view.complete, 'the follower to be complete');
    (await door.nth(0)).push(change(4, [task(3)], []));
    await until(() => sb.now() === 'task_1,task_2,task_3', 'the change in the follower');
    expect(door.feeds).toHaveLength(1);
    sa.stop();
    sb.stop();
  });

  itTabs('a follower’s new op is opened by the leader', async () => {
    const { door, tab } = browser();
    const a = await tab();
    const b = await tab();
    const key = b.manager.sync(tagged, { tag: 'work' });
    const feed = await door.nth(0);
    expect(feed.url).toContain('tasksTagged');
    feed.push(load([task(1, 'work')], 2));
    const sb = ids(b, key);
    await until(() => sb.now() === 'task_1', 'the follower to show the row');
    expect(a.manager.view.ops.map((o) => o.key)).toEqual([key]);
    expect(door.feeds).toHaveLength(1);
    sb.stop();
  });

  itTabs('the leader closes: the follower leads and resumes with no rows moved', async () => {
    const { door, tab } = browser();
    const a = await tab();
    const b = await tab();
    const key = a.manager.sync(tasks);
    b.manager.sync(tasks);
    (await door.nth(0)).push(load([task(1), task(2)], 3, 'h-two'));
    const sb = ids(b, key);
    await until(() => sb.now() === 'task_1,task_2');
    const before = await b.replica.scan('tasks');

    await a.close();
    await until(() => b.leading, 'the follower to lead');
    const second = await door.nth(1);
    expect(door.resumes[1]).toBe('h-two');
    second.push(resumed(5, 'h-two', 2));
    await until(() => b.manager.view.ops[0]?.position === 5, 'the resume');
    expect(await b.replica.scan('tasks')).toEqual(before);
    expect(sb.now()).toBe('task_1,task_2');
    expect(door.open()).toHaveLength(1);
    sb.stop();
  });

  itTabs('one tab lets a key go while another uses it: the feed stays', async () => {
    const { door, tab } = browser();
    const a = await tab();
    const b = await tab();
    const key = a.manager.sync(tasks);
    b.manager.sync(tasks);
    (await door.nth(0)).push(load([task(1)], 3));
    await until(() => b.manager.view.complete);
    // The leader's own screens go; the follower still uses it.
    await a.manager.idle(key);
    await new Promise((r) => setTimeout(r, 100));
    expect(door.open()).toHaveLength(1);
    // Then the follower lets go too: after idleMs the feed closes.
    await b.manager.idle(key);
    await until(() => door.open().length === 0, 'the feed to close');
  });

  itTabs('a tab that dies without a word: the leader drops its count', async () => {
    let silent = false;
    const env = (): TabEnvironment => {
      const page = pageTabs()!;
      return {
        ...page,
        channel: (name) => {
          const c = page.channel(name);
          const post = c.postMessage.bind(c);
          // The dying tab says nothing more: no release, no goodbye.
          c.postMessage = (m: unknown) => {
            if (!silent) post(m);
          };
          return c;
        },
      };
    };
    const { door, tab } = browser(env);
    await tab();
    const b = await tab();
    b.manager.sync(tasks);
    (await door.nth(0)).push(load([task(1)], 3));
    await until(() => b.manager.view.complete);
    silent = true;
    await b.close();
    // Only its lock going says it died.
    await until(() => door.open().length === 0, 'the dead tab’s feed to close');
  });

  itTabs('poison in one tab empties every tab', async () => {
    const { door, tab } = browser();
    const a = await tab();
    const b = await tab();
    const key = a.manager.sync(tasks);
    b.manager.sync(tasks);
    (await door.nth(0)).push(load([task(1)], 3));
    const sa = ids(a, key);
    const sb = ids(b, key);
    await until(() => sa.now() === 'task_1' && sb.now() === 'task_1');
    await b.purge();
    await until(() => sa.now() === '' && a.replica.poisoned, 'the leader to empty');
    expect(await a.replica.scan('tasks')).toEqual([]);
    sa.stop();
    sb.stop();
  });

  it('with no BroadcastChannel or locks, each tab leads itself', async () => {
    const { door, tab } = browser(() => null);
    const a = await tab();
    const b = await tab();
    expect(a.tabs).toBe('own');
    expect(a.leading && b.leading).toBe(true);
    a.manager.sync(tasks);
    b.manager.sync(tasks);
    await door.nth(1);
    expect(door.feeds).toHaveLength(2);
  });

  itTabs('a filtered handle in a follower keeps its exact set across an edit and a handover', async () => {
    const { door, tab } = browser();
    const a = await tab();
    const b = await tab();
    a.manager.sync(tasks);
    const all = await door.nth(0);
    const key = b.manager.sync(tagged, { tag: 'work' });
    const work = await door.nth(1);
    all.push(load([task(1, 'work'), task(2, 'work')], 3, 'h-all'));
    work.push(load([task(1, 'work'), task(2, 'work')], 3, 'h-work-2'));
    const sb = ids(b, key);
    await until(() => sb.now() === 'task_1,task_2');

    // The tag comes off task 2.
    all.push(change(4, [task(2, 'no tag')], [], 'h-all-4'));
    work.push(change(4, [], ['task_2'], 'h-work-1'));
    await until(() => sb.now() === 'task_1', 'the edit in the follower');

    // The leader goes; the follower leads and resumes both.
    await a.close();
    await until(() => b.leading);
    await until(() => door.open().length === 1, 'the follower’s own feed');
    const again = door.open()[0]!;
    expect(again.url).toContain('tasksTagged');
    again.push(resumed(6, 'h-work-1', 1));
    await until(() => b.manager.view.ops.find((o) => o.key === key)?.position === 6);
    expect(sb.now()).toBe('task_1');
    sb.stop();
  });

  itTabs('reconnect() from a follower reopens the leader’s feeds', async () => {
    const { door, tab } = browser();
    const a = await tab();
    const b = await tab();
    a.manager.sync(tasks);
    b.manager.sync(tasks);
    (await door.nth(0)).push(load([task(1)], 3));
    await until(() => b.manager.view.complete);
    const done = b.reconnect();
    const second = await door.nth(1);
    expect(door.feeds[0]!.ended()).toBe(true);
    second.push(load([task(1)], 4));
    await done;
    expect(door.open()).toHaveLength(1);
  });
});

describe('a hidden leader gives way', () => {
  itTabs('to a tab that becomes visible', async () => {
    const pages = new Map<string, EventTarget & { visibilityState: string }>();
    const page = (name: string) => {
      const t = Object.assign(new EventTarget(), { visibilityState: 'visible' });
      pages.set(name, t);
      return t;
    };
    let next = 'a';
    const env = (): TabEnvironment => ({ ...pageTabs()!, visibility: page(next) });
    const { door, tab } = browser(env);
    const a = await tab();
    next = 'b';
    const b = await tab();
    a.manager.sync(tasks);
    b.manager.sync(tasks);
    (await door.nth(0)).push(load([task(1)], 3));
    await until(() => b.manager.view.complete);

    // Tab A is hidden and B comes to the front.
    pages.get('b')!.visibilityState = 'hidden';
    pages.get('a')!.visibilityState = 'hidden';
    pages.get('a')!.dispatchEvent(new Event('visibilitychange'));
    await new Promise((r) => setTimeout(r, 20));
    pages.get('b')!.visibilityState = 'visible';
    pages.get('b')!.dispatchEvent(new Event('visibilitychange'));
    await until(() => b.leading && !a.leading, 'B to lead');
    await until(() => door.open().length === 1 && door.feeds.length >= 2, 'one feed, from B');
  });
});
