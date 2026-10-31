/**
 * The device copy: answering reads from the SDK's replica.
 *
 * `plan-client-cache.md` builds the replica in `@hominemai/ordo-sdk` (`LocalReplica`,
 * sealed at rest, one per tenant and person) and keeps ops in sync with
 * `SyncManager`. This package does not depend on the SDK. The app builds a
 * replica and a manager with it and hands them to {@link deviceCopy}, which
 * wraps them in the small interface the hooks use. That keeps this package's
 * one-dependency promise (React, as a peer) and keeps the SDK's behaviour
 * where it lives.
 *
 * ## What the device copy covers
 *
 * Only the reads the descriptor marks syncable: `result.sync.syncable`, which
 * the planner decides from the bound plan. Those are anchored, ungrouped
 * reads whose rows carry the anchor's id. It never covers:
 *
 * - `std.space.*` (the permission map and the audit spine, `plan-client-cache.md`
 *   §6). The SDK refuses these itself; the plan here refuses first.
 * - aggregates (`measure`), grouped reads, `distinct`, many-side joins, and
 *   reads with no anchor. The descriptor says `syncable: false` with the rule.
 * - writes. A write is never answered from the device and never queued.
 *
 * Nothing here caches "all data". A device holds the rows of the ops the app
 * has asked for, at the parameters it asked for them, for this person only.
 */

import type { OpDescriptor } from './descriptor.js';
import type { Params, Row } from './client.js';
import type { OrdoInterface } from './load.js';
import type { Refine } from './refine.js';
import { isRefusalClass, type Refusal } from './refusal.js';
// The read order and the idle default are the SDK's (owner ruling, 2026-10-23:
// ordo-react imports ordo-sdk's code and carries no copy).
import { DEFAULT_SYNC_IDLE_MS } from '@hominemai/ordo-sdk/replica/device.js';
import type { ReadOrder } from '@hominemai/ordo-sdk/core/spec.js';
export { DEFAULT_SYNC_IDLE_MS };
export type { ReadOrder };

// ── why an op is or is not answered from the device ─────────────────────

/** Why a read went to the network instead of the device. */
export type DeviceFallback =
  /** The provider has no device copy. */
  | 'no_device'
  /** The hook opted out with `device: false`. */
  | 'opted_out'
  /** The descriptor says the op cannot be synced (`result.sync`). */
  | 'not_syncable'
  /**
   * The op is one a device must never mirror: `std.space.*`, and the access
   * entity of an acl entity (`notesAccess` beside `notes`), which is the
   * permission map for those rows.
   */
  | 'never_mirrored'
  /** The hook asked for a refinement and the device has no way to run one. */
  | 'refine_unavailable';

export type DevicePlan =
  | { readonly use: true }
  | { readonly use: false; readonly why: DeviceFallback; readonly detail: string };

/** Ops a device must not hold, mirroring the SDK's own refusal. */
const NEVER_MIRRORED = ['std.space.'];

/**
 * Whether the op returns the access rows of an acl entity: the entity
 * `<E>Access` the platform derives beside an entity `E` that declares
 * `acl_levels`. Those rows say who holds what level on whose rows, which is
 * the permission map `std.space.*` keeps off a device for the same reason.
 */
export function isAccessMap(op: OpDescriptor, iface: OrdoInterface | undefined): boolean {
  const entity = op.result.row_identity?.entity;
  if (!iface || !entity || !entity.endsWith('Access')) return false;
  const parent = iface.entity(entity.slice(0, -'Access'.length));
  return parent !== undefined && parent.acl_levels !== null;
}

/** Whether the descriptor marks the op syncable. Reads only. */
export function isSyncable(op: OpDescriptor): boolean {
  return (
    op.kind === 'read' &&
    op.result.sync?.syncable === true &&
    typeof op.result.sync.id === 'string' &&
    op.result.row_identity !== null &&
    op.error === null
  );
}

/** The rule an op fails, in words, for a hook to say why it went to the network. */
export function whyNotSyncable(op: OpDescriptor): string {
  if (op.kind !== 'read') return `${op.op} is a ${op.kind}; only reads are kept on the device`;
  const reason = op.result.sync?.reason;
  switch (reason) {
    case 'grouped':
      return `${op.op} is a grouped read, so its rows have no identity to key them by`;
    case 'measure':
      return `${op.op} is an aggregate, so its rows are computed rather than stored rows`;
    case 'no_anchor':
      return `${op.op} has no anchor entity, so its rows have no identity to key them by`;
    default:
      return `${op.op} is not marked syncable by the descriptor`;
  }
}

