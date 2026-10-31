/**
 * The sync manager (`plan-client-cache.md` C5).
 *
 * {@link syncOp} keeps one op in sync for as long as its feed stays open. The
 * manager is what an app actually holds: it declares the ops a device syncs,
 * keeps each one connected, reconnects when a feed ends, and publishes one
 * view of where the device stands.
 *
 * Three rules it exists to enforce:
 *
 * - **Deltas apply atomically per position.** A frame's rows all reach the
 *   replica before the manager publishes the position they arrived at, so a
 *   screen reading at the published position never sees half of a frame.
 * - **Screens advance only at a common position.** The view's position is the
 *   lowest position any live op has reached, so a screen drawn from two
 *   synced ops never mixes one op's newer rows with another's older ones.
 * - **The position never moves backwards.** The replica already refuses to
 *   replace a version with one read at a lower position (C3); the view keeps
 *   the same promise at the screen level, including across a reconnect, whose
 *   reload starts at whatever position the server is at now.
 *
 * A reconnect runs a fresh load (ruled 2026-09-15, `feed-resume`), which is
 * also how a device catches up on deletes it missed while it was away: a
 * completed load releases the rows the op held and the load left out.
 */

import type { OrdoClient, Params } from '../core/client.js';
import { RECONNECT_CLASSES, reconnectDelayMs } from '../core/busy.js';
import { OrdoNetworkError } from '../core/network.js';
import { OrdoRefusal } from '../core/refusal.js';
import type { RefusalClass } from '../core/refusal.js';
import type { Refine } from '../core/refine.js';
import type { OpSpec } from '../core/spec.js';
import type { LocalReplica, ReplicaRecord } from './replica.js';
import { syncRefine } from '../core/client.js';
import { syncOp, type SyncDelta, type SyncState } from './sync.js';

/** One op a device syncs, with the parameters it is synced at. */
export interface SyncDeclaration {
  readonly spec: OpSpec;
  readonly params?: Params;
  /**
   * Sync only the rows this `where` matches (R6). Two declarations of one op
   * with two filters are two holders over one row store. `pick` is refused.
   */
  readonly refine?: Refine;
}

/** Where one declared op stands. */
export interface ManagedOp extends SyncState {
  /** The manager's handle for this declaration: its op, parameters and refinement. */
  readonly key: string;
  /** Connections that have failed in a row; zero once one succeeds. */
  readonly attempts: number;
  /**
   * The refusal that ended this op for good, when the door's answer is a
   * standing one. Re-declare the op to try again, once whatever it named has
   * changed.
   */
  readonly stopped: OrdoRefusal | null;
  /**
   * Why the last connection got no answer, when it did not: a timeout, an
   * unreachable door or a blocked origin. `null` while the feed is working,
   * and once a frame arrives.
   */
  readonly network?: OrdoNetworkError | null;
}

/** Where the device stands, after the last frame any op applied. */
export interface SyncView {
  /**
   * The position every live op has reached. Null until they all have one.
   * It never decreases, and it does not advance past an op that is behind.
   */
  readonly position: number | null;
  /** True when every live op has finished a load, so no answer is partial. */
  readonly complete: boolean;
  readonly ops: readonly ManagedOp[];
}

export interface SyncManagerOptions {
  /** Called after every change to the view. */
  readonly onView?: (view: SyncView) => void;
  /** How long to wait before a reconnect, given the consecutive failures. */
  readonly backoff?: (attempts: number) => number;
  /** How the manager waits. Replaced in tests; defaults to a timer. */
  readonly wait?: (ms: number) => Promise<void>;
  /**
   * While a load arrives, an observed handle's snapshot is published at most
   * once per this many milliseconds; 100 by default, 0 for every frame. The
   * first rows, the frame that completes a load, a change after it, and a
   * stop publish at once (`plan-client-loading-fixes.md` §1.2, Part 4).
   */
  readonly publishMs?: number;
  /**
   * What tells the manager the browser has its network back. It reconnects
   * every feed (see {@link SyncManager.nudge}) when this target gets an
   * `online` event. Defaults to the page (`globalThis`) where that can take
   * events; pass `null` to turn it off.
   */
  readonly online?: EventTarget | null;
  /**
   * How often the replica's last-used time of each live op is written again,
   * in milliseconds; one hour by default. An op is noted when it is declared
   * and when it goes idle, and every this often while it runs, so a device
   * opened elsewhere never erases what an open tab still uses
   * (`plan-sdk-device.md` §6b). `Infinity` turns the refresh off.
   */
  readonly touchEveryMs?: number;
}

/**
 * How one declared op's connection runs when it is not the manager's own
 * feed: in a tab that follows another (`tabs.ts`). It resolves or rejects
 * when the connection ends; an abort of `signal` ends it.
 * @internal
 */
export type FeedSource = (
  declared: { readonly key: string; readonly spec: OpSpec; readonly params: Params; readonly refine: Refine | undefined },
  signal: AbortSignal,
) => Promise<void>;

/**
 * The tab link's hold on a manager (`plan-sdk-device.md` §7a).
 * @internal
 */
