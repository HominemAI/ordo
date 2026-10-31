/**
 * One set of feeds per person and space, however many tabs
 * (`docs/plan-sdk-device.md` §7a).
 *
 * Every tab on one replica used to run its own feeds, so two or three notes
 * tabs ran out of the browser's 6 connections per host over HTTP/1.1, and the
 * server read every result once per tab. Now the tabs elect a leader:
 *
 * - **Election.** The leader holds the Web Lock
 *   `ordo-sync-leader:<store>:<ns>`. The others wait on it. Tabs talk on
 *   `BroadcastChannel('ordo-sync:<store>:<ns>')`. `<ns>` is the replica's
 *   namespace, a digest of space and person, so one person's rows never
 *   cross another's channel on the same origin. `<store>` is the IndexedDB
 *   name, so two stores never share a leader.
 * - **The leader** runs the only real feeds and is the only tab writing
 *   frames. After each frame it posts `{key, state, delta}`, `members`
 *   included, and a refusal that stops a feed.
 * - **A follower** keeps its own manager, with the same surface, in follow
 *   mode: declaring an op asks the leader for it, and the leader's frames
 *   reach its memory and view. Its screens fill from the store once and then
 *   take the deltas; the fill buffer and `members` cover the race.
 * - **Counting tabs.** The leader keeps a feed while any tab uses its key,
 *   and idles a key no tab uses after `idleMs`. Each tab holds the lock
 *   `ordo-tab:<store>:<ns>:<id>` for its life, so the leader learns when one dies.
 * - **Handover.** When the leader closes, a waiting tab wins the lock, says
 *   so, every tab asks for its keys again, and the new leader resumes each
 *   feed on its stored result hash. A hidden leader gives way to a visible
 *   tab, which takes the lock, because browsers slow hidden tabs' timers.
 * - **Sign-out** in any tab poisons the shared replica, and every tab
 *   empties at once (C7).
 *
 * Without `BroadcastChannel` or `navigator.locks`, every tab leads itself.
 */

import type { Params } from '../core/client.js';
import { OrdoRefusal, type RefusalDetail } from '../core/refusal.js';
import type { Refine } from '../core/refine.js';
import type { OpSpec } from '../core/spec.js';
import type { FeedSource, ManagerLink, SyncManager } from './manager.js';
import type { LocalReplica } from './replica.js';
import type { SyncDelta, SyncState } from './sync.js';

/** The part of `navigator.locks` the link uses. */
export interface TabLocks {
  request<T>(
    name: string,
    options: { readonly ifAvailable?: boolean; readonly steal?: boolean; readonly signal?: AbortSignal },
    callback: (lock: unknown) => Promise<T> | T,
  ): Promise<T>;
}

/** The part of `BroadcastChannel` the link uses. */
export interface TabChannel {
  postMessage(message: unknown): void;
  onmessage: ((event: { readonly data: unknown }) => void) | null;
  close(): void;
}

/** The page's visibility, as `document` gives it. */
export interface TabVisibility {
  readonly visibilityState: string;
  addEventListener(type: 'visibilitychange', listener: () => void): void;
  removeEventListener(type: 'visibilitychange', listener: () => void): void;
}

export interface TabEnvironment {
  readonly locks: TabLocks;
  readonly channel: (name: string) => TabChannel;
  /** `document`, where there is one. Without it every tab counts as visible. */
  readonly visibility?: TabVisibility | null;
}

/** The page's own locks and channel, or `null` when it lacks either. */
export function pageTabs(): TabEnvironment | null {
  const g = globalThis as {
    navigator?: { locks?: TabLocks };
    BroadcastChannel?: new (name: string) => TabChannel;
    document?: TabVisibility;
  };
  const locks = g.navigator?.locks;
  const Channel = g.BroadcastChannel;
  if (!locks || typeof locks.request !== 'function' || typeof Channel !== 'function') return null;
  return {
    locks,
    channel: (name) => new Channel(name),
    visibility: g.document && typeof g.document.addEventListener === 'function' ? g.document : null,
  };
}

type Declared = { readonly spec: OpSpec; readonly params: Params; readonly refine: Refine | undefined };

/** What tabs say to each other. Every message names the tab it came from. */
type Message =
  | { readonly t: 'hello'; readonly from: string }
  | { readonly t: 'leader'; readonly from: string; readonly hidden: boolean }
  | { readonly t: 'want'; readonly from: string; readonly key: string; readonly declared: Declared }
  | { readonly t: 'release'; readonly from: string; readonly key: string }
  | { readonly t: 'bye'; readonly from: string }
  | { readonly t: 'frame'; readonly from: string; readonly key: string; readonly state: SyncState; readonly delta: SyncDelta }
  | { readonly t: 'stopped'; readonly from: string; readonly key: string; readonly detail: RefusalDetail }
  | { readonly t: 'reconnect'; readonly from: string; readonly req: string }
  | { readonly t: 'reconnected'; readonly from: string; readonly req: string }
  | { readonly t: 'nudge'; readonly from: string }
  | { readonly t: 'poisoned'; readonly from: string };