export function planDevice(
  op: OpDescriptor,
  device: DeviceCopy | null,
  options: {
    readonly optOut?: boolean;
    readonly refine?: Refine | undefined;
    /** The interface, so an acl entity's access rows are recognised. */
    readonly iface?: OrdoInterface | undefined;
  } = {},
): DevicePlan {
  if (device === null) return { use: false, why: 'no_device', detail: 'no device copy is configured' };
  if (options.optOut) return { use: false, why: 'opted_out', detail: 'the hook asked for the network' };
  if (NEVER_MIRRORED.some((p) => op.op.startsWith(p))) {
    return {
      use: false,
      why: 'never_mirrored',
      detail: `${op.op} is never mirrored: a device must not hold the permission map or the audit spine`,
    };
  }
  if (isAccessMap(op, options.iface)) {
    return {
      use: false,
      why: 'never_mirrored',
      detail:
        `${op.op} lists the access rows of ${op.result.row_identity?.entity ?? 'an entity'}, which are the permission map ` +
        'for those rows: a device must not hold who may read what',
    };
  }
  if (!isSyncable(op)) return { use: false, why: 'not_syncable', detail: whyNotSyncable(op) };
  if (options.refine && !device.canRefine) {
    return {
      use: false,
      why: 'refine_unavailable',
      detail: 'the device copy was built without a local refiner (pass `refineRows` to deviceCopy)',
    };
  }
  return { use: true };
}

// ── the interface the hooks use ──────────────────────────────────────────

/** Where one declared op stands on the device. */
export interface DeviceOpState {
  /** False until the current load's last page arrived; an answer may be partial. */
  readonly complete: boolean;
  readonly loaded: number;
  /** Connections that failed in a row. Nonzero with nothing held usually means offline. */
  readonly attempts: number;
  /** The refusal that ended this op for good, or `null`. */
  readonly stopped: Refusal | null;
  /**
   * True when {@link stopped} is a revoking refusal (`access_denied`,
   * `access_guard`, `delegation_required`): the caller may no longer have
   * this op's rows, and the device holds none of them.
   */
  readonly revoked: boolean;
}

/** A question asked of rows already on the device. `sort` has no wire form. */
export interface DeviceQuery extends Refine {
  readonly sort?: readonly { readonly field: string; readonly direction?: 'asc' | 'desc' }[];
}

export interface DeviceCopy {
  /** Whether {@link read} can apply a `where`/`pick`/`sort`. */
  readonly canRefine: boolean;
  /**
   * Keep `op` at `params` in sync. Declaring one twice is one sync. Returns
   * the handle the other methods take. `syncWhere` narrows what the device
   * holds to the rows that pass it (the SDK's narrowed sync): a screen on a
   * large op can keep a slice of it. Two filters are two syncs.
   */
  declare(op: OpDescriptor, params: Params, syncWhere?: Refine['where']): string;
  /**
   * Keep `op` at `params` in sync while a screen uses it. The same as
   * {@link declare}, except that `release()` gives it back: when every use
   * of a handle has been released for `idleMs`, its feed closes. The rows
   * stay on the device, so the next use shows them at once and resumes.
   * `declare` is a use never released. Safe to release twice.
   */
  use?(op: OpDescriptor, params: Params, syncWhere?: Refine['where']): { readonly handle: string; release(): void };
  state(handle: string): DeviceOpState | null;
  /** The rows this op holds, refined locally. Empty once the op was revoked. */
  read(handle: string, query?: DeviceQuery): Promise<Row[]>;
  /**
   * True when the manager keeps rows in memory (`observe`), so a screen can
   * use {@link observe} instead of {@link read}.
   */
  readonly canObserve?: boolean;
  /**
   * The rows of `handle`, refined by `query`, kept current in memory: each
   * frame's changes are merged into the last answer, and `listener` is
   * called when the snapshot changed. `null` when the manager cannot
   * observe; the screen then reads with {@link read}.
   */
  observe?(handle: string, query: DeviceQuery, listener: () => void): DeviceObservation | null;
  /**
   * Called after a change to what the device holds or where it stands.
   *
   * With a `handle`, the listener is called only when that handle's own
   * state changed, when an op over the same entity changed (it may have
   * written a row this handle holds too), or when the copy itself changed
   * (poisoned, revoked, a new manager). A frame for another entity's op does
   * not wake it. Without one, it is called on every notice.
   */
  watch(listener: () => void, handle?: string): () => void;
  /**
   * Stop syncing every declaration of `op` and release what they held. A
   * revoking refusal means the caller may not have this op's rows, at any
   * parameters, so all of its variants go.
   */
  revokeOp(op: string): Promise<void>;
  /**
   * Called after a write committed. Runs the app's `reload` when it gave
   * one. Without one it does nothing: the open feed brings the change.
   */
  afterCommit(): Promise<void>;
  /**
   * Ask the server again. Runs the app's `reload` when it gave one, which
   * may replace the manager, then binds to the manager now current (see
   * {@link DeviceCopyOptions.manager}). Without one, it calls the manager's
   * `reconnect()`, which reopens every feed in place and resolves when each
   * has answered. A revoked device does nothing.
   */
  reload(): Promise<void>;
  /**
   * Reconnect every feed now, which runs a fresh load. The manager does this
   * itself when the browser comes back online, so an app calls it only when
   * it has its own reason to think the connection is stale. A manager without
   * a `nudge` ignores it, and so does a revoked device.
   */
  nudge(): void;
  /** True once a revoking refusal ended an op here, so the replica was poisoned. */
  readonly revoked: boolean;
}

