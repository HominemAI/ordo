/**
 * The device's replica of one space, for one principal (`plan-client-cache.md`
 * C3).
 *
 * It holds rows keyed by entity and id, each with the command-log position it
 * was read at, and enforces the rule the rest of the replica relies on: **a
 * version read at a lower position never replaces one read at a higher
 * position.** A removal is kept as a marker at its position, so an older
 * version arriving later cannot bring the row back.
 *
 * One replica per space and principal, never a store shared across them
 * (`implementation.md`, client storage rules): two people on one device, or
 * one person in two spaces, open different replicas under different keys.
 *
 * Every write is one transaction of the backend, and a sync frame is one
 * write (`plan-client-loading-fixes.md` H4). Reads do not wait behind writes:
 * each is one read of the backend, so it sees whole writes only.
 */

import type { Row } from '../core/client.js';
import type { Backend } from './backend.js';
import {
  createDeviceKeys,
  digestOf,
  nameOf,
  openBytes,
  sealBytes,
  type DeviceKeys,
} from './seal.js';

/** One row as the replica holds it. */
export interface ReplicaRecord {
  readonly entity: string;
  readonly id: string;
  /** The command-log position this version was read at. */
  readonly position: number;
  readonly row: Row;
}

/** What is sealed: a row, or the marker left when it stopped being visible. */
interface Stored {
  entity: string;
  id: string;
  position: number;
  row: Row | null;
}

/**
 * A row as the replica holds it after a write: `row` is `null` when a removal
 * at a higher position holds its place, so the row is not visible.
 */
export interface HeldRow {
  readonly entity: string;
  readonly id: string;
  readonly position: number;
  readonly row: Row | null;
}

/** What one {@link LocalReplica.applyFrame} changed, taken from its own write. */
export interface FrameDelta {
  /**
   * Each row the frame put, as the replica now holds it. When the replica
   * already held a newer version (C3 refused the put), this is that version.
   */
  readonly upserts: readonly HeldRow[];
  /**
   * Each row the holder let go. `removed` is true when no other holder kept
   * it, so it left the replica.
   */
  readonly releases: readonly { readonly entity: string; readonly id: string; readonly removed: boolean }[];
}

/** One whole frame for {@link LocalReplica.applyFrame}. */
export interface Frame {
  /** The command-log position every change in the frame was read at. */
  readonly position: number;
  /** Rows to put and hold, in order. */
  readonly upserts?: readonly { readonly entity: string; readonly id: string; readonly row: Row }[];
  /** Rows the holder lets go, after the upserts. */
  readonly releases?: readonly { readonly entity: string; readonly id: string }[];
  /**
   * The server's hash of the whole result, kept in the same write (C4d). A
   * `null` hash forgets the one held, so the next connection loads in full.
   */
  readonly result?: { readonly version: string; readonly hash: string | null };
}

/** One change inside a write. A write applies its changes in order. */
type Change =
  | { readonly kind: 'put'; readonly record: Stored }
  | { readonly kind: 'hold'; readonly op: string; readonly entity: string; readonly id: string }
  | {
      readonly kind: 'release';
      readonly op: string;
      readonly entity: string;
      readonly id: string;
      readonly position: number;
    }
  | { readonly kind: 'result'; readonly op: string; readonly version: string; readonly hash: string | null }
  /** Let go of a row with no position: the row record is deleted when no holder keeps it. */
  | { readonly kind: 'forget'; readonly op: string; readonly entity: string; readonly id: string }
  /** When the holder `op` was last used, or `null` to forget it. */
  | { readonly kind: 'used'; readonly op: string; readonly at: number | null };

/** One row's two records while a write is decided. */
interface Slot {
  readonly key: string;
  readonly holdersKey: string;
  stored: Stored | undefined;
  holders: readonly string[];
  rowDirty: boolean;
  holdersDirty: boolean;
}

/** A record a write puts (`value`) or deletes (`null`), before its name is known. */
type Intent =
  | { readonly name: 'hold'; readonly op: string; readonly entity: string; readonly id: string; readonly value: unknown }
  | { readonly name: 'result'; readonly op: string; readonly value: unknown }
  | { readonly name: 'used'; readonly op: string; readonly value: unknown };

