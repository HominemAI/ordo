/**
 * A backend that counts its transactions: one per call, as `IndexedDbBackend`
 * opens one per call. Shared by the budget tests and the store tests.
 */

import { MemoryBackend } from '../src/index.js';

export class CountingBackend extends MemoryBackend {
  /** Read-write transactions. */
  writes = 0;
  /** Read-only transactions. */
  reads = 0;
  /** Record keys read, across every read transaction. */
  keysRead = 0;

  override async get(key: string): Promise<Uint8Array | undefined> {
    this.reads += 1;
    this.keysRead += 1;
    return super.get(key);
  }

  override async put(key: string, value: Uint8Array): Promise<void> {
    this.writes += 1;
    return super.put(key, value);
  }

  override async delete(key: string): Promise<void> {
    this.writes += 1;
    return super.delete(key);
  }

  override async keys(prefix: string): Promise<string[]> {
    this.reads += 1;
    return super.keys(prefix);
  }

  override async entries(prefix: string): Promise<[string, Uint8Array][]> {
    this.reads += 1;
    const out = await super.entries(prefix);
    this.keysRead += out.length;
    return out;
  }

  override async deletePrefix(prefix: string): Promise<void> {
    this.writes += 1;
    return super.deletePrefix(prefix);
  }

  override async getMany(keys: readonly string[]): Promise<(Uint8Array | undefined)[]> {
    this.reads += 1;
    this.keysRead += keys.length;
    return super.getMany(keys);
  }

  override async write(puts: readonly (readonly [string, Uint8Array])[], deletes: readonly string[]): Promise<void> {
    this.writes += 1;
    return super.write(puts, deletes);
  }

  reset(): void {
    this.writes = 0;
    this.reads = 0;
    this.keysRead = 0;
  }
}