// ── what `deviceCopy` needs from the SDK, structurally ──────────────────

/** The op table entry the SDK's `syncOp` and `openFeed` read (the SDK's `OpSpec`). */
export interface SyncSpec {
  readonly op: string;
  readonly version: string;
  readonly method: string;
  readonly kind: string;
  readonly terminal: readonly string[];
  readonly paged: boolean;
  readonly singleRow: boolean;
  readonly casParam: string | null;
  readonly params: readonly {
    readonly name: string;
    readonly type: string;
    readonly required: boolean;
    readonly idEntity: string | null;
    readonly enumValues: readonly string[] | null;
    readonly min: string | null;
    readonly max: string | null;
    /** The places of a `money(p)` or the scale of a `decimal(p)`; `null` or absent otherwise. */
    readonly places?: number | null;
  }[];
  readonly refusals: readonly string[];
  readonly standingError: { readonly code: string; readonly message: string } | null;
  readonly sync: { readonly entity: string; readonly key: string } | null;
  readonly refine: {
    readonly tier: 'none' | 'basic';
    readonly fields: readonly string[];
    /**
     * The outputs that are exact decimals, `money` and `decimal` (27ak).
     * They arrive as strings and the device compares them as numbers, as
     * Postgres does; without this list a `sort total` on the device would
     * put "100.00" before "29.99".
     */
    readonly decimals?: readonly string[];
    /** Every output's descriptor type, so a condition's value is checked before the wire. */
    readonly types?: Readonly<Record<string, string>>;
    /** The sealed outputs: at the door a sealed condition is `eq` or `in` only. */
    readonly sealed?: readonly string[];
    /** The refinement size limits, or `null` when the descriptor states none. */
    readonly limits?: { readonly conditions: number; readonly inValues: number } | null;
  };
  /** The read's stated order (`result.order`), which {@link DeviceCopy.read} returns rows in. */
  readonly order: ReadOrder | null;
}

/**
 * The SDK's `OpSpec`, built from the descriptor instead of the generated
 * table. It is the same projection `ordo gen ts` performs, so an app that
 * uses this package needs no generated code to sync an op.
 */
export function toSyncSpec(op: OpDescriptor): SyncSpec {
  const sync = op.result.sync;
  const casParam = op.statements.find((s) => s.cas_param !== null)?.cas_param ?? null;
  return {
    op: op.op,
    version: op.version,
    method: op.call.method,
    kind: op.kind,
    terminal: op.call.terminal,
    paged: op.call.paged,
    singleRow: op.result.single_row,
    casParam,
    params: op.params.map((p) => ({
      name: p.name,
      type: p.type,
      required: p.required,
      idEntity: p.id_entity,
      enumValues: p.enum_values && p.enum_values.length > 0 ? p.enum_values : null,
      min: p.min,
      max: p.max,
      places: p.places ?? null,
    })),
    refusals: op.refusals,
    standingError: op.error ? { code: op.error.code, message: op.error.message } : null,
    sync:
      sync?.syncable === true && typeof sync.id === 'string' && op.result.row_identity
        ? { entity: op.result.row_identity.entity, key: sync.id }
        : null,
    refine: {
      tier: op.refine.tier,
      fields: op.refine.fields,
      decimals: op.result.outputs.filter((o) => o.type === 'money' || o.type === 'decimal').map((o) => o.name),
      types: Object.fromEntries(
        op.result.outputs.filter((o) => o.type !== null).map((o) => [o.name, o.type as string]),
      ),
      sealed: op.result.outputs.filter((o) => o.sealed).map((o) => o.name),
      limits: op.refine.limits ? { conditions: op.refine.limits.conditions, inValues: op.refine.limits.in_values } : null,
    },
    order: op.result.order ?? null,
  };
}

/**
 * The part of the SDK's `SyncManager` the adapter uses. The SDK's own manager
 * is assignable to it as it is: {@link SyncSpec} is the SDK's `OpSpec` shape,
 * so a refined, sealed or limited read keeps its refine types, sealed fields,
 * limits and places when the adapter declares it.
 */
export interface ManagerLike {
  sync(spec: SyncSpec, params?: Params, refine?: Refine): string;
  drop(key: string): Promise<void>;
  /**
   * The SDK's `SyncManager.idle`: end one op's feed for now and keep its
   * rows and result hash. Without it, a sync nobody uses stays open.
   */
  idle?(key: string): Promise<void>;
  /** The SDK's `SyncManager.nudge`: reconnect every feed now. */
  nudge?(): void;
  /**
   * The SDK's `SyncManager.reconnect`: reopen every feed in place, each only
   * after its old one ended, and resolve when each has answered.
   */
  reconnect?(): Promise<void>;
  readonly view: {
    readonly ops: readonly {
      readonly key: string;
      readonly loaded: number;
      readonly complete: boolean;
      /** The position of the last frame applied; the SDK's view carries it. */
      readonly position?: number | null;
      readonly attempts: number;
      readonly stopped: { readonly detail: Omit<Refusal, 'declared'> & { readonly declared?: boolean } } | null;
    }[];
  };
  // The view is the SDK's `SyncView`, which the adapter reads through `view` and never from the callback.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  watch(listener: (view: any) => void): () => void;
  /**
   * The SDK's `SyncManager.observe`: one handle's rows kept in memory, with
   * each frame applied there. When the manager has it, a screen reads its
   * rows from memory and the store is not read again during a load (H1).
   */
  observe?(key: string, listener: (rows: ObservedLike) => void): { snapshot(): ObservedLike; stop(): void };
}

