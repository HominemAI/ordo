/**
 * The guest's half of sharing, against a mock door (`plan-sharing.md` G7).
 *
 * What is proved is what this package owns: which route is called with which
 * bytes and which token, that the window client carries the window token and
 * the shared tenant on every call and asks for a new token before the old one
 * ends, and that a window the sharer closed purges what it synced on that
 * tenant's replica and on no other.
 */

import { describe, expect, it } from 'vitest';

import {
  LocalReplica,
  MemoryBackend,
  OrdoClient,
  PasskeyError,
  SyncManager,
  acceptShare,
  declineInvitation,
  declineShare,
  leaveShare,
  linkFeed,
  listSpaces,
  openLink,
  openWindow,
  peekInvitation,
  peekShare,
  windowFeed,
} from '../src/index.js';
import type { ListedLink, MaxOpensParam, OpSpec, SyncView } from '../src/index.js';
import { activeProducts } from '../src/generated/index.js';
import { MockDoor } from './harness.js';

const BASE = 'https://door.example/api';

const WINDOW = (token: string, expiresAt: number) => ({
  json: { token, expires_at: expiresAt, space: 'spc_sharer', grant: 'shr_x', ops: ['activeProducts'] },
});
const ROWS = { json: { rows: [] } };
const REFUSED = { status: 403, json: { error: { code: 'share_refused', message: 'that share cannot be used' } } };

const OPENED = (token: string, expiresAt: number) => ({
  json: { token, expires_at: expiresAt, space: 'spc_sharer', link: 'lnk_x', ops: ['activeProducts'] },
});
const LINK = 'lks_' + 'ab'.repeat(32);

describe('the window client', () => {
  it('sends the window token and the shared space header', async () => {
    const door = new MockDoor().reply(WINDOW('win-1', 1100), ROWS);
    const client = await openWindow({ baseUrl: BASE, fetch: door.fetch }, 'session-tok', 'spc_sharer', {
      now: () => 1000,
    });
    expect(client.space).toBe('spc_sharer');
    await activeProducts(client).fetch();

    // The window is minted from the session, for that tenant.
    expect(door.calls[0]).toMatchObject({
      url: `${BASE}/auth/window`,
      method: 'POST',
      body: { space: 'spc_sharer' },
    });
    expect(door.calls[0]!.headers['Authorization']).toBe('Bearer session-tok');
    // Every read then carries the window token and the tenant it is bound to.
    expect(door.calls[1]!.headers['Authorization']).toBe('Bearer win-1');
    expect(door.calls[1]!.headers['X-Ordo-Space']).toBe('spc_sharer');
    expect(door.calls[1]!.url).toContain('/op/');
  });

  it('asks for a new window token before the old one ends, once for concurrent calls', async () => {
    const door = new MockDoor().reply(WINDOW('win-1', 1100), ROWS, WINDOW('win-2', 1200), ROWS);
    let clock = 1000;
    const client = await openWindow({ baseUrl: BASE, fetch: door.fetch }, () => 'fresh-session', 'spc_sharer', {
      now: () => clock,
    });
    // Ninety seconds from the end: no new mint.
    await activeProducts(client).fetch();
    expect(door.calls.filter((c) => c.url.endsWith('/auth/window'))).toHaveLength(1);

    // Twenty-five seconds from the end, inside the thirty-second margin:
    // three reads at once make one mint, and all three use the new token.
    clock = 1075;
    await Promise.all([
      activeProducts(client).fetch(),
      activeProducts(client).fetch(),
      activeProducts(client).fetch(),
    ]);
    const mints = door.calls.filter((c) => c.url.endsWith('/auth/window'));
    expect(mints).toHaveLength(2);
    expect(mints[1]!.headers['Authorization']).toBe('Bearer fresh-session');
    const reads = door.calls.filter((c) => c.url.includes('/op/'));
    expect(reads.slice(-3).map((c) => c.headers['Authorization'])).toEqual([
      'Bearer win-2',
      'Bearer win-2',
      'Bearer win-2',
    ]);
  });

  it('mints a new token when a refused one is renewed, and answers false once the window is closed', async () => {
    // A second accept replaces the grant the held token names, so the door
    // refuses that token long before it ends while the window is still open.
    const door = new MockDoor().reply(WINDOW('win-1', 5000), WINDOW('win-2', 5000), ROWS, REFUSED);
    const client = await openWindow({ baseUrl: BASE, fetch: door.fetch }, 'session-tok', 'spc_sharer', {
      now: () => 1000,
    });
    expect(await client.renewToken()).toBe(true);
    await activeProducts(client).fetch();
    expect(door.calls[2]!.headers['Authorization']).toBe('Bearer win-2');

    // Closed: the door will not mint one, and the refusal stands.
    expect(await client.renewToken()).toBe(false);
  });

  it('is refused when there is no window, and when the sharer closes it', async () => {
    // Never made, or already closed: the first mint says so, before any read.
    const none = new MockDoor().reply(REFUSED);
    await expect(
      openWindow({ baseUrl: BASE, fetch: none.fetch }, 'session-tok', 'spc_nobody'),
    ).rejects.toMatchObject({ failure: 'refused', message: 'that share cannot be used' });

    // Closed after it was opened: the next mint is refused, and it is the
    // call that needed the token that hears it.
    const door = new MockDoor().reply(WINDOW('win-1', 1100), REFUSED);
    let clock = 1000;
    const client = await openWindow({ baseUrl: BASE, fetch: door.fetch }, 'session-tok', 'spc_sharer', {
      now: () => clock,
    });
    clock = 1090;
    await expect(activeProducts(client).fetch()).rejects.toBeInstanceOf(PasskeyError);
  });
});

