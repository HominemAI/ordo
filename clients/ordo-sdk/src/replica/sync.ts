/**
 * Keeping one op in sync on the device (`plan-replica-feed.md` C4c).
 *
 * {@link syncOp} opens the op's sync feed (`?live=1&sync=1`, C4b) and writes
 * its frames into the device replica:
 *
 * - A `load` frame puts each row at the frame's position and records that
 *   this op holds it. When the load completes, the rows this op held before
 *   it and the load did not include are released, because a complete load is
 *   the op's whole result. That is how deletes made while the device was away
 *   reach it on reconnect.
 * - A `change` frame puts its upserts and releases its deletes, at its
 *   position. A delete means the row is no longer in this op's result for this
 *   person, whatever the cause, so a guard change removes rows the same way.
 * - A row leaves the replica only when no synced op holds it.
 *
 * Until a load completes the op's state says so, and an answer taken from it
 * may be partial (ruled 2026-09-15, `feed-cap`). Rows only move forward: the
 * replica never replaces a version with one read at a lower position (C3).
 */

import type { OrdoClient, Params, Pin, Row } from '../core/client.js';
import { OrdoLocalRefusal, OrdoRefusal, decodeRefusal } from '../core/refusal.js';
import type { Refine } from '../core/refine.js';
import type { OpSpec } from '../core/spec.js';
import type { FrameDelta, LocalReplica } from './replica.js';

/** Where one synced op stands after the last frame it applied. */
export interface SyncState {
  readonly op: string;
  /** Rows the current load has delivered so far. */
  readonly loaded: number;
  /**
   * False until the current load's last page has arrived. An answer taken
   * from an op that is still loading may be partial.
   */
  readonly complete: boolean;
  /** The position of the last frame applied. */
  readonly position: number | null;
  /** The signed head at that position, when the door attests. */
  readonly pin: Pin | null;
}

/** What one frame changed on the device, for a caller that keeps rows in memory. */
export interface SyncDelta extends FrameDelta {
  /**
   * True on the first frame of a load. The rows held before it stay until
   * the load completes, and the completing frame releases the ones it left out.
   */
  readonly reset: boolean;
  /**
   * On the frame that completes a load, every row this holder holds once the
   * frame is applied: the load's own rows, or on a resume what the replica
   * holds. A caller keeping the holder's rows in memory makes its set exactly
   * this, so a row another writer released (an earlier manager, another tab)
   * leaves memory too. Absent on every other frame.
   */
  readonly members?: readonly { readonly entity: string; readonly id: string }[];
}

export interface SyncOptions {
  /** Aborting it ends the sync; the promise rejects with the abort's reason. */
  readonly signal?: AbortSignal;
  /**
   * Called after each frame is applied, with the state it reached and what
   * it changed. The frame is one write, so the delta is all of it or nothing.
   */
  readonly onFrame?: (state: SyncState, delta: SyncDelta) => void;
  /**
   * What the replica records as holding these rows; the op's name by default.
   * The sync manager (C5) passes its own handle, so one op synced at two sets
   * of parameters holds its rows apart and dropping one keeps the other's.
   */
  readonly holder?: string;
  /**
   * Sync only the rows this `where` matches (R6). A row that stops matching
   * arrives as a delete and one that starts as an upsert. A `pick` is
   * refused: the device stores whole rows, and picks on read.
   */
  readonly refine?: Refine;
}

interface LoadFrame {
  readonly entity: string;
  readonly key: string;
  readonly position: number | null;
  readonly pin: Pin | null;
  readonly rows: readonly Row[];
  readonly loaded: number;
  readonly complete: boolean;
  /** The server's hash of the whole result, on the completing frame (C4d). */
  readonly result?: string | null;
  /** This completing frame is a resume: the device keeps what it holds. */
  readonly resumed?: boolean;
}

interface ChangeFrame {
  readonly entity: string;
  readonly key: string;
  readonly position: number | null;
  readonly pin: Pin | null;
  readonly upserts: readonly Row[];
  readonly deletes: readonly string[];
  /**
   * The server's hash of the whole result after this change (C4d). Kept, so
   * a reconnect resumes on the result the device now holds and never on the
   * one its last load ended at.
   */
  readonly result?: string | null;
}

/**
 * Keep `spec` in sync in `replica` until the feed ends. Resolves with the
 * last state when the server closes the stream; call it again to reconnect,
 * which runs a new load.
 */
/**
 * Results a device must never mirror (`plan-client-cache.md` §6): a stolen
 * device must not yield the permission map or the audit spine.
 *
 * `std.space.*` is the permission map and is named here. Command-log slices
 * are the other half of the rule and have no client op to name today, so
 * this covers what exists; an op that ever exposes a log slice belongs in
 * this list on the day it ships.
 *
 * The device refuses rather than relying on the server to: the rule is about
 * what may rest on this disk, which is the device's own business.
 */
const NEVER_MIRRORED = ['std.space.'];