/** One handle's rows in memory, as the SDK's `ObservedRows` gives them. */
export interface ObservedLike {
  readonly rows: readonly { readonly entity: string; readonly id: string; readonly row: Row }[];
  readonly state: ManagerLike['view']['ops'][number] | null;
  readonly filled: boolean;
  readonly version?: number;
  readonly changes?: {
    readonly from: number;
    readonly added: readonly { readonly row: Row }[];
    readonly removed: readonly { readonly row: Row }[];
  } | null;
}

/** What a screen shows of one handle: its rows, refined, and where it stands, from the same frames. */
export interface DeviceSnapshot {
  readonly rows: readonly Row[];
  readonly state: DeviceOpState | null;
  /** False until the rows were first read from the store. */
  readonly filled: boolean;
}

/** One {@link DeviceCopy.observe} call. */
export interface DeviceObservation {
  /** The last snapshot: the same object until something this screen shows changed. */
  snapshot(): DeviceSnapshot;
  stop(): void;
}

/** The part of the SDK's `LocalReplica` the adapter uses. */
export interface ReplicaLike {
  holdings(op: string): Promise<readonly { entity: string; id: string }[]>;
  get(entity: string, id: string): Promise<{ readonly row: Row } | undefined>;
  /** Erase and refuse everything held. Called once when an op is revoked. */
  poison?(): Promise<void>;
}

export interface DeviceCopyOptions {
  /**
   * The SDK's sync manager, or a function that answers the one now current.
   *
   * A reload replaces the manager (a fresh feed is a fresh manager), so the
   * binding cannot be for good. A function is the smaller contract than a
   * `refresh()` on the manager: the SDK's `SyncManager` is assignable as it
   * is, nothing has to be added to it, and the adapter does the rest. It
   * looks at the function on every call. When the answer is a different
   * manager it declares everything the hooks declared on the new one and
   * moves its watchers across, so the app keeps no table of declarations.
   */
  readonly manager: ManagerLike | (() => ManagerLike);
  readonly replica: ReplicaLike;
  /**
   * The SDK's `refineRows`. Without it the device answers only unrefined
   * reads in id order, and a refined one goes to the network
   * (`refine_unavailable`). With it, an unrefined read comes back in the
   * op's stated order (27o), which the SDK sorts by the same rule the server
   * uses. `decimals` names the money and decimal outputs, which compare as
   * numbers (27ak).
   */
  readonly refineRows?: (
    rows: readonly Row[],
    query: DeviceQuery,
    order?: ReadOrder | null,
    decimals?: readonly string[],
  ) => Row[];
  /**
   * The SDK's `refineMerge`. With it, a screen on an observing manager
   * brings its answer up to date with each frame's rows alone, in one pass,
   * instead of refining every row again on each publish.
   */
  readonly refineMerge?: (
    sorted: readonly Row[],
    added: readonly Row[],
    removed: ReadonlySet<Row>,
    query: DeviceQuery,
    order?: ReadOrder | null,
    decimals?: readonly string[],
  ) => readonly Row[];
  /**
   * The SDK's `refineRowsAsync`. With it, a full refinement of more than
   * {@link refineChunk} rows (the first answer, or a new query) runs in
   * slices and never holds the page for long. Without it that refinement is
   * one synchronous task, which can hold the page for a moment over many
   * rows; the device says so once with `console.warn`.
   */
  readonly refineRowsAsync?: (
    rows: readonly Row[],
    query: DeviceQuery,
    order?: ReadOrder | null,
    decimals?: readonly string[],
    options?: { readonly signal?: AbortSignal; readonly chunk?: number },
  ) => Promise<Row[]>;
  /** Rows per slice of a sliced refinement, and the size above which one is used. 5,000 by default. */
  readonly refineChunk?: number;
  /**
   * How the app makes the device ask the server again, after a write and on
   * {@link DeviceCopy.reload}; for example, one that replaces the manager.
   * The SDK's manager keeps a feed open and the server pushes a change when
   * it hears of one, a tenant made after the server booted included, so most
   * apps give none. Without it, `reload()` calls the manager's `reconnect()`,
   * and a committed write reaches the device when the feed says so.
   */
  readonly reload?: () => Promise<void> | void;
  /**
   * Poison the replica when an op is stopped by a revoking refusal, so it
   * holds nothing and serves nothing, as the notes page did by hand. On by
   * default; it needs `replica.poison`. The door answers one `access_denied`
   * for "not allowed" and "closed", so a feed it refuses for good is the case
   * treated as revocation.
   */
  readonly poisonOnRevoke?: boolean;
  /**
   * How long a sync stays open after its last screen lets go, in
   * milliseconds. Default {@link DEFAULT_SYNC_IDLE_MS} (15 s, owner
   * 2026-10-21). The SDK's `openDevice` answers it as set there. `Infinity`
   * keeps every sync open until the device closes.
   */
  readonly idleMs?: number;
  /**
   * How {@link DeviceCopy.reload} reopens the feeds when the app gives no
   * `reload`. The SDK's `openDevice` answers an object with one, so that
   * object can be passed as these options. Defaults to the manager's own.
   */
  readonly reconnect?: () => Promise<void>;
}