/** A fetch that keeps the request body exactly as it was sent. */
function raw() {
  const sent: { url: string; method: string; headers: Record<string, string>; body: string | null }[] = [];
  const replies: unknown[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    sent.push({
      url: String(input),
      method: String(init?.method),
      headers: Object.fromEntries(Object.entries((init?.headers ?? {}) as Record<string, string>)),
      body: typeof init?.body === 'string' ? init.body : null,
    });
    const json = replies.length > 1 ? replies.shift() : replies[0];
    return new Response(JSON.stringify(json), { status: 200 });
  };
  return { sent, fetch, reply: (...r: unknown[]) => void replies.push(...r) };
}

describe('the accept page', () => {
  it('accept posts the ticket and the name byte for byte', async () => {
    const door = raw();
    door.reply({
      space: 'spc_sharer',
      name: 'Ada’s notes',
      seat: 'mem_abc',
      grant: 'shr_0123',
      expires: 1_600_000,
      ops: ['boardList', 'cardList'],
    });
    const ticket = `tkt_${'ab12'.repeat(16)}`;
    const name = 'Gil “B.” Ünï';
    const accepted = await acceptShare({ baseUrl: BASE, fetch: door.fetch }, 'session-tok', ticket, name);

    expect(door.sent).toHaveLength(1);
    expect(door.sent[0]!.url).toBe(`${BASE}/auth/share/accept`);
    expect(door.sent[0]!.method).toBe('POST');
    expect(door.sent[0]!.headers['Authorization']).toBe('Bearer session-tok');
    // Byte for byte: the ticket is not trimmed, cased or encoded, and the
    // name is JSON of exactly what the person typed.
    expect(door.sent[0]!.body).toBe(JSON.stringify({ ticket, display_name: name }));
    expect(accepted).toEqual({
      space: 'spc_sharer',
      name: 'Ada’s notes',
      seat: 'mem_abc',
      grant: 'shr_0123',
      expires: 1_600_000,
      ops: ['boardList', 'cardList'],
    });
  });

  it('peek needs no session and says what accepting discloses', async () => {
    const door = raw();
    door.reply({
      space: 'spc_sharer',
      name: null,
      label: 'the roadmap',
      ops: ['boardList'],
      rows: [{ entity: 'boards', id: 'board_1', level: 'read' }],
      window_ends_at: 1_700_000,
      ticket_expires_at: 1_650_000,
      named: false,
      discloses: 'the display name you enter is shown to the sharer',
      notice: 'The owner of this tenant can reach anything in it, and every time they do it is logged.',
    });
    const ticket = `tkt_${'cd34'.repeat(16)}`;
    const peeked = await peekShare({ baseUrl: BASE, fetch: door.fetch }, ticket);
    expect(door.sent[0]!.url).toBe(`${BASE}/auth/share/peek`);
    expect(door.sent[0]!.headers['Authorization']).toBeUndefined();
    expect(door.sent[0]!.body).toBe(JSON.stringify({ ticket }));
    expect(peeked).toMatchObject({
      space: 'spc_sharer',
      name: null,
      label: 'the roadmap',
      ops: ['boardList'],
      rows: [{ entity: 'boards', id: 'board_1', level: 'read' }],
      windowEndsAt: 1_700_000,
      ticketExpiresAt: 1_650_000,
      named: false,
    });
    expect(peeked.notice).toContain('owner');
  });

  it('leave posts the tenant, and the list is the person’s own', async () => {
    const door = raw();
    door.reply({ space: 'spc_sharer', outcome: 'left' });
    await leaveShare({ baseUrl: BASE, fetch: door.fetch }, 'session-tok', 'spc_sharer');
    expect(door.sent[0]!.url).toBe(`${BASE}/auth/share/leave`);
    expect(door.sent[0]!.body).toBe(JSON.stringify({ space: 'spc_sharer' }));
    expect(door.sent[0]!.headers['Authorization']).toBe('Bearer session-tok');

    const lists = raw();
    lists.reply([
      { space: 'spc_own', name: 'Ada', role: 'owner', via: 'member' },
      { space: 'spc_sharer', name: null, role: null, via: 'guest' },
    ]);
    const mine = await listSpaces({ baseUrl: BASE, fetch: lists.fetch }, 'session-tok');
    expect(lists.sent[0]!.method).toBe('GET');
    expect(lists.sent[0]!.url).toBe(`${BASE}/auth/spaces`);
    expect(mine).toEqual([
      { space: 'spc_own', name: 'Ada', role: 'owner', via: 'member' },
      { space: 'spc_sharer', name: null, role: null, via: 'guest' },
    ]);
  });

  it('decline posts the ticket, with a session only when one is given', async () => {
    const door = raw();
    door.reply({ space: 'spc_sharer', outcome: 'declined' });
    await declineShare({ baseUrl: BASE, fetch: door.fetch }, 'tkt_x');
    expect(door.sent[0]!.url).toBe(`${BASE}/auth/share/decline`);
    expect(door.sent[0]!.body).toBe(JSON.stringify({ ticket: 'tkt_x' }));
    expect(door.sent[0]!.headers['Authorization']).toBeUndefined();

    const named = raw();
    named.reply({ space: 'spc_t', outcome: 'declined' });
    await declineInvitation({ baseUrl: BASE, fetch: named.fetch }, 'tkt_y', 'session-tok');
    expect(named.sent[0]!.url).toBe(`${BASE}/auth/invitation/decline`);
    expect(named.sent[0]!.headers['Authorization']).toBe('Bearer session-tok');

    const peek = raw();
    peek.reply({ space: 'spc_t', role: null, ticket_expires_at: 2000, named: true });
    expect(await peekInvitation({ baseUrl: BASE, fetch: peek.fetch }, 'tkt_y')).toEqual({
      space: 'spc_t',
      role: null,
      ticketExpiresAt: 2000,
      named: true,
    });

    const fetch: typeof globalThis.fetch = async () =>
      new Response(JSON.stringify({ error: { code: 'invitation_refused' } }), { status: 403 });
    await expect(declineInvitation({ baseUrl: BASE, fetch }, 'tkt_z')).rejects.toMatchObject({
      failure: 'refused',
      message: 'that invitation cannot be used',
    });
  });

  it('a refused share is one message whatever the reason', async () => {
    const fetch: typeof globalThis.fetch = async () =>
      new Response(JSON.stringify({ error: { code: 'share_refused' } }), { status: 403 });
    await expect(
      acceptShare({ baseUrl: BASE, fetch }, 'session-tok', 'tkt_x', 'Gil'),
    ).rejects.toMatchObject({ failure: 'refused', message: 'that share cannot be used' });
  });
});

