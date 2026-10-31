/**
 * A ready-made device (`docs/plan-sdk-device.md` §5).
 *
 * Before this, an app built its device by hand: a backend, a replica, a sync
 * manager, the refiners, a restart, a purge and a close. {@link openDevice}
 * does all of it in one call and answers an {@link OrdoDevice}, which
 * `<OrdoProvider device={...}>` in `@hominemai/ordo-react` takes as it is.
 *
 * What it does not do, on purpose (ruled 2026-10-21, decision 2):
 *
 * - **No polling.** It reloads nothing on focus and runs no timer. The
 *   server pushes each change down the open feed. When the browser says it
 *   is back online, the manager reopens its feeds at once ({@link SyncManager.nudge}).
 * - **No restart by swapping managers.** {@link OrdoDevice.reconnect}
 *   reopens every feed on the same manager, so a screen never sees two
 *   managers write to it.
 */

import type { OrdoClient } from '../core/client.js';
import { IndexedDbBackend, type Backend } from './backend.js';
import { SyncManager, type SyncManagerOptions } from './manager.js';
import { refineMerge, refineRows, refineRowsAsync } from './refine.js';
import { LocalReplica } from './replica.js';
import { pageTabs, TabLink, type TabEnvironment } from './tabs.js';

/** The IndexedDB database {@link openDevice} uses when the app names none. */
export const DEFAULT_DEVICE_STORE = 'ordo';

/**
 * How long a sync stays open after the last screen using it goes, in
 * milliseconds: 15 s (owner, 2026-10-21, device plan decision 3).
 */
export const DEFAULT_SYNC_IDLE_MS = 15_000;

/**
 * How long rows held only by a sync nobody used are kept, in milliseconds:
 * 7 days (owner, 2026-10-21, device plan decision 4).
 */
export const DEFAULT_ERASE_AFTER_MS = 7 * 24 * 60 * 60 * 1000;

export interface OpenDeviceOptions {
  /** The client the feeds are opened with: this person's token into `space`. */
  readonly client: OrdoClient;
  /** The tenant. */
  readonly space: string;
  /** The person. One person's rows are sealed apart from another's. */
  readonly principal: string;
  /** Where the rows are kept. Default: IndexedDB, under {@link store}. */
  readonly backend?: Backend;
  /**
   * The IndexedDB database name, when no `backend` is given. Default
   * `'ordo'`. An app moving onto `openDevice` passes its old name, or the
   * device syncs again from empty (nothing is lost: writes are never queued).
   */
  readonly store?: string;
  /** Passed to the sync manager: its backoff, pacing and `online` target. */
  readonly manager?: SyncManagerOptions;
  /**
   * How long a sync stays open after the last screen using it unmounts, in
   * milliseconds. Then its feed closes and its rows stay on the device, so
   * the next screen shows them at once and resumes. Default
   * {@link DEFAULT_SYNC_IDLE_MS} (15 s). `Infinity` keeps every sync open
   * until the device closes.
   */
  readonly idleMs?: number;
  /**
   * Rows held only by syncs unused for this long, in milliseconds, are
   * erased when the device opens. Default {@link DEFAULT_ERASE_AFTER_MS}
   * (7 days). `Infinity` keeps them until sign-out.
   */
  readonly eraseAfterMs?: number;
  /**
   * `'share'`: the tabs on one replica elect a leader, which runs the only
   * feeds and passes each frame to the others, so a person holds one set of
   * feeds however many tabs are open. `'own'`: this tab runs its own feeds,
   * as before. The default is `'share'` when `openDevice` opens IndexedDB
   * itself, and `'own'` when the app passes a `backend`, which other tabs
   * may not see. Without `BroadcastChannel` or `navigator.locks`, every tab
   * runs its own.
   */
  readonly tabs?: 'share' | 'own';
  /**
   * The locks and channel the tabs share. Defaults to the page's own; tests
   * pass stand-ins. `null` is the same as `tabs: 'own'`.
   */
  readonly tabEnvironment?: TabEnvironment | null;
}

/** What {@link openDevice} answers. */
export interface OrdoDevice {
  readonly space: string;
  readonly principal: string;
  readonly replica: LocalReplica;
  readonly manager: SyncManager;
  /** The SDK's refiners, so the hooks can refine rows on the device. */
  readonly refineRows: typeof refineRows;
  readonly refineMerge: typeof refineMerge;
  readonly refineRowsAsync: typeof refineRowsAsync;
  /** How long a sync nobody uses stays open, in milliseconds ({@link OpenDeviceOptions.idleMs}). */
  readonly idleMs: number;
  /** Whether the tabs share feeds (see {@link OpenDeviceOptions.tabs}). */
  readonly tabs: 'share' | 'own';
  /** True while this tab runs the feeds: always, when the tabs do not share. */
  readonly leading: boolean;
  /**
   * Reopen every feed in place, and resolve when each has answered. A feed
   * the door refused for good stays stopped.
   */
  reconnect(): Promise<void>;
  /** Cut any wait before a retry short, and reopen every feed now. */
  nudge(): void;
  /**
   * Stop syncing, wait for every feed to end, then erase what is held.
   * Sign-out does this. When the tabs share, every other tab on this replica
   * empties at once too.
   */
  purge(): Promise<void>;
  /** Stop syncing and keep what is held. */
  close(): Promise<void>;
}

/**
 * Open the device for `principal` in `space`: the replica, sealed at rest,
 * and a sync manager over it with no op declared yet. The hooks declare
 * what they read.
 */
export async function openDevice(options: OpenDeviceOptions): Promise<OrdoDevice> {
  const backend = options.backend ?? (await IndexedDbBackend.open(options.store ?? DEFAULT_DEVICE_STORE));
  const replica = await LocalReplica.open(backend, options.space, options.principal);
  const eraseAfterMs = options.eraseAfterMs ?? DEFAULT_ERASE_AFTER_MS;
  // Before any feed opens, so nothing this device is about to sync is caught.
  if (Number.isFinite(eraseAfterMs)) await replica.eraseUnused(Date.now() - eraseAfterMs);
  const manager = SyncManager.start(options.client, replica, [], options.manager ?? {});
  const idleMs = options.idleMs ?? DEFAULT_SYNC_IDLE_MS;
  const share = options.tabs === 'share' || (options.tabs === undefined && options.backend === undefined);
  const env = !share ? null : options.tabEnvironment === undefined ? pageTabs() : options.tabEnvironment;
  const store = options.store ?? DEFAULT_DEVICE_STORE;
  const link = env ? await TabLink.join(env, manager, replica, idleMs, store) : null;
  return {
    space: options.space,
    principal: options.principal,
    replica,
    manager,
    refineRows,
    refineMerge,
    refineRowsAsync,
    idleMs,
    tabs: link ? 'share' : 'own',
    get leading() {
      return link === null || link.leading;
    },
    reconnect: () => (link ? link.reconnect() : manager.reconnect()),
    nudge: () => (link ? link.nudge() : manager.nudge()),
    purge: async () => {
      await manager.stop();
      if (link) {
        // Poisoning tells every other tab, which empties at once (C7).
        await replica.poison();
        link.close();
      } else {
        await replica.purge();
      }
    },
    close: async () => {
      await manager.stop();
      link?.close();
    },
  };
}