/**
 * What `<OrdoProvider device={...}>` takes: a {@link DeviceCopy}, or the
 * options to build one. The SDK's `openDevice` answers the second kind, so an
 * app writes `device={await openDevice({ client, space, principal })}`.
 */
export type DeviceSource = DeviceCopy | DeviceCopyOptions;

const wrapped = new WeakMap<object, DeviceCopy>();

/**
 * The device copy for `source`: itself when it is one, otherwise one built
 * by {@link deviceCopy} the first time and the same one every time after.
 */
export function asDeviceCopy(source: DeviceSource): DeviceCopy {
  if (isDeviceCopy(source)) return source;
  let copy = wrapped.get(source);
  if (!copy) {
    copy = deviceCopy(source);
    wrapped.set(source, copy);
  }
  return copy;
}

function isDeviceCopy(source: DeviceSource): source is DeviceCopy {
  return typeof (source as Partial<DeviceCopy>).declare === 'function';
}

/** Refusals that mean the device may no longer hold what an op synced. */
export const REVOKING: ReadonlySet<string> = new Set(['access_denied', 'access_guard', 'delegation_required']);

export function isRevoking(refusal: Pick<Refusal, 'class'> | null): boolean {
  return refusal !== null && isRefusalClass(refusal.class) && REVOKING.has(refusal.class);
}

/** {@link DeviceCopyOptions.refineChunk} when the app sets none, as the SDK's `DEFAULT_REFINE_CHUNK`. */
export const DEFAULT_DEVICE_REFINE_CHUNK = 5_000;

type OpView = ManagerLike['view']['ops'][number];

function toState(found: OpView): DeviceOpState {
  const stopped = found.stopped ? ({ declared: false, ...found.stopped.detail } as Refusal) : null;
  return {
    complete: found.complete,
    loaded: found.loaded,
    attempts: found.attempts,
    stopped,
    revoked: isRevoking(stopped),
  };
}

function sameDeviceState(a: DeviceOpState | null, b: DeviceOpState | null): boolean {
  if (a === null || b === null) return a === b;
  return (
    a.complete === b.complete &&
    a.loaded === b.loaded &&
    a.attempts === b.attempts &&
    a.revoked === b.revoked &&
    a.stopped?.class === b.stopped?.class
  );
}

const NOTHING: readonly Row[] = Object.freeze([]) as readonly Row[];