// ── the replica ──────────────────────────────────────────────────────────

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
    signal?.addEventListener('abort', () => {
      if (this.#done) return;
      this.#done = true;
      this.#controller.error(signal.reason);
    });
  }

  readonly push: (text: string) => void;

  close(): void {
    if (this.#done) return;
    this.#done = true;
    this.#controller.close();
  }
}

/** A door that opens a feed, then refuses the reconnect as a revoked window is. */
class WindowDoor {
  readonly feeds: Feed[] = [];
  #refuse = false;
  #waiting: (() => void)[] = [];

  /** From now on every call is `access_denied`, the way a closed window is. */
  close(): void {
    this.#refuse = true;
  }

  readonly fetch: typeof globalThis.fetch = async (_input, init) => {
    if (this.#refuse) {
      return new Response(
        JSON.stringify({ error: { code: 'access_denied', message: 'op is not callable with this token', op: 'boardList' } }),
        { status: 403, headers: { 'Content-Type': 'application/json' } },
      );
    }
    const feed = new Feed(init?.signal);
    this.feeds.push(feed);
    for (const w of this.#waiting.splice(0)) w();
    return new Response(feed.stream, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
  };

  async nth(n: number): Promise<Feed> {
    while (this.feeds.length <= n) await new Promise<void>((r) => this.#waiting.push(r));
    return this.feeds[n]!;
  }
}

const boards: OpSpec = {
  op: 'boardList',
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
  sync: { entity: 'boards', key: 'id' },
  refine: { tier: 'none', fields: ['id', 'name'] },
  order: { keys: [{ path: 'id', desc: false }], stated: true },
};

const frame = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
const load = (rows: unknown[], position: number) =>
  frame('load', { entity: 'boards', key: 'id', position, pin: null, rows, loaded: rows.length, complete: true });

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

describe('a window the sharer closed', () => {
  it('a refusal after revocation purges the replica of that space only', async () => {
    // One device, one person, two replicas: the shared tenant's, synced
    // through the window, and the person's own tenant's.
    const backend = new MemoryBackend();
    const shared = await LocalReplica.open(backend, 'spc_sharer', 'usr_gil');
    const own = await LocalReplica.open(backend, 'spc_own', 'usr_gil');
    await own.put({ entity: 'boards', id: 'mine_1', position: 4, row: { id: 'mine_1', name: 'my own' } });

    const door = new WindowDoor();
    const client = new OrdoClient({ baseUrl: BASE, space: 'spc_sharer', token: 'win-1', fetch: door.fetch });
    const manager = SyncManager.start(client, shared, [{ spec: boards }], { wait: async () => {} });
    (await door.nth(0)).push(load([{ id: 'board_1', name: 'roadmap' }], 3));
    await when(manager, (v) => v.complete);
    expect((await shared.scan('boards')).map((r) => r.id)).toEqual(['board_1']);

    // The sharer closes the window. The feed drops, the reconnect is refused
    // `access_denied`, and what the window synced goes.
    door.close();
    (await door.nth(0)).close();
    await when(manager, (v) => v.ops[0]!.stopped !== null);
    expect(manager.view.ops[0]!.stopped?.detail.class).toBe('access_denied');
    expect(await shared.scan('boards')).toEqual([]);

    // The person's own tenant's replica is another namespace and is intact,
    // and poisoning the shared one cannot reach it.
    await shared.poison();
    expect(shared.poisoned).toBe(true);
    expect(await shared.get('boards', 'board_1')).toBeUndefined();
    expect((await own.scan('boards')).map((r) => r.id)).toEqual(['mine_1']);
    expect(own.poisoned).toBe(false);
    await manager.stop();
  });
});

describe('the link client', () => {
  it('open_link_posts_the_secret_and_binds_the_answered_space', async () => {
    const door = new MockDoor().reply(OPENED('lnk-tok-1', 1100), ROWS);
    const client = await openLink({ baseUrl: BASE, fetch: door.fetch }, LINK, { now: () => 1000 });
    // The tenant is the door's answer, not the caller's.
    expect(client.space).toBe('spc_sharer');
    await activeProducts(client).fetch();

    // The secret goes to the one route, with no Authorization.
    expect(door.calls[0]).toMatchObject({
      url: `${BASE}/auth/link`,
      method: 'POST',
      body: { link: LINK },
    });
    expect(door.calls[0]!.headers['Authorization']).toBeUndefined();
    // Every read carries the link token and the tenant it is bound to, and
    // never the secret.
    expect(door.calls[1]!.headers['Authorization']).toBe('Bearer lnk-tok-1');
    expect(door.calls[1]!.headers['X-Ordo-Space']).toBe('spc_sharer');
    expect(JSON.stringify(door.calls[1])).not.toContain(LINK);
  });

  it('opens the link again before the token ends, and says one thing when it is refused', async () => {
    const door = new MockDoor().reply(OPENED('t1', 1100), ROWS, OPENED('t2', 1200), ROWS);
    let clock = 1000;
    const client = await openLink({ baseUrl: BASE, fetch: door.fetch }, LINK, { now: () => clock });
    await activeProducts(client).fetch();
    clock = 1090;
    await Promise.all([activeProducts(client).fetch(), activeProducts(client).fetch()]);
    expect(door.calls.filter((c) => c.url.endsWith('/auth/link'))).toHaveLength(2);
    expect(door.calls.at(-1)!.headers['Authorization']).toBe('Bearer t2');

    for (const status of [403, 429]) {
      const closed = new MockDoor().reply({
        status,
        json: { error: { code: 'link_refused', message: 'that link cannot be used' } },
      });
      await expect(openLink({ baseUrl: BASE, fetch: closed.fetch }, LINK)).rejects.toMatchObject({
        failure: 'refused',
        message: 'that link cannot be used',
      });
    }
  });

  it('sends the password beside the secret, on the first open and on each renewal', async () => {
    const door = new MockDoor().reply(OPENED('t1', 1100), ROWS, OPENED('t2', 1200), ROWS);
    let clock = 1000;
    const client = await openLink({ baseUrl: BASE, fetch: door.fetch }, LINK, {
      now: () => clock,
      password: 'correct horse',
    });
    await activeProducts(client).fetch();
    clock = 1090;
    await activeProducts(client).fetch();
    const opens = door.calls.filter((c) => c.url.endsWith('/auth/link'));
    expect(opens).toHaveLength(2);
    for (const open of opens) expect(open.body).toMatchObject({ link: LINK, password: 'correct horse' });
    // Reads carry the token, never the password.
    for (const read of door.calls.filter((c) => !c.url.endsWith('/auth/link'))) {
      expect(JSON.stringify(read)).not.toContain('correct horse');
    }
  });

  it('types the password answers, each its own failure', async () => {
    const cases = [
      {
        reply: { status: 401, json: { error: { code: 'link_password_required', message: 'x' } } },
        want: { failure: 'link_password_required' },
      },
      {
        reply: { status: 401, json: { error: { code: 'link_password_wrong', message: 'x' } } },
        want: { failure: 'link_password_wrong' },
      },
      {
        reply: {
          status: 429,
          json: {
            error: {
              code: 'link_locked',
              message: 'x',
              retry_after: 600,
              setting: 'std.space.setLinkPasswordLimits',
            },
          },
        },
        want: { failure: 'link_locked', retryAfter: 600, setting: 'std.space.setLinkPasswordLimits' },
      },
      {
        // The always-on slowdown (owner, 2026-10-25): wait, never locked.
        reply: {
          status: 429,
          json: {
            error: {
              code: 'too_many_attempts',
              message: 'x',
              retry_after: 4,
              setting: 'std.space.setLinkPasswordLimits',
            },
          },
        },
        want: {
          failure: 'too_many_attempts',
          retryAfter: 4,
          retryAfterMs: 4000,
          setting: 'std.space.setLinkPasswordLimits',
        },
      },
    ];
    for (const { reply, want } of cases) {
      const door = new MockDoor().reply(reply);
      await expect(openLink({ baseUrl: BASE, fetch: door.fetch }, LINK)).rejects.toMatchObject(want);
    }
  });

  it('opens with the password after the door asked for it', async () => {
    const door = new MockDoor().reply(
      { status: 401, json: { error: { code: 'link_password_required', message: 'x' } } },
      OPENED('t1', 1100),
    );
    const endpoint = { baseUrl: BASE, fetch: door.fetch };
    await expect(openLink(endpoint, LINK)).rejects.toMatchObject({ failure: 'link_password_required' });
    const client = await openLink(endpoint, LINK, { password: 'correct horse', now: () => 1000 });
    expect(client.space).toBe('spc_sharer');
    expect(door.calls[1]!.body).toMatchObject({ link: LINK, password: 'correct horse' });
  });

  it('sends one device id on the first open and on every renewal, so a renewal is not a new open', async () => {
    const door = new MockDoor().reply(OPENED('t1', 1100), OPENED('t2', 1200));
    let clock = 1000;
    const feed = await linkFeed({ baseUrl: BASE, fetch: door.fetch }, LINK, {
      now: () => clock,
      device: 'dev-fixed',
    });
    expect(await feed.token()).toBe('t1');
    clock = 1090;
    expect(await feed.token()).toBe('t2');
    const opens = door.calls.filter((c) => c.url.endsWith('/auth/link'));
    expect(opens).toHaveLength(2);
    expect(opens.map((c) => (c.body as { device?: string }).device)).toEqual(['dev-fixed', 'dev-fixed']);
  });

  it('makes a device id when none is given, the same for the life of the feed', async () => {
    const door = new MockDoor().reply(OPENED('t1', 1100), OPENED('t2', 1200));
    let clock = 1000;
    const feed = await linkFeed({ baseUrl: BASE, fetch: door.fetch }, LINK, { now: () => clock });
    clock = 1090;
    await feed.token();
    const ids = door.calls.map((c) => (c.body as { device?: string }).device);
    expect(typeof ids[0]).toBe('string');
    expect(ids[0]!.length).toBeGreaterThanOrEqual(16);
    expect(ids[1]).toBe(ids[0]);
  });

  it('shows a used-up link as the one refusal, with no reason to the holder', async () => {
    const door = new MockDoor().reply({
      status: 403,
      json: { error: { code: 'link_refused', message: 'that link cannot be used' } },
    });
    await expect(openLink({ baseUrl: BASE, fetch: door.fetch }, LINK)).rejects.toMatchObject({
      failure: 'refused',
      message: 'that link cannot be used',
    });
  });

  it('types a listed link with its opens used, its limit and the used-up state', () => {
    const row: ListedLink = {
      id: 'lnk_1',
      label: 'x',
      ops: ['boardList'],
      rows: [],
      opens: 5,
      createdAt: 1,
      expiresAt: 2,
      state: 'used-up',
      issuedBy: 'p',
      hasPassword: false,
      lockedUntil: null,
      opensUsed: 3,
      maxOpens: 3,
    };
    const limit: MaxOpensParam = '3';
    expect([row.state, row.opensUsed, row.maxOpens, limit]).toEqual(['used-up', 3, 3, '3']);
  });
});

describe('a guest feed for a client built elsewhere', () => {
  it('gives a window guest the tenant and a token function, with no token property', async () => {
    const door = new MockDoor().reply(WINDOW('win-1', 1100), WINDOW('win-2', 1200));
    let clock = 1000;
    const feed = await windowFeed({ baseUrl: BASE, fetch: door.fetch }, 'session-tok', 'spc_sharer', {
      now: () => clock,
    });
    expect(feed.space).toBe('spc_sharer');
    // The token is the answer of a call, never a field a page can read or log.
    expect(Object.keys(feed).sort()).toEqual(['ops', 'renew', 'space', 'token']);
    expect(JSON.stringify(feed)).not.toContain('win-1');
    expect(await feed.token()).toBe('win-1');
    // And it renews before it ends, as the SDK's own window client does.
    clock = 1090;
    expect(await feed.token()).toBe('win-2');
    expect(door.calls.filter((c) => c.url.endsWith('/auth/window'))).toHaveLength(2);
  });

  it('gives a link guest the tenant the door named and a token function', async () => {
    const door = new MockDoor().reply(OPENED('t1', 1100));
    const feed = await linkFeed({ baseUrl: BASE, fetch: door.fetch }, LINK, { now: () => 1000 });
    expect(feed.space).toBe('spc_sharer');
    expect(JSON.stringify(feed)).not.toContain('t1');
    expect(JSON.stringify(feed)).not.toContain(LINK);
    expect(await feed.token()).toBe('t1');
  });

  it('says which ops the share lets the guest call, so a page asks before it reads', async () => {
    const door = new MockDoor().reply(OPENED('t1', 1100));
    const link = await linkFeed({ baseUrl: BASE, fetch: door.fetch }, LINK, { now: () => 1000 });
    expect(link.ops?.()).toEqual(['activeProducts']);
    const door2 = new MockDoor().reply(WINDOW('w1', 1100));
    const window = await windowFeed({ baseUrl: BASE, fetch: door2.fetch }, 'session-tok', 'spc_sharer', {
      now: () => 1000,
    });
    expect(window.ops?.()).toEqual(['activeProducts']);
  });
});