let tabCount = 0;
function newTabId(): string {
  const c = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  tabCount += 1;
  return c?.randomUUID ? c.randomUUID() : `tab-${Date.now()}-${tabCount}-${Math.random()}`;
}

/** One tab's part in sharing feeds. Built by `openDevice`. */
export class TabLink {
  readonly id = newTabId();
  readonly #env: TabEnvironment;
  readonly #manager: SyncManager;
  readonly #replica: LocalReplica;
  readonly #idleMs: number;
  readonly #channel: TabChannel;
  readonly #leaderLock: string;
  readonly #tabLockPrefix: string;
  readonly #closing = new AbortController();
  #leading = false;
  #closed = false;
  /** Ends the leader lock's hold. */
  #letGo: (() => void) | null = null;
  /** Ends this tab's own lock's hold. */
  #endTab: (() => void) | null = null;
  #leaderHidden = false;

  // Follower state.
  /** What this tab asked the leader for, while its connection runs. */
  readonly #wants = new Map<string, Declared>();
  /** How to end each waiting connection with the leader's refusal. */
  readonly #refuse = new Map<string, (refusal: OrdoRefusal) => void>();
  readonly #pending = new Map<string, () => void>();

  // Leader state.
  /** Keys this tab's own screens use. */
  readonly #own = new Set<string>();
  /** The other tabs using each key. */
  readonly #remote = new Map<string, Set<string>>();
  readonly #idleTimers = new Map<string, ReturnType<typeof setTimeout>>();
  /** Tabs whose lock the leader waits on, to learn when they die. */
  readonly #watched = new Set<string>();
  #offVisibility: (() => void) | null = null;
  #offPoison: (() => void) | null = null;

  private constructor(env: TabEnvironment, manager: SyncManager, replica: LocalReplica, idleMs: number, store: string) {
    this.#env = env;
    this.#manager = manager;
    this.#replica = replica;
    this.#idleMs = idleMs;
    const ns = `${store}:${replica.namespace}`;
    this.#leaderLock = `ordo-sync-leader:${ns}`;
    this.#tabLockPrefix = `ordo-tab:${ns}:`;
    this.#channel = env.channel(`ordo-sync:${ns}`);
    this.#channel.onmessage = (event) => this.#hear(event.data as Message);
  }

  /** Join the tabs on `replica` in `store`: lead when no tab does, follow otherwise. */
  static async join(
    env: TabEnvironment,
    manager: SyncManager,
    replica: LocalReplica,
    idleMs: number,
    store: string,
  ): Promise<TabLink> {
    const link = new TabLink(env, manager, replica, idleMs, store);
    await link.#start();
    return link;
  }

  /** True while this tab runs the feeds. */
  get leading(): boolean {
    return this.#leading;
  }