export interface ManagerLink {
  /** Where connections come from: `null` for the manager's own feeds. */
  source: FeedSource | null;
  /** A screen in this tab declared `key` through {@link SyncManager.sync}. */
  synced(key: string): void;
  /** This tab's screens let `key` go. False keeps the feed: another tab uses it. */
  idling(key: string): boolean;
  /** A frame of this manager's own feed was applied. */
  frame(key: string, state: SyncState, delta: SyncDelta): void;
  /** This manager's own feed for `key` was refused for good. */
  stopped(key: string, refusal: OrdoRefusal): void;
}

/** {@link SyncManagerOptions.touchEveryMs} when the app sets none: one hour. */
export const DEFAULT_TOUCH_EVERY_MS = 3_600_000;

/**
 * The rows one observed handle holds, and where it stands, taken together:
 * both come from the same frames (`plan-client-loading-fixes.md` §1.2).
 */
export interface ObservedRows {
  /** The visible rows the handle holds, by id. Every frame it covers is whole. */
  readonly rows: readonly ReplicaRecord[];
  /** Where the handle stands as of those rows; `null` when it is not declared. */
  readonly state: ManagedOp | null;
  /** False until the first fill from the store has finished. */
  readonly filled: boolean;
  /** Counts this handle's snapshots: each publish is one more. */
  readonly version: number;
  /**
   * What changed since the snapshot numbered `from`, the one published just
   * before: the records that left (a changed row's old version among them)
   * and the ones that came (its new version). A reader that kept an answer
   * built from snapshot `from` brings it up to date with these alone; any
   * other reader starts again from {@link rows}. `null` on the first.
   */
  readonly changes: {
    readonly from: number;
    readonly added: readonly ReplicaRecord[];
    readonly removed: readonly ReplicaRecord[];
  } | null;
}

/** One {@link SyncManager.observe} call. */
export interface Observation {
  /** The last published snapshot: the same object until the next publish. */
  snapshot(): ObservedRows;
  /** Stop observing. A handle nobody observes keeps nothing in memory. */
  stop(): void;
}

/** One observed handle: its rows in memory, and what changed since the last publish. */
interface Observed {
  readonly key: string;
  readonly listeners: Set<(rows: ObservedRows) => void>;
  /** The rows this handle holds, by reference (`[entity, id]`). */
  readonly members: Set<string>;
  filling: boolean;
  /** Deltas that arrived while the first fill read the store, applied after it. */
  buffer: { readonly holder: string; readonly delta: SyncDelta }[];
  readonly added: Set<string>;
  readonly removed: Set<string>;
  readonly updated: Set<string>;
  dirty: boolean;
  /** Counts this handle's own frames applied, so a late store read can tell it is stale. */
  frames: number;
  snapshot: ObservedRows;
  /** The reference of each row in `snapshot.rows`, in the same order. */
  order: string[];
}

/**
 * Refusals worth another connection: `internal`, and the classes every
 * client retries ({@link RETRY_CLASSES}: `server_busy`, `tenant_not_ready`,
 * `serialization_conflict`), so a busy door backs off and resumes instead of
 * stopping the op until a reload. Everything else the door names is a
 * standing answer to the same request, so repeating it only adds load: the op
 * stops and carries the refusal in its {@link ManagedOp.stopped}.
 */
const RETRYABLE: ReadonlySet<string> = RECONNECT_CLASSES;

/**
 * Refusals that mean the device may no longer hold what this op synced
 * (`plan-client-cache.md` §6: "a refusal purges what that op synced").
 *
 * Deliberately narrow. `not_found` and `unknown_op` are not here: an op
 * renamed in a deploy would otherwise throw away rows the caller is still
 * entitled to. These three are the door's own answers to "you may not have
 * this", and `access_denied` is the one revocation itself arrives as, since
 * the door answers uniformly so a token cannot enumerate.
 */
const REVOKING: ReadonlySet<string> = new Set<RefusalClass>([
  'access_denied',
  'access_guard',
  'delegation_required',
]);

/** 0, then 250 ms doubling to 30 s: the one reconnect backoff. A clean end reconnects at once. */
function defaultBackoff(attempts: number): number {
  return reconnectDelayMs(attempts);
}

interface Entry {
  readonly key: string;
  readonly spec: OpSpec;
  readonly params: Params;
  readonly refine: Refine | undefined;
  controller: AbortController;
  /** The connection now open or being opened; aborted by `nudge` to reconnect at once. */
  attempt: AbortController;
  /** Ends the wait before a reconnect, when there is one. */
  wake: (() => void) | null;
  state: SyncState;
  attempts: number;
  /** The wait the door asked for on its last refusal (`Retry-After`), in ms. */
  retryAfterMs: number | null;
  stopped: OrdoRefusal | null;
  network: OrdoNetworkError | null;
  /**
   * The token was renewed after a refusal and no frame has arrived since.
   * A second refusal then stands, so a door that refuses every token cannot
   * be asked in a loop.
   */
  renewed: boolean;
  task: Promise<void>;
  /** Connections begun so far, counting from 1 for the first. */
  connections: number;
  /**
   * Waiters for a connection after a given one to have answered: its first
   * frame, or its end. {@link SyncManager.reconnect} waits on these.
   */
  waiters: { readonly after: number; readonly resolve: () => void }[];
}

