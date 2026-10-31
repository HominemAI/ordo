/**
 * Where a replica's sealed records live: a small key-value store.
 *
 * Values are opaque sealed bytes and keys are keyed hashes, so a backend never
 * sees a row, an id or a parameter. Everything that has to understand a record
 * lives above it in `LocalReplica`, which is why the IndexedDB backend can stay
 * this thin.
 */

import type { DeviceKeys } from './seal.js';

export interface Backend {
  get(key: string): Promise<Uint8Array | undefined>;
  put(key: string, value: Uint8Array): Promise<void>;
  delete(key: string): Promise<void>;
  /** Every record key beginning with `prefix`, sorted. */
  keys(prefix: string): Promise<string[]>;
  /**
   * Every record beginning with `prefix`, as `[key, value]` sorted by key, in
   * one read: a write either happened before all of it or after all of it.
   */
  entries(prefix: string): Promise<[string, Uint8Array][]>;
  /** Delete every record beginning with `prefix`, in one write. */
  deletePrefix(prefix: string): Promise<void>;
  /** Several records, in one read, in the order asked. */
  getMany(keys: readonly string[]): Promise<(Uint8Array | undefined)[]>;
  /**
   * Put and delete several records in one write: all of it lands or none
   * does, and a read sees it whole or not at all (H4).
   */
  write(puts: readonly (readonly [string, Uint8Array])[], deletes: readonly string[]): Promise<void>;
  /** The keys one replica is sealed under, by its opaque id. */
  loadKeys(id: string): Promise<DeviceKeys | undefined>;
  saveKeys(id: string, keys: DeviceKeys): Promise<void>;
  deleteKeys(id: string): Promise<void>;
}

/** A backend in memory: for tests, and for runtimes with no IndexedDB. */
export class MemoryBackend implements Backend {
  readonly #records = new Map<string, Uint8Array>();
  readonly #keys = new Map<string, DeviceKeys>();

  async get(key: string): Promise<Uint8Array | undefined> {
    return this.#records.get(key);
  }

  async put(key: string, value: Uint8Array): Promise<void> {
    this.#records.set(key, value);
  }

  async delete(key: string): Promise<void> {
    this.#records.delete(key);
  }

  async keys(prefix: string): Promise<string[]> {
    return [...this.#records.keys()].filter((k) => k.startsWith(prefix)).sort();
  }

  async entries(prefix: string): Promise<[string, Uint8Array][]> {
    return [...this.#records]
      .filter(([k]) => k.startsWith(prefix))
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  }

  async deletePrefix(prefix: string): Promise<void> {
    for (const k of [...this.#records.keys()]) if (k.startsWith(prefix)) this.#records.delete(k);
  }

  async getMany(keys: readonly string[]): Promise<(Uint8Array | undefined)[]> {
    return keys.map((k) => this.#records.get(k));
  }

  async write(puts: readonly (readonly [string, Uint8Array])[], deletes: readonly string[]): Promise<void> {
    for (const k of deletes) this.#records.delete(k);
    for (const [k, v] of puts) this.#records.set(k, v);
  }

  async loadKeys(id: string): Promise<DeviceKeys | undefined> {
    return this.#keys.get(id);
  }

  async saveKeys(id: string, keys: DeviceKeys): Promise<void> {
    this.#keys.set(id, keys);
  }

  async deleteKeys(id: string): Promise<void> {
    this.#keys.delete(id);
  }

  /** Every stored record, as a copied disk would see it. */
  dump(): ReadonlyMap<string, Uint8Array> {
    return new Map(this.#records);
  }
}

/**
 * A backend in the browser's IndexedDB: the default on a device.
 *
 * Two object stores: `records` for sealed records, and `keys` for the
 * replicas' non-extractable `CryptoKey`s, which IndexedDB stores as keys the
 * page can use and cannot read.
 */
export class IndexedDbBackend implements Backend {
  readonly #db: IDBDatabase;
  /** The runtime's `IDBKeyRange`, which a prefix's range is built with. */
  readonly #ranges: typeof IDBKeyRange | undefined;

  private constructor(db: IDBDatabase, ranges: typeof IDBKeyRange | undefined) {
    this.#db = db;
    this.#ranges = ranges;
  }

  /**
   * Open the database. `ranges` is the `IDBKeyRange` of the same IndexedDB
   * as `factory`: the page's own by default. A runtime with neither (a test
   * passing only a factory) still works, scanning every key and filtering,
   * which is what every scan did before key ranges (H3).
   */
  static async open(
    name = 'ordo-replica',
    factory: IDBFactory | undefined = globalThis.indexedDB,
    ranges: typeof IDBKeyRange | undefined = globalThis.IDBKeyRange,
  ): Promise<IndexedDbBackend> {
    if (!factory) throw new Error('IndexedDB is not available in this runtime; use MemoryBackend');
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const req = factory.open(name, 1);
      req.onupgradeneeded = () => {
        const d = req.result;
        if (!d.objectStoreNames.contains('records')) d.createObjectStore('records');
        if (!d.objectStoreNames.contains('keys')) d.createObjectStore('keys');
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error ?? new Error('opening IndexedDB failed'));
    });
    return new IndexedDbBackend(db, ranges);
  }

  /** One request in its own transaction, resolved when the transaction commits. */
  #run<T>(
    store: 'records' | 'keys',
    mode: IDBTransactionMode,
    op: (s: IDBObjectStore) => IDBRequest<T>,
  ): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const tx = this.#db.transaction(store, mode);
      const req = op(tx.objectStore(store));
      tx.oncomplete = () => resolve(req.result);
      tx.onerror = () => reject(tx.error ?? new Error('an IndexedDB transaction failed'));
      tx.onabort = () => reject(tx.error ?? new Error('an IndexedDB transaction aborted'));
    });
  }