export async function syncOp(
  client: OrdoClient,
  replica: LocalReplica,
  spec: OpSpec,
  params: Params = {},
  options: SyncOptions = {},
): Promise<SyncState> {
  if (!spec.sync) {
    throw new OrdoLocalRefusal({
      class: 'not_live',
      message: `op \`${spec.op}\` cannot be synced: its rows have no identity to key them by`,
      op: spec.op,
      param: null,
    });
  }
  if (NEVER_MIRRORED.some((prefix) => spec.op.startsWith(prefix))) {
    throw new OrdoLocalRefusal({
      class: 'not_live',
      message:
        `op \`${spec.op}\` is never mirrored: a device must not hold the space's permission map ` +
        `or its audit spine (plan-client-cache.md §6)`,
      op: spec.op,
      param: null,
    });
  }
  const holder = options.holder ?? spec.op;
  // What this device already holds, if it was told (C4d). Scoped to the op's
  // version: a redeployed op is a different result and the old hash means
  // nothing.
  const resume = await replica.resultHash(holder, spec.version);
  const response = await client.openFeed(spec, params, options.signal, resume, options.refine);
  const before = new Set((await replica.holdings(holder)).map((h) => ref(h.entity, h.id)));
  const seen = new Set<string>();
  let state: SyncState = { op: spec.op, loaded: 0, complete: false, position: null, pin: null };
  let first = true;

  // Each frame is one write of the replica (H4): its puts, holds, releases
  // and the result's hash land together or not at all.
  for await (const { event, data } of frames(response, (e) => client.feedBroken(e))) {
    if (event === 'error') {
      throw new OrdoRefusal(decodeRefusal(data, 500, spec.op, spec.refusals));
    }
    let delta: FrameDelta;
    let reset = false;
    let members: { entity: string; id: string }[] | undefined;
    if (event === 'load') {
      const f = data as LoadFrame;
      const position = positionOf(f, spec.op);
      const upserts = f.rows.map((row) => ({ entity: f.entity, id: idOf(row, f.key, spec.op), row }));
      for (const u of upserts) seen.add(ref(u.entity, u.id));
      const releases: { entity: string; id: string }[] = [];
      // A resumed frame carries no rows BECAUSE nothing changed, so the "a
      // complete load is the whole result" rule must not fire: applying it
      // here would release every row the op holds on every successful resume.
      if (f.complete && !f.resumed) {
        for (const r of before) {
          if (seen.has(r)) continue;
          const [entity, id] = JSON.parse(r) as [string, string];
          releases.push({ entity, id });
        }
      }
      delta = await replica.applyFrame(holder, {
        position,
        upserts,
        releases,
        ...(f.complete && f.result ? { result: { version: spec.version, hash: f.result } } : {}),
      });
      if (f.complete) {
        members = f.resumed
          ? await replica.holdings(holder)
          : [...seen].map((r) => {
              const [entity, id] = JSON.parse(r) as [string, string];
              return { entity, id };
            });
      }
      reset = first;
      first = false;
      state = { op: spec.op, loaded: f.loaded, complete: f.complete, position, pin: f.pin ?? null };
    } else if (event === 'change') {
      const f = data as ChangeFrame;
      const position = positionOf(f, spec.op);
      delta = await replica.applyFrame(holder, {
        position,
        upserts: f.upserts.map((row) => ({ entity: f.entity, id: idOf(row, f.key, spec.op), row })),
        releases: f.deletes.map((id) => ({ entity: f.entity, id })),
        // The hash the device holds must describe what it holds. A server
        // that sends none leaves the device nothing to resume on, so the
        // next connection loads in full rather than resume on a stale one.
        result: { version: spec.version, hash: typeof f.result === 'string' ? f.result : null },
      });
      state = { ...state, position, pin: f.pin ?? null };
    } else {
      continue;
    }
    options.onFrame?.(state, members ? { ...delta, reset, members } : { ...delta, reset });
  }
  return state;
}

function ref(entity: string, id: string): string {
  return JSON.stringify([entity, id]);
}

/** The replica orders versions by position, so a frame without one cannot be applied. */
function positionOf(f: { position: number | null; pin: Pin | null }, op: string): number {
  const position = f.position ?? f.pin?.position ?? null;
  if (position === null) throw new Error(`a sync frame for \`${op}\` arrived without a position`);
  return position;
}

function idOf(row: Row, key: string, op: string): string {
  const id = (row as Record<string, unknown>)[key];
  if (typeof id === 'string') return id;
  if (typeof id === 'number') return String(id);
  throw new Error(`a synced row of \`${op}\` arrived without its \`${key}\``);
}

/** The server-sent events of a response body, one at a time. */
async function* frames(
  response: Response,
  broken: (e: unknown) => Error | null,
): AsyncGenerator<{ event: string; data: unknown }> {
  if (!response.body) return;
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  /** How far into `buffer` has been searched for a blank line. */
  let scanned = 0;
  for (;;) {
    let chunk: ReadableStreamReadResult<Uint8Array>;
    try {
      chunk = await reader.read();
    } catch (e) {
      // A connection lost mid-stream is a network failure, named as one.
      throw broken(e) ?? e;
    }
    const { value, done } = chunk;
    buffer += done ? decoder.decode() : decoder.decode(value, { stream: true });
    // Search only what arrived since the last look (L2): a frame split over
    // many chunks costs one pass over its bytes, not one per chunk. One
    // character back, since a chunk may end between the two newlines.
    let start = 0;
    let end: number;
    while ((end = buffer.indexOf('\n\n', Math.max(start, scanned - 1))) >= 0) {
      const frame = parse(buffer.slice(start, end));
      start = end + 2;
      scanned = start;
      if (frame) yield frame;
    }
    if (start > 0) buffer = buffer.slice(start);
    scanned = buffer.length;
    if (done) return;
  }
}

/** One event block. Comments (the keep-alive) and blocks without data are skipped. */
function parse(block: string): { event: string; data: unknown } | null {
  let event = 'message';
  const data: string[] = [];
  for (const line of block.split('\n')) {
    if (line.startsWith(':')) continue;
    const colon = line.indexOf(':');
    const field = colon < 0 ? line : line.slice(0, colon);
    let value = colon < 0 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') event = value;
    else if (field === 'data') data.push(value);
  }
  if (data.length === 0) return null;
  try {
    return { event, data: JSON.parse(data.join('\n')) as unknown };
  } catch {
    return null;
  }
}