interface Written {
  /** True when the replica was poisoned, so nothing was written. */
  readonly refused: boolean;
  readonly results: boolean[];
  readonly slots: ReadonlyMap<string, Slot>;
}

export class LocalReplica {
  readonly #backend: Backend;
  readonly #keys: DeviceKeys;
  readonly #ns: string;
  /**
   * Writes run one at a time, so a read-then-write is never interleaved.
   * Reads do not queue: each is one read of the backend, and every write is
   * one transaction, so a read sees whole writes only (H4).
   */
  #queue: Promise<unknown> = Promise.resolve();
  /**
   * Poisoned: access was revoked, the records are gone, and this instance
   * serves nothing (`plan-client-cache.md` C7).
   *
   * The latch is on the instance because {@link purge} deletes the keys as
   * well as the records: a purged replica is dead rather than empty, and
   * without this a later read would open a freshly keyed store and look
   * like an ordinary miss. `implementation.md` says the client "purges
   * before serving any read", and this is where "before" is enforced.
   *
   * It gates writes as well as reads, which a test caught rather than the
   * design: gating reads alone left a poisoned replica answering empty while
   * still sealing new records to disk under the keys it holds in memory.
   * Poisoned means it serves nothing AND stores nothing.
   */
  #poisoned = false;
  /** Called synchronously when the replica is poisoned, before its purge starts. */
  readonly #onPoison = new Set<() => void>();
  /**
   * Prefix names, kept for the replica's life: one HMAC per entity and per
   * holder rather than one per row. They depend only on the keys in memory.
   */
  readonly #prefixes = new Map<string, Promise<string>>();

  private constructor(backend: Backend, keys: DeviceKeys, ns: string) {
    this.#backend = backend;
    this.#keys = keys;
    this.#ns = ns;
  }

  /** Open the replica for `principal` in `space`, creating its keys the first time. */
  static async open(backend: Backend, space: string, principal: string): Promise<LocalReplica> {
    const ns = await digestOf(['ordo-replica', space, principal]);
    let keys = await backend.loadKeys(ns);
    if (!keys) {
      keys = await createDeviceKeys();
      await backend.saveKeys(ns, keys);
    }
    return new LocalReplica(backend, keys, ns);
  }

  /** Apply a version of a row, unless a later one is already held. */
  async put(record: ReplicaRecord): Promise<boolean> {
    const { results } = await this.#write([{ kind: 'put', record: { ...record } }]);
    return results[0] ?? false;
  }

  /**
   * The row stopped being visible to this principal at `position`. Kept as a
   * marker, so an older version cannot bring it back.
   */
  async remove(entity: string, id: string, position: number): Promise<boolean> {
    const { results } = await this.#write([{ kind: 'put', record: { entity, id, position, row: null } }]);
    return results[0] ?? false;
  }