/** Wrap an SDK `SyncManager` and `LocalReplica` as the interface the hooks use. */
export function deviceCopy(options: DeviceCopyOptions): DeviceCopy {
  const { manager, replica, refineRows, reload, reconnect } = options;
  const { refineMerge, refineRowsAsync } = options;
  const refineChunk = options.refineChunk ?? DEFAULT_DEVICE_REFINE_CHUNK;
  const poisonOnRevoke = options.poisonOnRevoke ?? true;
  const idleMs = options.idleMs ?? DEFAULT_SYNC_IDLE_MS;
  /**
   * Uses per handle (`use`), and the timer that idles a handle nobody uses.
   * A handle `declare`d is pinned: it is never idled.
   */
  const uses = new Map<string, number>();
  const pinned = new Set<string>();
  const idleTimers = new Map<string, ReturnType<typeof setTimeout>>();
  // Said once per device copy: a large refinement ran in one task because
  // `refineRowsAsync` was not passed (audit 2026-10-11, finding 3).
  let warnedUnsliced = false;
  const current = (): ManagerLike => (typeof manager === 'function' ? manager() : manager);
  let bound = current();
  let unwatch: (() => void) | null = null;
  let revoked = false;
  /** Each listener, with the handle it watches, or `undefined` for every notice. */
  const listeners = new Map<() => void, string | undefined>();
  /**
   * Each handle's state at the last notice, as a string, so a notice wakes
   * only the handles it is about (H1, `plan-client-loading-fixes.md` §1.2).
   */
  let seen = new Map<string, string>();
  // Every declaration, so a replaced manager gets them again.
  const declared = new Map<string, { readonly spec: SyncSpec; readonly params: Params; readonly refine?: Refine }>();
  // Each handle's stated order and its decimal outputs, kept from its
  // declaration so `read` can return rows in that order, and compare
  // money as money, without the descriptor in hand.
  const orders = new Map<string, ReadOrder | null>();
  const decimals = new Map<string, readonly string[]>();

  /** Wake every listener: the copy itself changed. */
  const notify = (): void => {
    seen = signatures();
    for (const l of [...listeners.keys()]) l();
  };
  const signatures = (): Map<string, string> => {
    const now = new Map<string, string>();
    for (const o of bound.view.ops) {
      now.set(o.key, `${o.loaded}|${o.complete}|${o.position ?? ''}|${o.attempts}|${o.stopped?.detail.class ?? ''}`);
    }
    return now;
  };
  const entityOf = (handle: string): string | undefined => declared.get(handle)?.spec.sync?.entity;
  /**
   * Wake the listeners a manager view is about: those of handles whose state
   * changed, and those of handles over the same entity, since a frame of one
   * op may write a row another op holds. The rest stay asleep.
   */
  const notifyChanged = (): void => {
    const now = signatures();
    const changed = new Set<string>();
    for (const [k, sig] of now) if (seen.get(k) !== sig) changed.add(k);
    for (const k of seen.keys()) if (!now.has(k)) changed.add(k);
    seen = now;
    if (changed.size === 0) return;
    const entities = new Set<string>();
    for (const k of changed) {
      const e = entityOf(k);
      if (e !== undefined) entities.add(e);
    }
    for (const [l, h] of [...listeners]) {
      if (h === undefined || changed.has(h)) l();
      else {
        const e = entityOf(h);
        if (e !== undefined && entities.has(e)) l();
      }
    }
  };
  /**
   * A revoking stop marks the copy revoked. The replica is poisoned only once
   * no op is still live, so a row another synced op still holds is never
   * wiped by the revocation of this one; the last op's revocation poisons it.
   */
  let poisoned = false;
  /**
   * Read the bound manager's view: latch `revoked` on a revoking stop and
   * start the poisoning when the last op has stopped. True when poisoning
   * began, which notifies when it ends.
   */
  const absorb = (): boolean => {
    const ops = bound.view.ops;
    if (ops.some((o) => o.stopped !== null && isRevoking(o.stopped.detail))) revoked = true;
    if (revoked && !poisoned && ops.every((o) => o.stopped !== null) && poisonOnRevoke && replica.poison) {
      poisoned = true;
      void replica.poison().then(notify, notify);
      return true;
    }
    return false;
  };
  const onView = (): void => {
    if (absorb()) return;
    notifyChanged();
  };
  // The adapter always watches the manager: a revocation must poison the
  // replica whether or not a hook is mounted to hear of it.
  const attach = (): void => {
    unwatch?.();
    unwatch = bound.watch(onView);
  };
  /** Follow the app's manager when it has been replaced. */
  const rebind = (): boolean => {
    const now = current();
    if (now === bound) return false;
    bound = now;
    // The latches belong to the manager they were set on. A new sign-in
    // starts clean, and the new manager's own view says if it is revoked.
    revoked = false;
    poisoned = false;
    const moved = new Map<string, string>();
    for (const [handle, d] of [...declared]) {
      const again = d.refine ? bound.sync(d.spec, d.params, d.refine) : bound.sync(d.spec, d.params);
      moved.set(handle, again);
      if (again !== handle) {
        declared.delete(handle);
        declared.set(again, d);
        orders.set(again, orders.get(handle) ?? null);
        decimals.set(again, decimals.get(handle) ?? []);
      }
    }
    attach();
    absorb();
    // Memory belongs to the manager it was filled from: every screen starts
    // again from the new manager's store (C7, a new sign-in).
    for (const w of [...watches]) restart(w, moved.get(w.handle) ?? w.handle);
    return true;
  };

  // ── screens on rows in memory (H1, I6) ──────────────────────────────

  interface Watch {
    handle: string;
    readonly query: DeviceQuery;
    readonly listener: () => void;
    inner: { snapshot(): ObservedLike; stop(): void } | null;
    current: DeviceSnapshot;
    /** The refined rows, before `pick`, as of the snapshot `seen` less the changes not yet merged. */
    base: readonly Row[] | null;
    /** The refined rows the shown rows were picked from. */
    shown: readonly Row[] | null;
    /** The last snapshot version taken in, and the changes since `base` not yet merged. */
    seen: number;
    added: Set<Row>;
    removed: Set<Row>;
    /** A change was missed, so the next answer is refined in full. */
    gap: boolean;
    running: AbortController | null;
    readonly picks: WeakMap<Row, Row>;
    stopped: boolean;
  }
  const watches = new Set<Watch>();

  const wantsRefine = (w: Watch): boolean => {
    if (!refineRows) return false;
    const q = w.query;
    if (q.where || q.pick || q.sort) return true;
    return orders.get(w.handle)?.stated === true;
  };

  const pickOf = (w: Watch, refined: readonly Row[]): readonly Row[] => {
    const wanted = w.query.pick;
    if (!wanted) return refined;
    return refined.map((row) => {
      let picked = w.picks.get(row);
      if (!picked) {
        const out: Record<string, unknown> = {};
        for (const field of wanted) out[field] = (row as Record<string, unknown>)[field] ?? null;
        picked = out as Row;
        w.picks.set(row, picked);
      }
      return picked;
    });
  };

  const publishWatch = (w: Watch, rows: readonly Row[], state: DeviceOpState | null, filled: boolean): void => {
    const was = w.current;
    if (was.rows === rows && was.filled === filled && sameDeviceState(was.state, state)) return;
    w.current = { rows, state, filled };
    if (!w.stopped) w.listener();
  };

  /**
   * Fold one snapshot's changes into those not merged yet. A snapshot that
   * does not follow the last one taken in is a gap: the next answer is
   * refined in full.
   */
  const takeIn = (w: Watch, s: ObservedLike): void => {
    const version = s.version ?? -1;
    if (version === w.seen && !w.gap) return;
    if (s.changes && s.changes.from === w.seen && !w.gap) {
      for (const r of s.changes.removed) {
        if (w.added.has(r.row)) w.added.delete(r.row);
        else w.removed.add(r.row);
      }
      for (const r of s.changes.added) w.added.add(r.row);
    } else {
      w.gap = true;
    }
    w.seen = version;
  };

  /** Show `refined`, picked, keeping the picked array when the refined one did not change. */
  const show = (w: Watch, refined: readonly Row[], state: DeviceOpState | null, filled: boolean): void => {
    const rows = refined === w.shown ? w.current.rows : pickOf(w, refined);
    w.shown = refined;
    publishWatch(w, rows, state, filled);
  };

  /** Bring a watch's answer up to the manager's latest snapshot. */
  const absorbWatch = (w: Watch, s: ObservedLike): void => {
    if (w.stopped) return;
    const state = s.state ? toState(s.state) : null;
    if (state?.revoked) {
      // A revoked op serves nothing, even a row another op still holds.
      w.running?.abort();
      w.running = null;
      w.base = null;
      w.gap = true;
      w.shown = null;
      publishWatch(w, NOTHING, state, s.filled);
      return;
    }
    takeIn(w, s);
    // A sliced refinement is under way: the changes wait for it to end.
    if (w.running) return;
    if (!wantsRefine(w)) {
      // The rows as held, by id: nothing to refine.
      const rows = s.rows.length === 0 ? NOTHING : s.rows.map((r) => r.row);
      w.base = rows;
      w.added.clear();
      w.removed.clear();
      w.gap = false;
      w.shown = rows;
      publishWatch(w, rows, state, s.filled);
      return;
    }
    const order = orders.get(w.handle) ?? null;
    const exact = decimals.get(w.handle) ?? [];
    const { pick: _pick, ...unpicked } = w.query;
    if (w.base !== null && !w.gap && (refineMerge || (w.added.size === 0 && w.removed.size === 0))) {
      const refined =
        w.added.size === 0 && w.removed.size === 0
          ? w.base
          : refineMerge!(w.base, [...w.added], w.removed, unpicked, order, exact);
      w.added.clear();
      w.removed.clear();
      w.base = refined;
      show(w, refined, state, s.filled);
      return;
    }
    // In full: the snapshot's rows, from which later changes are counted.
    const rows = s.rows.map((r) => r.row);
    w.added.clear();
    w.removed.clear();
    w.gap = false;
    if (refineRowsAsync && rows.length > refineChunk) {
      const controller = new AbortController();
      w.running = controller;
      void refineRowsAsync(rows, unpicked, order, exact, { signal: controller.signal, chunk: refineChunk }).then(
        (refined) => {
          if (w.running !== controller) return;
          w.running = null;
          w.base = refined;
          w.shown = null;
          // The changes that arrived meanwhile are merged now.
          absorbWatch(w, w.inner?.snapshot() ?? s);
        },
        () => {
          if (w.running !== controller) return;
          w.running = null;
          w.gap = true;
          absorbWatch(w, w.inner?.snapshot() ?? s);
        },
      );
      return;
    }
    if (rows.length > refineChunk && !warnedUnsliced) {
      warnedUnsliced = true;
      console.warn(
        `ordo: refined ${rows.length} rows in one task, which can hold the page. ` +
          'Pass the SDK\'s `refineRowsAsync` to deviceCopy to refine in slices.',
      );
    }
    const refined = refineRows!(rows, unpicked, order, exact);
    w.base = refined;
    show(w, refined, state, s.filled);
  };

  /** Observe `handle` on the bound manager, from nothing. */
  const restart = (w: Watch, handle: string): void => {
    w.inner?.stop();
    w.running?.abort();
    w.running = null;
    w.handle = handle;
    w.base = null;
    w.shown = null;
    w.seen = -1;
    w.added = new Set();
    w.removed = new Set();
    w.gap = true;
    if (!bound.observe) {
      w.inner = null;
      publishWatch(w, NOTHING, null, false);
      return;
    }
    // A snapshot published while `observe` returns is taken in after it.
    let early: ObservedLike | null = null;
    let ready = false;
    w.inner = bound.observe(handle, (s) => {
      if (!ready) early = s;
      else absorbWatch(w, s);
    });
    ready = true;
    absorbWatch(w, early ?? w.inner.snapshot());
  };

  /** Declare `op` on the bound manager and remember it, so a replaced manager gets it again. */
  const open = (op: OpDescriptor, params: Params, syncWhere: Refine['where'] | undefined): string => {
    rebind();
    const spec = toSyncSpec(op);
    const refine: Refine | undefined = syncWhere && syncWhere.length > 0 ? { where: syncWhere } : undefined;
    const handle = refine ? bound.sync(spec, params, refine) : bound.sync(spec, params);
    declared.set(handle, refine ? { spec, params, refine } : { spec, params });
    orders.set(handle, spec.order);
    decimals.set(handle, spec.refine.decimals ?? []);
    return handle;
  };

  const copy: DeviceCopy = {
    canRefine: refineRows !== undefined,
    get revoked() {
      return revoked;
    },
    declare(op, params, syncWhere) {
      const handle = open(op, params, syncWhere);
      pinned.add(handle);
      return handle;
    },
    use(op, params, syncWhere) {
      const handle = open(op, params, syncWhere);
      const timer = idleTimers.get(handle);
      if (timer !== undefined) {
        clearTimeout(timer);
        idleTimers.delete(handle);
      }
      uses.set(handle, (uses.get(handle) ?? 0) + 1);
      let released = false;
      return {
        handle,
        release: () => {
          if (released) return;
          released = true;
          const left = (uses.get(handle) ?? 1) - 1;
          if (left > 0) {
            uses.set(handle, left);
            return;
          }
          uses.delete(handle);
          if (pinned.has(handle) || !Number.isFinite(idleMs)) return;
          idleTimers.set(
            handle,
            setTimeout(() => {
              idleTimers.delete(handle);
              if ((uses.get(handle) ?? 0) > 0 || pinned.has(handle)) return;
              if (typeof bound.idle !== 'function') return;
              // Not declared again on a new manager, and not reopened by a reload.
              declared.delete(handle);
              void bound.idle(handle);
            }, Math.max(0, idleMs)),
          );
        },
      };
    },
    state(handle) {
      rebind();
      const found = bound.view.ops.find((o) => o.key === handle);
      return found ? toState(found) : null;
    },
    get canObserve() {
      rebind();
      return typeof bound.observe === 'function';
    },
    observe(handle, query, listener) {
      rebind();
      if (typeof bound.observe !== 'function') return null;
      const w: Watch = {
        handle,
        query,
        listener,
        inner: null,
        current: { rows: NOTHING, state: null, filled: false },
        base: null,
        shown: null,
        seen: -1,
        added: new Set(),
        removed: new Set(),
        gap: true,
        running: null,
        picks: new WeakMap(),
        stopped: false,
      };
      watches.add(w);
      restart(w, handle);
      return {
        snapshot: () => w.current,
        stop: () => {
          if (w.stopped) return;
          w.stopped = true;
          watches.delete(w);
          w.running?.abort();
          w.running = null;
          w.inner?.stop();
          w.inner = null;
        },
      };
    },
    async read(handle, query) {
      // A revoked op serves nothing, even if a row survives because another
      // synced op still holds it. The SDK's manager releases the op's rows;
      // this is the other half: the op does not answer from what is left.
      const state = this.state(handle);
      if (state?.revoked) return [];
      // The replica keeps rows by id, not in the op's order. Ids sort by
      // mint time, so id order is the stable default, and it is what the
      // device keeps where the descriptor states no order (27o). Where it
      // does, the SDK's refiner puts the rows in it; a hook that wants
      // another order says so with `sort`.
      const held = [...(await replica.holdings(handle))].sort((a, b) =>
        a.id < b.id ? -1 : a.id > b.id ? 1 : 0,
      );
      const rows: Row[] = [];
      for (const { entity, id } of held) {
        const record = await replica.get(entity, id);
        if (record) rows.push(record.row);
      }
      const order = orders.get(handle) ?? null;
      const exact = decimals.get(handle) ?? [];
      if (!refineRows) return rows;
      if (query && (query.where || query.pick || query.sort)) return refineRows(rows, query, order, exact);
      return order?.stated ? refineRows(rows, {}, order, exact) : rows;
    },
    watch(listener, handle) {
      rebind();
      listeners.set(listener, handle);
      return () => {
        listeners.delete(listener);
      };
    },
    async revokeOp(op) {
      rebind();
      // The manager's handle for an op is its name, then `?` and the bound
      // parameters when there are any, so this finds every variant.
      const keys = bound.view.ops.map((o) => o.key).filter((k) => k === op || k.startsWith(`${op}?`));
      for (const key of keys) {
        declared.delete(key);
        await bound.drop(key);
      }
    },
    async reload() {
      // A manager replaced since the last call is followed first, which
      // clears the latch the old one set.
      const followed = rebind();
      if (revoked) {
        if (followed) notify();
        return;
      }
      if (reload) await reload();
      // A manager just followed opened its feeds already.
      else if (!followed) await (reconnect ? reconnect() : bound.reconnect?.());
      if (rebind() || followed) notify();
    },
    async afterCommit() {
      // The open feed pushes the committed change. Reopening every feed on
      // each write would cost a load per feed, so only the app's own reload
      // runs here.
      if (reload) await this.reload();
      else rebind();
    },
    nudge() {
      rebind();
      if (!revoked) bound.nudge?.();
    },
  };
  attach();
  return copy;
}