  async get(key: string): Promise<Uint8Array | undefined> {
    const v: unknown = await this.#run('records', 'readonly', (s) => s.get(key));
    return v instanceof Uint8Array ? v : undefined;
  }

  async put(key: string, value: Uint8Array): Promise<void> {
    await this.#run('records', 'readwrite', (s) => s.put(value, key));
  }

  async delete(key: string): Promise<void> {
    await this.#run('records', 'readwrite', (s) => s.delete(key));
  }

  /**
   * Keys and values over the prefix's range, which IndexedDB returns in key
   * order. No filter and no sort runs on the page (H3).
   */
  async keys(prefix: string): Promise<string[]> {
    const r = this.#range(prefix);
    const all = await this.#run('records', 'readonly', (s) => s.getAllKeys(r));
    return r === undefined && prefix !== '' ? (all as string[]).filter((k) => k.startsWith(prefix)) : (all as string[]);
  }

  async entries(prefix: string): Promise<[string, Uint8Array][]> {
    return new Promise((resolve, reject) => {
      const tx = this.#db.transaction('records', 'readonly');
      const store = tx.objectStore('records');
      // Two requests in one transaction see the same records, in the same
      // (key) order, so they pair up by index.
      const r = this.#range(prefix);
      const keys = store.getAllKeys(r);
      const values = store.getAll(r);
      tx.oncomplete = () => {
        const out: [string, Uint8Array][] = [];
        const ks = keys.result as string[];
        const vs = values.result as unknown[];
        for (let i = 0; i < ks.length; i++) {
          const v = vs[i];
          if (v instanceof Uint8Array && (r !== undefined || ks[i]!.startsWith(prefix))) out.push([ks[i]!, v]);
        }
        resolve(out);
      };
      tx.onerror = () => reject(tx.error ?? new Error('an IndexedDB transaction failed'));
      tx.onabort = () => reject(tx.error ?? new Error('an IndexedDB transaction aborted'));
    });
  }

  async deletePrefix(prefix: string): Promise<void> {
    const r = this.#range(prefix);
    if (r !== undefined || prefix === '') {
      await this.#run('records', 'readwrite', (s) => (r === undefined ? s.clear() : s.delete(r)));
      return;
    }
    const keys = await this.keys(prefix);
    await new Promise<void>((resolve, reject) => {
      const tx = this.#db.transaction('records', 'readwrite');
      const store = tx.objectStore('records');
      for (const k of keys) store.delete(k);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error ?? new Error('an IndexedDB transaction failed'));
      tx.onabort = () => reject(tx.error ?? new Error('an IndexedDB transaction aborted'));
    });
  }

  async getMany(keys: readonly string[]): Promise<(Uint8Array | undefined)[]> {
    if (keys.length === 0) return [];
    return new Promise((resolve, reject) => {
      const tx = this.#db.transaction('records', 'readonly');
      const store = tx.objectStore('records');
      const reqs = keys.map((k) => store.get(k));
      tx.oncomplete = () => resolve(reqs.map((r) => (r.result instanceof Uint8Array ? r.result : undefined)));
      tx.onerror = () => reject(tx.error ?? new Error('an IndexedDB transaction failed'));
      tx.onabort = () => reject(tx.error ?? new Error('an IndexedDB transaction aborted'));
    });
  }

  /**
   * One read-write transaction. Every value is sealed before this is called:
   * a transaction commits once it has no request pending, so it cannot wait
   * on WebCrypto in the middle.
   */
  async write(puts: readonly (readonly [string, Uint8Array])[], deletes: readonly string[]): Promise<void> {
    if (puts.length === 0 && deletes.length === 0) return;
    await new Promise<void>((resolve, reject) => {
      const tx = this.#db.transaction('records', 'readwrite');
      const store = tx.objectStore('records');
      for (const k of deletes) store.delete(k);
      for (const [k, v] of puts) store.put(v, k);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error ?? new Error('an IndexedDB transaction failed'));
      tx.onabort = () => reject(tx.error ?? new Error('an IndexedDB transaction aborted'));
    });
  }

  /**
   * Every key that begins with `prefix`. Record keys are lowercase hex and
   * `/` (`nameOf` and `digestOf` are hex), so every one of them sorts below
   * `\uffff` and the range is exact.
   */
  #range(prefix: string): IDBKeyRange | undefined {
    if (prefix === '' || !this.#ranges) return undefined;
    return this.#ranges.bound(prefix, `${prefix}\uffff`);
  }

  async loadKeys(id: string): Promise<DeviceKeys | undefined> {
    const v: unknown = await this.#run('keys', 'readonly', (s) => s.get(id));
    return (v ?? undefined) as DeviceKeys | undefined;
  }

  async saveKeys(id: string, keys: DeviceKeys): Promise<void> {
    await this.#run('keys', 'readwrite', (s) => s.put(keys, id));
  }

  async deleteKeys(id: string): Promise<void> {
    await this.#run('keys', 'readwrite', (s) => s.delete(id));
  }

  close(): void {
    this.#db.close();
  }
}