  /** The row, if the replica holds a visible version of it. */
  async get(entity: string, id: string): Promise<ReplicaRecord | undefined> {
    if (this.#poisoned) return undefined;
    const sealed = await this.#backend.get(await this.#key(entity, id));
    if (!sealed || this.#poisoned) return undefined;
    const r = visible((await this.#open(sealed)) as Stored);
    return this.#poisoned ? undefined : r;
  }

  /** Every visible row of `entity`, by id. One read of the backend. */
  async scan(entity: string): Promise<ReplicaRecord[]> {
    if (this.#poisoned) return [];
    const found = await this.#backend.entries(await this.#prefix(entity));
    const opened = await Promise.all(found.map(([, sealed]) => this.#open(sealed) as Promise<Stored>));
    if (this.#poisoned) return [];
    const out: ReplicaRecord[] = [];
    for (const s of opened) {
      const r = visible(s);
      if (r) out.push(r);
    }
    return out.sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  }

  /** Sign-out, or access revoked: every record, and the keys that open them. */
  purge(): Promise<void> {
    return this.#serial(async () => {
      // One ranged delete in one transaction (M2), not one per record.
      await this.#backend.deletePrefix(`${this.#ns}/`);
      await this.#backend.deleteKeys(this.#ns);
    });
  }

  /**
   * Access was revoked: purge, and refuse to serve anything afterwards.
   *
   * The door cannot tell a device the difference between "you are revoked"
   * and "you may not call this op" — both are `access_denied`, deliberately
   * uniform so a token cannot enumerate (`ordo-host/src/platform_ops.rs`).
   * So the decision of *when* a refusal means revocation belongs to the
   * caller; this is only what happens once it has decided.
   */
  async poison(): Promise<void> {
    this.#poisoned = true;
    // Whatever keeps rows in memory drops them now, before the purge starts,
    // so memory never outlives the latch (C7).
    for (const listener of [...this.#onPoison]) listener();
    await this.purge();
  }

  /**
   * Called synchronously when the replica is poisoned, before its purge
   * starts. Returns the function that stops listening.
   */
  onPoison(listener: () => void): () => void {
    this.#onPoison.add(listener);
    return () => {
      this.#onPoison.delete(listener);
    };
  }

  /**
   * The replica's namespace: a digest of its space and person, the same in
   * every tab. Tabs name their shared channel and locks by it.
   */
  get namespace(): string {
    return this.#ns;
  }

  /** Whether this replica has been poisoned and serves nothing. */
  get poisoned(): boolean {
    return this.#poisoned;
  }

  /** Record that the synced op `op` holds this row (C4c). */
  async hold(op: string, entity: string, id: string): Promise<void> {
    await this.#write([{ kind: 'hold', op, entity, id }]);
  }

  /**
   * The synced op `op` stopped holding this row at `position`. The row is
   * removed at that position once no synced op holds it; true when it was.
   */
  async release(op: string, entity: string, id: string, position: number): Promise<boolean> {
    const { results } = await this.#write([{ kind: 'release', op, entity, id, position }]);
    return results[0] ?? false;
  }

  /**
   * Apply one sync frame for `holder` in one write (H4): put and hold each
   * upsert, then release each release, then remember the result's hash. C3
   * holds row by row, exactly as {@link put} and {@link release} apply it.
   * Returns what changed, taken from the write itself, so a caller keeping
   * rows in memory needs no read of its own.
   */
  async applyFrame(holder: string, frame: Frame): Promise<FrameDelta> {
    const ups = frame.upserts ?? [];
    const rels = frame.releases ?? [];
    const changes: Change[] = [];
    for (const u of ups) {
      changes.push({ kind: 'put', record: { entity: u.entity, id: u.id, position: frame.position, row: u.row } });
      changes.push({ kind: 'hold', op: holder, entity: u.entity, id: u.id });
    }
    for (const r of rels) {
      changes.push({ kind: 'release', op: holder, entity: r.entity, id: r.id, position: frame.position });
    }
    if (frame.result) changes.push({ kind: 'result', op: holder, ...frame.result });
    const { refused, results, slots } = await this.#write(changes);
    if (refused) return { upserts: [], releases: [] };
    const upserts: HeldRow[] = [];
    const seen = new Set<string>();
    for (const u of ups) {
      const r = refOf(u.entity, u.id);
      if (seen.has(r)) continue;
      seen.add(r);
      const stored = slots.get(r)?.stored;
      if (stored) upserts.push({ entity: stored.entity, id: stored.id, position: stored.position, row: stored.row });
    }
    const releases = rels.map((r, i) => ({ entity: r.entity, id: r.id, removed: results[ups.length * 2 + i] ?? false }));
    return { upserts, releases };
  }

  /**
   * Remember the server's hash of an op's whole result, so a reconnect can
   * say what it already has (`plan-replica-feed.md` C4d).
   *
   * Stored against the op's version as well as its name: a redeployed op is
   * a different result shape, and a hash minted under the old one means
   * nothing. The device never computes this hash — it is the server's, over
   * the server's own serialization — it only gives it back.
   */
  async rememberResult(op: string, version: string, hash: string): Promise<void> {
    await this.#write([{ kind: 'result', op, version, hash }]);
  }

  /** The remembered hash for `op`, when one was stored under this version. */
  async resultHash(op: string, version: string): Promise<string | undefined> {
    if (this.#poisoned) return undefined;
    const sealed = await this.#backend.get(await this.#resultKey(op));
    if (!sealed || this.#poisoned) return undefined;
    const held = (await this.#open(sealed)) as { version?: string; hash?: string };
    return held.version === version ? held.hash : undefined;
  }

  /**
   * Note that the holder `op` is in use now (`plan-sdk-device.md` §6b). The
   * time is sealed like everything else. {@link eraseUnused} reads it.
   */
  async touch(op: string, at: number = Date.now()): Promise<void> {
    await this.#write([{ kind: 'used', op, at }]);
  }

  /** When the holder `op` was last noted in use, or `undefined` when never. */
  async lastUsed(op: string): Promise<number | undefined> {
    if (this.#poisoned) return undefined;
    const sealed = await this.#backend.get(await this.#usedKey(op));
    if (!sealed || this.#poisoned) return undefined;
    return ((await this.#open(sealed)) as { at?: number }).at;
  }

  /**
   * Let go of every holder last used before `before`, as if each were
   * dropped: a row no other holder keeps leaves the device, and the holder's
   * result hash and last-used time go too. All of it under the replica lock,
   * so a tab noting a holder in use meanwhile waits, and is seen. A holder
   * with no last-used time (one synced before these were kept) is given
   * "now" first, once per replica (owner, 2026-10-21), so it is erased after
   * the same period as the rest. Returns the holders let go.
   */
  async eraseUnused(before: number): Promise<string[]> {
    return this.#serial(async () => {
      if (this.#poisoned) return [];
      await this.#stampLegacy();
      const found = await this.#backend.entries(await this.#usedPrefix());
      const used = await Promise.all(found.map(([, sealed]) => this.#open(sealed) as Promise<{ holder: string; at: number }>));
      const stale = used.filter((u) => typeof u.at === 'number' && u.at < before).map((u) => u.holder);
      for (const holder of stale) {
        const holds = await this.holdings(holder);
        await this.#writeLocked([
          ...holds.map((h): Change => ({ kind: 'forget', op: holder, entity: h.entity, id: h.id })),
          { kind: 'result', op: holder, version: '', hash: null },
          { kind: 'used', op: holder, at: null },
        ]);
      }
      return stale;
    });
  }

  /**
   * Give every holder saved before last-used times existed a last-used time
   * of now. Op names are HMAC'd into key names, so holders cannot be listed
   * by prefix; the sealed per-row holders records (the only sealed arrays
   * under the namespace) name their ops, so one scan finds them. A sealed
   * marker records that the scan ran, so later opens cost one read. Holders
   * with a result hash but no rows are not found, and have nothing to erase.
   * The caller holds the replica lock.
   */
  async #stampLegacy(): Promise<void> {
    const marker = `${this.#ns}/${await nameOf(this.#keys, ['meta', 'legacy-used-stamped'])}`;
    if (await this.#backend.get(marker)) return;
    const found = await this.#backend.entries(`${this.#ns}/`);
    const opened = await Promise.all(found.map(([, sealed]) => this.#open(sealed)));
    if (this.#poisoned) return;
    const ops = new Set<string>();
    for (const v of opened) if (Array.isArray(v)) for (const op of v) if (typeof op === 'string') ops.add(op);
    const now = Date.now();
    const unstamped: string[] = [];
    await Promise.all(
      [...ops].map(async (op) => {
        if (!(await this.#backend.get(await this.#usedKey(op)))) unstamped.push(op);
      }),
    );
    if (unstamped.length > 0) await this.#writeLocked(unstamped.map((op): Change => ({ kind: 'used', op, at: now })));
    await this.#backend.write([[marker, await this.#seal({ done: true })]], []);
  }

  /** Every row the synced op `op` holds. One read of the backend. */
  async holdings(op: string): Promise<{ entity: string; id: string }[]> {
    if (this.#poisoned) return [];
    const found = await this.#backend.entries(await this.#holdPrefix(op));
    const out = await Promise.all(
      found.map(([, sealed]) => this.#open(sealed) as Promise<{ entity: string; id: string }>),
    );
    return this.#poisoned ? [] : out;
  }

  /**
   * Every visible row the synced op `op` holds, with its position: its holds
   * in one read, then their rows in one more.
   */
  async held(op: string): Promise<ReplicaRecord[]> {
    const holds = await this.holdings(op);
    if (holds.length === 0) return [];
    const keys = await Promise.all(holds.map((h) => this.#key(h.entity, h.id)));
    const sealed = await this.#backend.getMany(keys);
    const opened = await Promise.all(sealed.map((s) => (s ? (this.#open(s) as Promise<Stored>) : undefined)));
    if (this.#poisoned) return [];
    const out: ReplicaRecord[] = [];
    for (const s of opened) {
      const r = visible(s);
      if (r) out.push(r);
    }
    return out;
  }

  /**
   * One write, in steps, because an IndexedDB transaction commits once it
   * has no request pending, so it cannot wait on WebCrypto: name the records,
   * read the ones held in one read, decide every change in memory, seal, then
   * put and delete everything in one write.
   *
   * `results` has one entry per change: for a put, whether it applied; for a
   * release, whether the row left the replica; true otherwise. A poisoned
   * replica writes nothing and answers false throughout.
   */
  #write(changes: readonly Change[]): Promise<Written> {
    return this.#serial(() => this.#writeLocked(changes));
  }

  /** {@link #write}'s body, for a caller already holding the replica lock. */
  async #writeLocked(changes: readonly Change[]): Promise<Written> {
    {
      if (this.#poisoned) return { refused: true, results: changes.map(() => false), slots: new Map() };
      // 1. Name each row's two records.
      const rows = new Map<string, { entity: string; id: string }>();
      for (const c of changes) {
        if (c.kind === 'put') rows.set(refOf(c.record.entity, c.record.id), c.record);
        else if (c.kind !== 'result' && c.kind !== 'used') rows.set(refOf(c.entity, c.id), c);
      }
      const list = [...rows.values()];
      const names = await Promise.all(
        list.map(async ({ entity, id }) => [await this.#key(entity, id), await this.#holdersKey(entity, id)] as const),
      );
      // 2. Read what is held, in one read.
      const sealed = list.length === 0 ? [] : await this.#backend.getMany(names.flat());
      const opened = await Promise.all(sealed.map((s) => (s ? this.#open(s) : undefined)));
      const slots = new Map<string, Slot>();
      list.forEach(({ entity, id }, i) => {
        slots.set(refOf(entity, id), {
          key: names[i]![0],
          holdersKey: names[i]![1],
          stored: opened[2 * i] as Stored | undefined,
          holders: (opened[2 * i + 1] as string[] | undefined) ?? [],
          rowDirty: false,
          holdersDirty: false,
        });
      });
      // 3. Decide, in order, in memory.
      const results: boolean[] = [];
      const intents: Intent[] = [];
      const apply = (slot: Slot, next: Stored): boolean => {
        if (slot.stored && slot.stored.position > next.position) return false;
        slot.stored = next;
        slot.rowDirty = true;
        return true;
      };
      for (const c of changes) {
        if (c.kind === 'put') {
          results.push(apply(slots.get(refOf(c.record.entity, c.record.id))!, c.record));
        } else if (c.kind === 'hold') {
          const slot = slots.get(refOf(c.entity, c.id))!;
          if (!slot.holders.includes(c.op)) {
            slot.holders = [...slot.holders, c.op];
            slot.holdersDirty = true;
            intents.push({ name: 'hold', op: c.op, entity: c.entity, id: c.id, value: { entity: c.entity, id: c.id } });
          }
          results.push(true);
        } else if (c.kind === 'release') {
          const slot = slots.get(refOf(c.entity, c.id))!;
          intents.push({ name: 'hold', op: c.op, entity: c.entity, id: c.id, value: null });
          slot.holders = slot.holders.filter((o) => o !== c.op);
          slot.holdersDirty = true;
          results.push(
            slot.holders.length > 0 ? false : apply(slot, { entity: c.entity, id: c.id, position: c.position, row: null }),
          );
        } else if (c.kind === 'forget') {
          const slot = slots.get(refOf(c.entity, c.id))!;
          intents.push({ name: 'hold', op: c.op, entity: c.entity, id: c.id, value: null });
          slot.holders = slot.holders.filter((o) => o !== c.op);
          slot.holdersDirty = true;
          const gone = slot.holders.length === 0 && slot.stored !== undefined;
          if (gone) {
            slot.stored = undefined;
            slot.rowDirty = true;
          }
          results.push(gone);
        } else if (c.kind === 'used') {
          intents.push({ name: 'used', op: c.op, value: c.at === null ? null : { holder: c.op, at: c.at } });
          results.push(true);
        } else {
          intents.push({ name: 'result', op: c.op, value: c.hash === null ? null : { version: c.version, hash: c.hash } });
          results.push(true);
        }
      }
      // 4. Seal, then write it all at once. The last change to a record wins.
      const named = await Promise.all(
        intents.map((t) =>
          t.name === 'hold' ? this.#holdKey(t.op, t.entity, t.id) : t.name === 'used' ? this.#usedKey(t.op) : this.#resultKey(t.op),
        ),
      );
      const final = new Map<string, unknown>();
      intents.forEach((t, i) => final.set(named[i]!, t.value));
      for (const slot of slots.values()) {
        if (slot.rowDirty) final.set(slot.key, slot.stored ?? null);
        if (slot.holdersDirty) final.set(slot.holdersKey, slot.holders.length > 0 ? slot.holders : null);
      }
      const deletes: string[] = [];
      const pending: Promise<readonly [string, Uint8Array]>[] = [];
      for (const [key, value] of final) {
        if (value === null) deletes.push(key);
        else pending.push(this.#seal(value).then((v) => [key, v] as const));
      }
      await this.#backend.write(await Promise.all(pending), deletes);
      return { refused: false, results, slots };
    }
  }

  async #seal(value: unknown): Promise<Uint8Array> {
    return sealBytes(this.#keys, new TextEncoder().encode(JSON.stringify(value)));
  }

  async #open(sealed: Uint8Array): Promise<unknown> {
    return JSON.parse(new TextDecoder().decode(await openBytes(this.#keys, sealed))) as unknown;
  }

  /** A prefix's name, computed once for the replica's life. */
  #memo(parts: readonly string[]): Promise<string> {
    const k = JSON.stringify(parts);
    let found = this.#prefixes.get(k);
    if (!found) {
      found = nameOf(this.#keys, parts).then((n) => `${this.#ns}/${n}/`);
      this.#prefixes.set(k, found);
    }
    return found;
  }

  async #resultKey(op: string): Promise<string> {
    return `${this.#ns}/${await nameOf(this.#keys, ['result', op])}`;
  }

  #usedPrefix(): Promise<string> {
    return this.#memo(['used']);
  }

  async #usedKey(op: string): Promise<string> {
    return `${await this.#usedPrefix()}${await nameOf(this.#keys, ['used', op])}`;
  }

  #holdPrefix(op: string): Promise<string> {
    return this.#memo(['holds', op]);
  }

  async #holdKey(op: string, entity: string, id: string): Promise<string> {
    return `${await this.#holdPrefix(op)}${await nameOf(this.#keys, ['hold', op, entity, id])}`;
  }

  async #holdersKey(entity: string, id: string): Promise<string> {
    return `${this.#ns}/${await nameOf(this.#keys, ['holders', entity, id])}`;
  }

  #prefix(entity: string): Promise<string> {
    return this.#memo(['entity', entity]);
  }

  async #key(entity: string, id: string): Promise<string> {
    return `${await this.#prefix(entity)}${await nameOf(this.#keys, ['row', entity, id])}`;
  }

  /**
   * One write at a time on this instance, and one at a time across every tab
   * and worker on this replica: a write reads, decides and then writes in
   * separate transactions, so two tabs could each decide from the same old
   * record and the later write would drop the earlier one's holder. The Web
   * Lock named for the replica's namespace makes the three steps one unit.
   * Where there is no `navigator.locks` (Node, an old browser) it falls back
   * to the instance queue alone.
   */
  #serial<T>(work: () => Promise<T>): Promise<T> {
    const locked = () => withReplicaLock(`ordo-replica:${this.#ns}`, work);
    const run = this.#queue.then(locked, locked);
    this.#queue = run.catch(() => undefined);
    return run;
  }
}

function refOf(entity: string, id: string): string {
  return JSON.stringify([entity, id]);
}

function visible(s: Stored | undefined): ReplicaRecord | undefined {
  return s && s.row ? { entity: s.entity, id: s.id, position: s.position, row: s.row } : undefined;
}

interface LockManagerLike {
  request<T>(name: string, callback: () => Promise<T>): Promise<T>;
}

/** Run `work` under the Web Lock `name` when the platform has one. */
function withReplicaLock<T>(name: string, work: () => Promise<T>): Promise<T> {
  const locks = (globalThis as { navigator?: { locks?: LockManagerLike } }).navigator?.locks;
  return locks && typeof locks.request === 'function' ? locks.request(name, work) : work();
}