  async #start(): Promise<void> {
    // This tab's own lock, held for its life, before anyone hears of it.
    await new Promise<void>((granted) => {
      void this.#env.locks
        .request(this.#tabLockPrefix + this.id, { signal: this.#closing.signal }, () => {
          granted();
          return new Promise<void>((end) => {
            this.#endTab = end;
          });
        })
        .catch(() => granted());
    });
    this.#manager._attach(this.#hooks());
    this.#offPoison = this.#replica.onPoison(() => this.#post({ t: 'poisoned', from: this.id }));
    const vis = this.#env.visibility;
    if (vis) {
      const onChange = () => this.#visibilityChanged();
      vis.addEventListener('visibilitychange', onChange);
      this.#offVisibility = () => vis.removeEventListener('visibilitychange', onChange);
    }
    // Lead at once when nobody does; otherwise follow and wait in line.
    const free = await new Promise<boolean>((decided) => this.#take({ ifAvailable: true }, decided));
    if (!free) {
      this.#follow(false);
      this.#queue(false);
      this.#post({ t: 'hello', from: this.id });
    }
  }

  /** A queued request for the leader lock is waiting. */
  #inLine = false;

  /**
   * Ask for the leader lock. While it is held this tab leads. When a hold
   * ends because another tab stole it, this tab follows and waits in line.
   */
  #take(
    options: { readonly ifAvailable?: boolean; readonly steal?: boolean; readonly signal?: AbortSignal },
    decided?: (got: boolean) => void,
  ): void {
    let held = false;
    this.#env.locks
      .request(this.#leaderLock, options, (lock) => {
        if (lock === null || this.#closed) {
          decided?.(false);
          return undefined;
        }
        held = true;
        decided?.(true);
        this.#lead();
        return new Promise<void>((end) => {
          this.#letGo = end;
        });
      })
      .then(
        () => undefined,
        () => {
          decided?.(false);
          // Stolen from while leading: follow, and wait in line again.
          if (this.#closed || !held || !this.#leading) return;
          this.#letGo = null;
          this.#follow(true);
          this.#queue(false);
        },
      );
  }

  /** Wait in line for the leader lock, or take it now with `steal`. */
  #queue(steal: boolean): void {
    if (this.#closed) return;
    if (steal) {
      this.#take({ steal: true });
      return;
    }
    if (this.#inLine) return;
    this.#inLine = true;
    this.#take({ signal: this.#closing.signal }, () => {
      this.#inLine = false;
    });
  }

  #lead(): void {
    if (this.#closed) return;
    this.#leading = true;
    this.#own.clear();
    for (const d of this.#manager._declared()) this.#own.add(d.key);
    this.#wants.clear();
    for (const resolve of this.#pending.values()) resolve();
    this.#pending.clear();
    this.#hooksSource(null);
    this.#post({ t: 'leader', from: this.id, hidden: this.#hidden() });
    // Every connection reopens as this tab's own feed, resuming on the stored hashes.
    void this.#manager.reconnect();
  }

  #follow(wasLeading: boolean): void {
    this.#leading = false;
    for (const t of this.#idleTimers.values()) clearTimeout(t);
    this.#idleTimers.clear();
    if (wasLeading) {
      // Keys only other tabs used go; this tab's own stay, followed.
      for (const d of this.#manager._declared()) if (!this.#own.has(d.key)) void this.#manager._idle(d.key);
    }
    this.#remote.clear();
    this.#hooksSource(this.#source());
    if (wasLeading) void this.#manager.reconnect();
  }

  #hooksSource(source: FeedSource | null): void {
    this.#link.source = source;
  }

  #link!: ManagerLink;

  #hooks(): ManagerLink {
    this.#link = {
      source: null,
      synced: (key) => {
        if (!this.#leading) return;
        this.#own.add(key);
        this.#cancelIdle(key);
      },
      idling: (key) => {
        if (!this.#leading) return true;
        this.#own.delete(key);
        return (this.#remote.get(key)?.size ?? 0) === 0;
      },
      frame: (key, state, delta) => {
        if (this.#leading) this.#post({ t: 'frame', from: this.id, key, state, delta });
      },
      stopped: (key, refusal) => {
        if (this.#leading) this.#post({ t: 'stopped', from: this.id, key, detail: refusal.detail as RefusalDetail });
      },
    };
    return this.#link;
  }

  /** A follower's connection: ask the leader, then wait for its frames. */
  #source(): FeedSource {
    return (declared, signal) =>
      new Promise<void>((_resolve, reject) => {
        const { key } = declared;
        const d: Declared = { spec: declared.spec, params: declared.params, refine: declared.refine };
        this.#wants.set(key, d);
        const end = (error: unknown) => {
          signal.removeEventListener('abort', onAbort);
          if (this.#refuse.get(key) === refuse) this.#refuse.delete(key);
          this.#wants.delete(key);
          if (!this.#closed) this.#post({ t: 'release', from: this.id, key });
          reject(error);
        };
        const onAbort = () => end(signal.reason);
        const refuse = (refusal: OrdoRefusal) => end(refusal);
        signal.addEventListener('abort', onAbort, { once: true });
        this.#refuse.set(key, refuse);
        this.#post({ t: 'want', from: this.id, key, declared: d });
      });
  }

  #hear(m: Message): void {
    if (this.#closed || !m || m.from === this.id) return;
    switch (m.t) {
      case 'hello':
        if (this.#leading) this.#post({ t: 'leader', from: this.id, hidden: this.#hidden() });
        return;
      case 'leader':
        if (this.#leading) return;
        this.#leaderHidden = m.hidden;
        // A new leader, or one saying hello: ask again for every key in use.
        for (const [key, declared] of this.#wants) this.#post({ t: 'want', from: this.id, key, declared });
        for (const resolve of this.#pending.values()) resolve();
        this.#pending.clear();
        if (m.hidden && !this.#hidden()) this.#queue(true);
        return;
      case 'want':
        if (this.#leading) this.#wanted(m.from, m.key, m.declared);
        return;
      case 'release':
        if (this.#leading) this.#released(m.from, m.key);
        return;
      case 'bye':
        if (this.#leading) this.#gone(m.from);
        return;
      case 'frame':
        if (!this.#leading) this.#manager._remoteFrame(m.key, m.state, m.delta);
        return;
      case 'stopped':
        if (!this.#leading) this.#refuse.get(m.key)?.(new OrdoRefusal(m.detail));
        return;
      case 'reconnect':
        if (this.#leading) {
          void this.#manager.reconnect().then(() => this.#post({ t: 'reconnected', from: this.id, req: m.req }));
        }
        return;
      case 'reconnected':
        this.#pending.get(m.req)?.();
        this.#pending.delete(m.req);
        return;
      case 'nudge':
        if (this.#leading) this.#manager.nudge();
        return;
      case 'poisoned':
        if (!this.#replica.poisoned) void this.#replica.poison();
        return;
    }
  }

  // ── leader: counting the tabs ─────────────────────────────────────────

  #wanted(tab: string, key: string, declared: Declared): void {
    let tabs = this.#remote.get(key);
    if (!tabs) {
      tabs = new Set();
      this.#remote.set(key, tabs);
    }
    tabs.add(tab);
    this.#cancelIdle(key);
    this.#watch(tab);
    this.#manager._sync(declared.spec, declared.params, declared.refine);
    // Where the feed stands now, so the asking tab need not wait for a change.
    const state = this.#manager._state(key);
    if (state) this.#post({ t: 'frame', from: this.id, key, state, delta: { upserts: [], releases: [], reset: false } });
  }

  #released(tab: string, key: string): void {
    const tabs = this.#remote.get(key);
    if (!tabs) return;
    tabs.delete(tab);
    if (tabs.size > 0) return;
    this.#remote.delete(key);
    if (this.#own.has(key)) return;
    this.#idleLater(key);
  }

  #gone(tab: string): void {
    for (const key of [...this.#remote.keys()]) this.#released(tab, key);
  }

  #idleLater(key: string): void {
    this.#cancelIdle(key);
    if (!Number.isFinite(this.#idleMs)) return;
    this.#idleTimers.set(
      key,
      setTimeout(() => {
        this.#idleTimers.delete(key);
        if (!this.#leading || this.#own.has(key) || (this.#remote.get(key)?.size ?? 0) > 0) return;
        void this.#manager._idle(key);
      }, Math.max(0, this.#idleMs)),
    );
  }

  #cancelIdle(key: string): void {
    const t = this.#idleTimers.get(key);
    if (t !== undefined) clearTimeout(t);
    this.#idleTimers.delete(key);
  }

  /** Wait on a tab's own lock: it is free once that tab has gone. */
  #watch(tab: string): void {
    if (this.#watched.has(tab)) return;
    this.#watched.add(tab);
    void this.#env.locks
      .request(this.#tabLockPrefix + tab, { signal: this.#closing.signal }, () => {
        this.#watched.delete(tab);
        if (this.#leading) this.#gone(tab);
      })
      .catch(() => undefined);
  }

  // ── visibility ────────────────────────────────────────────────────────

  #hidden(): boolean {
    return this.#env.visibility?.visibilityState === 'hidden';
  }

  #visibilityChanged(): void {
    if (this.#closed) return;
    if (this.#leading) this.#post({ t: 'leader', from: this.id, hidden: this.#hidden() });
    else if (!this.#hidden() && this.#leaderHidden) this.#queue(true);
  }

  // ── what the device calls ─────────────────────────────────────────────

  /** Reopen every feed: here when leading, at the leader otherwise. */
  reconnect(): Promise<void> {
    if (this.#leading || this.#closed) return this.#manager.reconnect();
    const req = newTabId();
    return new Promise<void>((resolve) => {
      this.#pending.set(req, resolve);
      this.#post({ t: 'reconnect', from: this.id, req });
    });
  }

  nudge(): void {
    if (this.#leading || this.#closed) this.#manager.nudge();
    else this.#post({ t: 'nudge', from: this.id });
  }

  /** Leave the tabs. Call after the manager has stopped. */
  close(): void {
    if (this.#closed) return;
    this.#post({ t: 'bye', from: this.id });
    this.#closed = true;
    for (const t of this.#idleTimers.values()) clearTimeout(t);
    this.#idleTimers.clear();
    for (const resolve of this.#pending.values()) resolve();
    this.#pending.clear();
    this.#offVisibility?.();
    this.#offPoison?.();
    this.#manager._attach(null);
    this.#letGo?.();
    this.#letGo = null;
    this.#endTab?.();
    this.#endTab = null;
    this.#closing.abort(new DOMException('closed', 'AbortError'));
    this.#channel.onmessage = null;
    this.#channel.close();
  }

  #post(m: Message): void {
    if (this.#closed) return;
    try {
      this.#channel.postMessage(m);
    } catch {
      // A closed channel: nobody is listening.
    }
  }
}