export class SyncManager {
  readonly #client: OrdoClient;
  readonly #replica: LocalReplica;
  readonly #backoff: (attempts: number) => number;
  readonly #wait: (ms: number) => Promise<void>;
  readonly #entries = new Map<string, Entry>();
  readonly #watchers = new Set<(view: SyncView) => void>();
  /** The last common position published. It only ever rises. */
  #position: number | null = null;
  #running = true;
  /** Observed handles, by key (`observe`). Only these keep rows in memory. */
  readonly #observed = new Map<string, Observed>();
  /** One row per reference, shared by every observed handle that holds it. */
  readonly #rows = new Map<string, ReplicaRecord>();
  /** How many observed handles hold each row in `#rows`. */
  readonly #holds = new Map<string, number>();
  readonly #publishMs: number;
  #lastFlush = -Infinity;
  #flushTimer: ReturnType<typeof setTimeout> | null = null;
  #unpoison: (() => void) | null = null;
  #offOnline: (() => void) | null = null;
  #touchTimer: ReturnType<typeof setInterval> | null = null;
  #link: ManagerLink | null = null;
  /** The reconnect under way, shared by every call made while it runs. */
  #reconnecting: Promise<void> | null = null;
  /** Resolved on stop, so a waiting reconnect gives up straight away. */
  #stopped!: () => void;
  readonly #whenStopped = new Promise<void>((resolve) => {
    this.#stopped = resolve;
  });

  private constructor(client: OrdoClient, replica: LocalReplica, options: SyncManagerOptions) {
    this.#client = client;
    this.#replica = replica;
    this.#backoff = options.backoff ?? defaultBackoff;
    this.#wait = options.wait ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
    this.#publishMs = options.publishMs ?? 100;
    if (options.onView) this.#watchers.add(options.onView);
    const target =
      options.online === undefined
        ? typeof globalThis.addEventListener === 'function'
          ? (globalThis as unknown as EventTarget)
          : null
        : options.online;
    const every = options.touchEveryMs ?? DEFAULT_TOUCH_EVERY_MS;
    if (Number.isFinite(every) && every > 0) {
      this.#touchTimer = setInterval(() => {
        for (const key of this.#entries.keys()) this.#touch(key);
      }, every);
      // A Node process is not kept alive by it.
      (this.#touchTimer as { unref?: () => void }).unref?.();
    }
    if (target) {
      const onOnline = () => this.nudge();
      target.addEventListener('online', onOnline);
      this.#offOnline = () => target.removeEventListener('online', onOnline);
    }
  }

  /** Start syncing `declarations`. Syncing is on by default (ruled 2026-09-14). */
  static start(
    client: OrdoClient,
    replica: LocalReplica,
    declarations: readonly SyncDeclaration[] = [],
    options: SyncManagerOptions = {},
  ): SyncManager {
    const manager = new SyncManager(client, replica, options);
    for (const d of declarations) manager.sync(d.spec, d.params, d.refine);
    return manager;
  }

  /**
   * Declare one more op. Declaring the same op, parameters and refinement
   * twice is one sync. A refinement's `where` narrows what the device holds
   * for this declaration; a `pick` is refused here, before any connection.
   */
  sync(spec: OpSpec, params: Params = {}, refine?: Refine): string {
    const key = this._sync(spec, params, refine);
    this.#link?.synced(key);
    return key;
  }

  /** {@link sync} for the tab link: another tab's declaration, not this tab's. @internal */
  _sync(spec: OpSpec, params: Params = {}, refine?: Refine): string {
    // Checked now, so a refused refinement throws at the call that declared
    // it rather than stopping a background feed.
    const where = syncRefine(spec, refine);
    const narrowed = where === null ? undefined : (JSON.parse(where) as Refine);
    const key = handle(spec, params, where);
    const existing = this.#entries.get(key);
    if (existing) return existing.key;
    const entry: Entry = {
      key,
      spec,
      params,
      refine: narrowed,
      controller: new AbortController(),
      attempt: new AbortController(),
      wake: null,
      state: { op: spec.op, loaded: 0, complete: false, position: null, pin: null },
      attempts: 0,
      retryAfterMs: null,
      stopped: null,
      network: null,
      renewed: false,
      task: Promise.resolve(),
      connections: 0,
      waiters: [],
    };
    this.#entries.set(key, entry);
    this.#touch(key);
    entry.task = this.#run(entry);
    this.#publish();
    return key;
  }

  /**
   * Stop syncing one op and release what it held, so rows no other synced op
   * holds leave the device.
   */
  async drop(key: string): Promise<void> {
    const entry = this.#entries.get(key);
    if (!entry) return;
    this.#entries.delete(key);
    answered(entry, Infinity);
    // A dropped handle shows nothing from now on, before its release lands.
    this.#clearObserved(key);
    entry.controller.abort(new DOMException('dropped', 'AbortError'));
    await entry.task;
    await this.#releaseHeld(entry);
    this.#publish();
  }

  /**
   * Stop syncing one op for now, because nothing on screen uses it
   * (`plan-sdk-device.md` §6). Its feed ends and it leaves the view, so
   * `complete` and the common position stop waiting on it.
   *
   * Unlike {@link drop}, the rows it held stay on the device, with the
   * stored result hash. Every tab on one replica uses the same holder key,
   * so releasing here would empty another tab's list. Declaring it again
   * with {@link sync} resumes on that hash: one server read, and no rows
   * moved when nothing changed. Rows in memory go once no screen observes
   * the handle, as always.
   */
  async idle(key: string): Promise<void> {
    if (this.#link && !this.#link.idling(key)) return;
    await this._idle(key);
  }

  /** {@link idle} for the tab link, whoever else uses the key. @internal */
  async _idle(key: string): Promise<void> {
    const entry = this.#entries.get(key);
    if (!entry) return;
    this.#entries.delete(key);
    answered(entry, Infinity);
    // The retention period counts from the last moment it was used.
    this.#touch(key);
    entry.controller.abort(new DOMException('idle', 'AbortError'));
    await entry.task;
    this.#publish();
  }

  /** Attach or detach the tab link. @internal */
  _attach(link: ManagerLink | null): void {
    this.#link = link;
  }

  /** The keys declared now, with what declared them. @internal */
  _declared(): { readonly key: string; readonly spec: OpSpec; readonly params: Params; readonly refine: Refine | undefined }[] {
    return [...this.#entries.values()].map((e) => ({ key: e.key, spec: e.spec, params: e.params, refine: e.refine }));
  }

  /** Where `key` stands, once it has a position. @internal */
  _state(key: string): SyncState | null {
    const e = this.#entries.get(key);
    return e && e.state.position !== null ? e.state : null;
  }

  /**
   * A frame another tab's manager applied to the shared replica. It is in
   * the store already; here it reaches memory and the view, exactly as one
   * of this manager's own frames would. @internal
   */
  _remoteFrame(key: string, state: SyncState, delta: SyncDelta): void {
    if (!this.#running) return;
    const entry = this.#entries.get(key);
    if (entry && this.#link?.source) {
      entry.network = null;
      this.#applied(entry, state, delta);
      answered(entry, entry.connections);
      return;
    }
    this.#absorb(key, delta);
    this.#flush(false);
  }

  /** Where the device stands now. */
  get view(): SyncView {
    return this.#view();
  }

  /** Watch the view. Returns the function that stops watching. */
  watch(listener: (view: SyncView) => void): () => void {
    this.#watchers.add(listener);
    return () => this.#watchers.delete(listener);
  }

  /**
   * Resolves at the first view where every live op has loaded and they share
   * a position, which is the first moment a screen can be drawn from the
   * device alone.
   */
  settled(): Promise<SyncView> {
    const now = this.#view();
    if (settledView(now)) return Promise.resolve(now);
    return new Promise((resolve) => {
      const stop = this.watch((view) => {
        if (!settledView(view)) return;
        stop();
        resolve(view);
      });
    });
  }

  /**
   * Keep the rows of `key` in memory and hear when they change (H1).
   *
   * The first observer of a handle fills it from the store in one read of
   * its holds and one of their rows. From then on each frame's delta is
   * applied in memory, in the frame's own callback, before the view is
   * published, so a snapshot never holds part of a frame and the store is
   * never read again for this handle. A row another observed handle holds is
   * updated there too when a frame of this one changes it.
   *
   * The listener is called with each new snapshot. Snapshots are paced while
   * a load arrives (`publishMs`); every observer touched by one frame is
   * published in the same call.
   */
  observe(key: string, listener: (rows: ObservedRows) => void): Observation {
    let o = this.#observed.get(key);
    if (!o) {
      o = {
        key,
        listeners: new Set(),
        members: new Set(),
        filling: true,
        buffer: [],
        added: new Set(),
        removed: new Set(),
        updated: new Set(),
        dirty: false,
        frames: 0,
        snapshot: { rows: [], state: this.#stateOf(key), filled: false, version: 0, changes: null },
        order: [],
      };
      this.#observed.set(key, o);
      this.#unpoison ??= this.#replica.onPoison(() => this.#poisoned());
      void this.#fill(o);
    }
    const observed = o;
    observed.listeners.add(listener);
    return {
      snapshot: () => observed.snapshot,
      stop: () => {
        observed.listeners.delete(listener);
        if (observed.listeners.size > 0 || this.#observed.get(key) !== observed) return;
        this.#observed.delete(key);
        for (const ref of observed.members) this.#unhold(ref);
        observed.members.clear();
        if (this.#observed.size === 0) {
          this.#unpoison?.();
          this.#unpoison = null;
        }
      },
    };
  }

  /**
   * Reconnect every feed now. A feed waiting to retry stops waiting, and one
   * that is open is closed and opened again, which runs a fresh load. Call it
   * when the app has reason to think the connection is stale. The manager
   * calls it itself when the browser says it is back online, so an app needs
   * no timer, and no event of its own, for that. A feed the door refused for
   * good stays stopped.
   */
  nudge(): void {
    if (!this.#running) return;
    for (const entry of this.#entries.values()) {
      if (entry.stopped !== null) continue;
      if (entry.wake) entry.wake();
      else entry.attempt.abort(new DOMException('nudged', 'AbortError'));
    }
  }

  /**
   * Close every feed and open it again, in place. Each feed is reopened only
   * after the old one has ended, so one handle never has two feeds and the
   * replica has one writer in this tab. The manager, its rows in memory and
   * its watchers stay the same objects, so screens keep what they show while
   * the new load arrives. The new connection offers the stored result hash,
   * so an unchanged result moves no rows.
   *
   * Resolves when every feed has reopened: its new connection sent its first
   * frame, or failed. Calls while one is under way share it and open one
   * feed per handle. A feed the door refused for good stays stopped, and the
   * manager stays revoked: only a new device clears that.
   */
  reconnect(): Promise<void> {
    if (!this.#running) return Promise.resolve();
    if (this.#reconnecting) return this.#reconnecting;
    const waits: Promise<void>[] = [];
    for (const entry of this.#entries.values()) {
      if (entry.stopped !== null) continue;
      const after = entry.connections;
      waits.push(new Promise<void>((resolve) => entry.waiters.push({ after, resolve })));
      if (entry.wake) entry.wake();
      else entry.attempt.abort(new DOMException('reconnect', 'AbortError'));
    }
    const all = Promise.all(waits).then(() => {
      if (this.#reconnecting === all) this.#reconnecting = null;
    });
    this.#reconnecting = all;
    return all;
  }

  /** Stop every sync. The replica keeps what it holds. */
  async stop(): Promise<void> {
    if (!this.#running) return;
    this.#running = false;
    this.#offOnline?.();
    this.#offOnline = null;
    if (this.#touchTimer !== null) clearInterval(this.#touchTimer);
    this.#touchTimer = null;
    this.#stopped();
    for (const entry of this.#entries.values()) {
      entry.controller.abort(new DOMException('stopped', 'AbortError'));
      answered(entry, Infinity);
    }
    await Promise.all([...this.#entries.values()].map((e) => e.task));
    // What a paced publish still owed goes out now; nothing arrives after.
    this.#flush(true);
  }

  /** Note `key` in use on the replica. A failure costs nothing but the note. */
  #touch(key: string): void {
    void this.#replica.touch(key).catch(() => undefined);
  }

  /** One op's feed, reopened for as long as the manager runs. */
  async #run(entry: Entry): Promise<void> {
    while (this.#running && this.#entries.get(entry.key) === entry) {
      let applied = false;
      let nudged = false;
      // One connection's own signal, so a nudge ends it and nothing else.
      const attempt = new AbortController();
      entry.attempt = attempt;
      entry.connections += 1;
      const connection = entry.connections;
      const onEnd = () => attempt.abort(entry.controller.signal.reason);
      entry.controller.signal.addEventListener('abort', onEnd, { once: true });
      try {
        const source = this.#link?.source ?? null;
        if (source) {
          await source({ key: entry.key, spec: entry.spec, params: entry.params, refine: entry.refine }, attempt.signal);
          applied = true;
        } else {
          await syncOp(this.#client, this.#replica, entry.spec, entry.params, {
            signal: attempt.signal,
            holder: entry.key,
            ...(entry.refine ? { refine: entry.refine } : {}),
            onFrame: (state, delta) => {
              applied = true;
              entry.network = null;
              entry.renewed = false;
              this.#applied(entry, state, delta);
              answered(entry, connection);
              this.#link?.frame(entry.key, state, delta);
            },
          });
        }
        // The server closed the stream. A connection that delivered frames is
        // working, so the next one is immediate. One that delivered none
        // counts as a failure, or a door that closes every stream at once
        // would be reopened in a tight loop.
        entry.attempts = applied ? 0 : entry.attempts + 1;
      } catch (error) {
        if (!this.#running || entry.controller.signal.aborted) return;
        // A refused token may only be stale: a guest's window token names
        // one grant, and a second accept replaces it while the window stays
        // open. The client mints a new one if the door will give it, once
        // per refusal with no frame between.
        const renewal =
          !attempt.signal.aborted &&
          error instanceof OrdoRefusal &&
          error.detail.class === 'access_denied' &&
          !entry.renewed
            ? await this.#renew()
            : null;
        if (!this.#running || entry.controller.signal.aborted) return;
        if (attempt.signal.aborted || renewal === 'renewed') {
          // A nudge, or a new token: reconnect at once, as after a clean end.
          nudged = true;
          entry.attempts = 0;
          if (renewal === 'renewed') entry.renewed = true;
        } else if (renewal instanceof OrdoNetworkError) {
          // The door could not be asked for a new token: try again later,
          // as for any feed that could not reach it.
          entry.network = renewal;
          entry.attempts += 1;
        } else if (error instanceof OrdoRefusal && !RETRYABLE.has(error.detail.class)) {
          entry.stopped = error;
          this.#link?.stopped(entry.key, error);
          // The rows this op put on the device were the caller's only while
          // the door allowed the op. It no longer does, so they go now
          // rather than at the next reload that may never come. Rows another
          // synced op still holds survive, because `release` removes a row
          // only when no op holds it.
          if (REVOKING.has(error.detail.class)) {
            // The screen empties at once; the release on disk follows.
            this.#clearObserved(entry.key);
            await this.#releaseHeld(entry);
          }
          this.#publish();
          return;
        } else {
          if (error instanceof OrdoNetworkError) entry.network = error;
          if (error instanceof OrdoRefusal) entry.retryAfterMs = error.retryAfterMs;
          entry.attempts += 1;
        }
      } finally {
        entry.controller.signal.removeEventListener('abort', onEnd);
        // This connection is over. A reconnect waiting on it to answer was
        // waiting on the one before: the next one answers for it, unless the
        // op stopped or left, which ends the wait here.
        if (!this.#running || entry.stopped !== null || this.#entries.get(entry.key) !== entry) {
          answered(entry, Infinity);
        } else if (!attempt.signal.aborted || entry.controller.signal.aborted) {
          answered(entry, connection);
        }
      }
      if (!this.#running || this.#entries.get(entry.key) !== entry) return;
      // The next connection reloads from the start, so the op is loading
      // again. Its rows stay readable at the positions they were read at.
      entry.state = { ...entry.state, loaded: 0, complete: false };
      this.#publish();
      // A busy door's own `Retry-After` is a floor under the backoff.
      const asked = entry.retryAfterMs ?? 0;
      entry.retryAfterMs = null;
      if (!nudged) await this.#pause(entry, Math.max(this.#backoff(entry.attempts), asked));
    }
  }

  /**
   * Ask the client for a new token after a refusal: `renewed`, `refused`
   * (the refusal stands), or the network failure that kept the door from
   * answering. Anything else thrown is a refusal too.
   */
  async #renew(): Promise<'renewed' | 'refused' | OrdoNetworkError> {
    try {
      return (await this.#client.renewToken()) ? 'renewed' : 'refused';
    } catch (e) {
      return e instanceof OrdoNetworkError ? e : 'refused';
    }
  }

  /**
   * Release everything one op holds, at the last position it reached. Shared
   * by {@link drop} and by a refusal that revokes the op.
   */
  async #releaseHeld(entry: Entry): Promise<void> {
    const position = entry.state.position ?? this.#position;
    if (position === null) return;
    // One write for all of them, as a frame is.
    await this.#replica.applyFrame(entry.key, { position, releases: await this.#replica.holdings(entry.key) });
  }

  #applied(entry: Entry, state: SyncState, delta: SyncDelta): void {
    const wasComplete = entry.state.complete;
    entry.state = state;
    entry.attempts = 0;
    // The frame is in memory before anyone hears of it.
    this.#absorb(entry.key, delta);
    // The completing frame, and a change after a load, publish at once.
    this.#publish(state.complete && (!wasComplete || !delta.reset));
  }

  // ── rows in memory for observed handles ──────────────────────────────

  #stateOf(key: string): ManagedOp | null {
    const e = this.#entries.get(key);
    return e ? { ...e.state, key: e.key, attempts: e.attempts, stopped: e.stopped, network: e.network } : null;
  }

  /** Apply one frame's delta to every observed handle; a filling one keeps it for later. */
  #absorb(holder: string, delta: SyncDelta): void {
    if (this.#observed.size === 0) return;
    for (const o of this.#observed.values()) {
      if (o.filling) o.buffer.push({ holder, delta });
      else this.#applyTo(o, holder, delta);
    }
  }

  /**
   * One delta's effect on one observed handle: its own frames add and remove
   * its rows; any frame updates a row it holds. A row is replaced only by a
   * version at an equal or higher position (C3, as the replica does).
   */
  #applyTo(o: Observed, holder: string, delta: SyncDelta): void {
    if (holder === o.key) o.frames += 1;
    for (const u of delta.upserts) {
      const ref = refOf(u.entity, u.id);
      if (holder === o.key) {
        if (u.row === null) {
          // A newer removal holds the row's place: it is not visible.
          this.#leave(o, ref);
          continue;
        }
        if (!o.members.has(ref)) this.#join(o, ref, { entity: u.entity, id: u.id, position: u.position, row: u.row });
      }
      if (u.row !== null && o.members.has(ref)) {
        this.#offer(ref, { entity: u.entity, id: u.id, position: u.position, row: u.row });
      }
    }
    if (holder === o.key) for (const r of delta.releases) this.#leave(o, refOf(r.entity, r.id));
    if (holder === o.key && delta.members) this.#reconcile(o, delta.members);
  }

  /**
   * A load completed: the handle holds exactly `members` now. Memory is kept
   * from this manager's own frames and one fill from the store, and the store
   * has other writers (a manager being replaced, another tab on the same
   * replica). A row one of them released between the fill and this load
   * would otherwise stay on screen with its fields kept current by other
   * handles' frames. It leaves here. A row the store holds and memory lacks
   * is read back from the store.
   */
  #reconcile(o: Observed, members: readonly { readonly entity: string; readonly id: string }[]): void {
    const want = new Set(members.map((m) => refOf(m.entity, m.id)));
    for (const ref of [...o.members]) if (!want.has(ref)) this.#leave(o, ref);
    const missing = [...want].filter((ref) => !o.members.has(ref));
    if (missing.length === 0) return;
    const at = o.frames;
    void this.#replica.held(o.key).then(
      (records) => {
        // A frame of this handle since then may have released one of them;
        // the next completed load reconciles again.
        if (this.#observed.get(o.key) !== o || this.#replica.poisoned || o.frames !== at) return;
        const wanted = new Set(missing);
        for (const r of records) {
          const ref = refOf(r.entity, r.id);
          if (wanted.has(ref) && !o.members.has(ref)) this.#join(o, ref, r);
        }
        this.#flush(true);
      },
      () => undefined,
    );
  }

  /** A newer version of a row held in memory reaches every handle that holds it. */
  #offer(ref: string, record: ReplicaRecord): void {
    const current = this.#rows.get(ref);
    if (!current || current === record) return;
    if (current.position > record.position) return;
    if (current.position === record.position && current.row === record.row) return;
    this.#rows.set(ref, record);
    for (const other of this.#observed.values()) {
      if (other.filling || !other.members.has(ref)) continue;
      if (!other.added.has(ref)) other.updated.add(ref);
      other.dirty = true;
    }
  }

  #join(o: Observed, ref: string, record: ReplicaRecord): void {
    o.members.add(ref);
    o.added.add(ref);
    o.dirty = true;
    this.#holds.set(ref, (this.#holds.get(ref) ?? 0) + 1);
    const current = this.#rows.get(ref);
    if (!current || current.position <= record.position) this.#rows.set(ref, record);
  }

  #leave(o: Observed, ref: string): void {
    if (!o.members.delete(ref)) return;
    o.added.delete(ref);
    o.updated.delete(ref);
    o.removed.add(ref);
    o.dirty = true;
    this.#unhold(ref);
  }

  #unhold(ref: string): void {
    const n = (this.#holds.get(ref) ?? 0) - 1;
    if (n > 0) {
      this.#holds.set(ref, n);
    } else {
      this.#holds.delete(ref);
      this.#rows.delete(ref);
    }
  }

  /** The first fill: the store once, then whatever frames arrived meanwhile. */
  async #fill(o: Observed): Promise<void> {
    let records: ReplicaRecord[] = [];
    try {
      records = await this.#replica.held(o.key);
    } catch {
      records = [];
    }
    if (this.#observed.get(o.key) !== o || !o.filling) return;
    o.filling = false;
    if (!this.#replica.poisoned) {
      for (const r of records) this.#join(o, refOf(r.entity, r.id), r);
      for (const { holder, delta } of o.buffer) this.#applyTo(o, holder, delta);
    }
    o.buffer = [];
    o.dirty = true;
    this.#flush(true);
  }

  /** Empty one handle's snapshot at once: dropped, or revoked. */
  #clearObserved(key: string): void {
    const o = this.#observed.get(key);
    if (!o) return;
    for (const ref of [...o.members]) this.#leave(o, ref);
    o.buffer = [];
    o.dirty = true;
    this.#flush(true);
  }

  /**
   * The replica was poisoned: every row in memory goes, synchronously, before
   * the purge starts, and every observer is told (C7).
   */
  #poisoned(): void {
    this.#rows.clear();
    this.#holds.clear();
    for (const o of this.#observed.values()) {
      for (const ref of o.members) o.removed.add(ref);
      o.members.clear();
      o.added.clear();
      o.updated.clear();
      o.buffer = [];
      o.filling = false;
      o.dirty = true;
    }
    this.#flush(true);
  }

  /**
   * Publish every observed handle that changed, paced: at once when `urgent`
   * or when `publishMs` has passed since the last, otherwise on a timer. All
   * of them are rebuilt before any listener runs, so two screens never show
   * two different frames of one op.
   */
  #flush(urgent: boolean): void {
    if (this.#observed.size === 0) return;
    for (const o of this.#observed.values()) {
      if (o.filling) continue;
      // A state change alone (a reconnect, a stop) is a change too.
      const now = this.#stateOf(o.key);
      if (!sameState(o.snapshot.state, now)) {
        o.dirty = true;
        if (now?.stopped && !o.snapshot.state?.stopped) urgent = true;
      }
      // The first rows go out at once.
      if (o.dirty && o.snapshot.rows.length === 0 && o.members.size > 0) urgent = true;
    }
    const at = Date.now();
    if (!urgent && at - this.#lastFlush < this.#publishMs) {
      this.#flushTimer ??= setTimeout(() => {
        this.#flushTimer = null;
        this.#flush(true);
      }, this.#publishMs - (at - this.#lastFlush));
      return;
    }
    if (this.#flushTimer !== null) {
      clearTimeout(this.#flushTimer);
      this.#flushTimer = null;
    }
    this.#lastFlush = at;
    const told: Observed[] = [];
    for (const o of this.#observed.values()) {
      if (o.filling || !o.dirty) continue;
      const from = o.snapshot.version;
      const changes = { from, added: [] as ReplicaRecord[], removed: [] as ReplicaRecord[] };
      const [rows, order] = this.#rebuild(o, changes);
      o.snapshot = { rows, state: this.#stateOf(o.key), filled: true, version: from + 1, changes };
      o.order = order;
      o.added.clear();
      o.removed.clear();
      o.updated.clear();
      o.dirty = false;
      told.push(o);
    }
    for (const o of told) for (const l of [...o.listeners]) l(o.snapshot);
  }

  /**
   * The handle's rows by id, from the last published array: removed rows
   * drop out and updated ones are replaced in one pass, and the added rows,
   * sorted, are merged in. Unchanged rows keep their object.
   */
  #rebuild(o: Observed, changes: { added: ReplicaRecord[]; removed: ReplicaRecord[] }): [ReplicaRecord[], string[]] {
    const prev = o.snapshot.rows;
    let kept: ReplicaRecord[] = [];
    let keptRefs: string[] = [];
    if (o.removed.size === 0 && o.updated.size === 0) {
      kept = prev.slice();
      keptRefs = o.order.slice();
    } else {
      for (let i = 0; i < prev.length; i++) {
        const ref = o.order[i]!;
        if (o.removed.has(ref) || !o.members.has(ref)) {
          changes.removed.push(prev[i]!);
          continue;
        }
        const now = o.updated.has(ref) ? (this.#rows.get(ref) ?? prev[i]!) : prev[i]!;
        if (now !== prev[i]) {
          changes.removed.push(prev[i]!);
          changes.added.push(now);
        }
        kept.push(now);
        keptRefs.push(ref);
      }
    }
    const fresh: [ReplicaRecord, string][] = [];
    for (const ref of o.added) {
      const r = this.#rows.get(ref);
      if (r && o.members.has(ref)) {
        fresh.push([r, ref]);
        changes.added.push(r);
      }
    }
    if (fresh.length === 0) return [kept, keptRefs];
    fresh.sort((a, b) => byId(a[0], b[0]));
    const total = kept.length + fresh.length;
    const out = new Array<ReplicaRecord>(total);
    const refs = new Array<string>(total);
    let i = 0;
    let j = 0;
    let k = 0;
    while (i < kept.length && j < fresh.length) {
      if (byId(kept[i]!, fresh[j]![0]) <= 0) {
        refs[k] = keptRefs[i]!;
        out[k++] = kept[i++]!;
      } else {
        refs[k] = fresh[j]![1];
        out[k++] = fresh[j++]![0];
      }
    }
    while (i < kept.length) {
      refs[k] = keptRefs[i]!;
      out[k++] = kept[i++]!;
    }
    while (j < fresh.length) {
      refs[k] = fresh[j]![1];
      out[k++] = fresh[j++]![0];
    }
    return [out, refs];
  }

  /** Waits, unless the manager stops first. */
  async #pause(entry: Entry, ms: number): Promise<void> {
    if (!this.#running) return;
    const woken = new Promise<void>((resolve) => {
      entry.wake = resolve;
    });
    try {
      await Promise.race([this.#wait(ms), this.#whenStopped, woken]);
    } finally {
      entry.wake = null;
    }
  }

  #view(): SyncView {
    const ops: ManagedOp[] = [...this.#entries.values()].map((e) => ({
      ...e.state,
      key: e.key,
      attempts: e.attempts,
      stopped: e.stopped,
      network: e.network,
    }));
    // An op that gave up can never advance, so it is left out of the common
    // position rather than freezing every screen. Its refusal is in `ops`.
    const live = ops.filter((o) => o.stopped === null);
    const positions = live.map((o) => o.position);
    if (live.length > 0 && positions.every((p): p is number => p !== null)) {
      const common = Math.min(...positions);
      this.#position = this.#position === null ? common : Math.max(this.#position, common);
    }
    return {
      position: this.#position,
      complete: live.length > 0 && live.every((o) => o.complete),
      ops,
    };
  }

  #publish(urgent = false): void {
    this.#flush(urgent);
    const view = this.#view();
    for (const watcher of [...this.#watchers]) watcher(view);
  }
}

function refOf(entity: string, id: string): string {
  return JSON.stringify([entity, id]);
}

/** Connection `connection` of `entry` answered: wake whoever waited on one after theirs. */
function answered(entry: Entry, connection: number): void {
  if (entry.waiters.length === 0) return;
  const keep: Entry['waiters'] = [];
  for (const w of entry.waiters) {
    if (connection > w.after) w.resolve();
    else keep.push(w);
  }
  entry.waiters = keep;
}

function byId(a: ReplicaRecord, b: ReplicaRecord): number {
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

function sameState(a: ManagedOp | null, b: ManagedOp | null): boolean {
  if (a === null || b === null) return a === b;
  return (
    a.loaded === b.loaded &&
    a.complete === b.complete &&
    a.position === b.position &&
    a.attempts === b.attempts &&
    a.stopped === b.stopped
  );
}

function settledView(view: SyncView): boolean {
  return view.complete && view.position !== null;
}

/**
 * An op synced at two sets of parameters, or under two refinements, is two
 * syncs, held apart on the device. The refinement is its checked wire form,
 * so two spellings of one filter are one handle. The handle is also the key
 * the replica remembers a result hash under (C4d), so a resume never offers
 * one filter's hash for another's result.
 */
function handle(spec: OpSpec, params: Params, where: string | null): string {
  const names = Object.keys(params).sort();
  const pairs = names.map((n) => `${n}=${String(params[n])}`);
  if (where !== null) pairs.push(`$refine=${where}`);
  if (pairs.length === 0) return spec.op;
  return `${spec.op}?${pairs.join('&')}`;
}
