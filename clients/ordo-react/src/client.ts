/**
 * The door client over a descriptor.
 *
 * Every routing decision is read off the descriptor rather than guessed from
 * the op's name: the method comes from `call.method`, the legal verbs from
 * `call.terminal`, the version pin from `op.version`. `api-engine-design.md`
 * §B3's point is that a renderer needs no name heuristics, and this file is
 * where that is cashed.
 *
 * Every call goes through `@hominemai/ordo-sdk`'s one request helper
 * (`core/door.ts`): a deadline (`callTimeoutMs`), a call that got no answer
 * named as an `OrdoNetworkError`, and the one retry policy. Every refusal is
 * the SDK's `OrdoRefusal` (audit-service-interfaces.md, fix order step 7).
 *
 * The class was `OrdoClient` in 0.3. That name is the SDK's generated-op
 * client, so this one is `DescriptorClient`; `OrdoClient`,
 * `OrdoClientOptions` and `CallOptions` were deprecated aliases through 0.4
 * and are gone: `OrdoClient` is only the SDK's generated-op client.
 */

import type { OpDescriptor, ParamDescriptor } from './descriptor.js';
import type { OrdoInterface } from './load.js';
import { OrdoLocalRefusal, type RefusalClass } from '@hominemai/ordo-sdk/core/refusal.js';
import { busyRetryOf, conflictRetryOf } from '@hominemai/ordo-sdk/core/busy.js';
import type {
  OnRetry,
  ResolvedBusyRetry,
  ResolvedConflictRetry,
} from '@hominemai/ordo-sdk/core/busy.js';
import {
  NO_RETRY,
  busyOnly,
  doorRequest,
  fetchOf,
  readBody,
  refusalOf,
  type RetryPolicy,
} from '@hominemai/ordo-sdk/core/door.js';
import {
  DEFAULT_CLIENT_CALL_TIMEOUT_MS,
  networkError,
  streamError,
  type NetworkSite,
  type OrdoNetworkError,
} from '@hominemai/ordo-sdk/core/network.js';
import { checkRefine, type Refine } from '@hominemai/ordo-sdk/core/refine.js';
import type { FileDoor } from '@hominemai/ordo-sdk/core/files.js';
import {
  callFunction,
  type FunctionCallOptions,
  type FunctionResponse,
} from '@hominemai/ordo-sdk/core/function.js';
import type { JsonValue } from '@hominemai/ordo-sdk/core/json.js';

// Plain wire types, the answer's shape and the download helpers are the
// SDK's (owner ruling, 2026-10-23: ordo-react imports ordo-sdk's code and
// carries no copy).
import {
  DOWNLOAD_NAME,
  downloadName,
  opResponseOf,
  renewedBy,
  type OpResponse,
  type OrdoClientOptions as SdkClientOptions,
  type OwnDownload,
  type ParamValue,
  type Params,
  type Row,
  type TokenSource,
} from '@hominemai/ordo-sdk/core/client.js';
export { DOWNLOAD_NAME, downloadName };
export type { OpResponse, OwnDownload, ParamValue, Params, Row, TokenSource };

/**
 * What a {@link DescriptorClient} is built from: the SDK client's options
 * (the door, the space, the token, the deadline, the retry policy), and the
 * descriptor.
 */
export interface DescriptorClientOptions extends SdkClientOptions {
  readonly iface: OrdoInterface;
}

/** What one call to a descriptor op takes. */
export interface DescriptorCallOptions {
  readonly params?: Params;
  /** Opaque keyset cursor. Rides as `X-Ordo-Page`, never the query string. */
  readonly page?: string | undefined;
  readonly refine?: Refine | undefined;
  /**
   * Write only. Supply to make a retry replay rather than re-execute; omit
   * and one is minted per call. Reads ignore it — the door does too.
   */
  readonly idempotencyKey?: string | undefined;
  readonly signal?: AbortSignal | undefined;
  /**
   * Called before each wait on a `server_busy` or `serialization_conflict`
   * answer, with the retry's number for that reason (from 1), the wait in
   * milliseconds and the reason, so a screen can say it is retrying (L7).
   */
  readonly onRetry?: OnRetry | undefined;
}

export class DescriptorClient {
  readonly iface: OrdoInterface;
  readonly space: string;

  readonly #baseUrl: string;
  readonly #token: TokenSource | undefined;
  readonly #renew: (() => Promise<boolean>) | undefined;
  readonly #fetch: typeof globalThis.fetch;
  readonly #site: NetworkSite;
  readonly #options: DescriptorClientOptions;
  readonly #newKey: () => string;
  readonly #busyRetry: ResolvedBusyRetry;
  readonly #conflictRetry: ResolvedConflictRetry;

  constructor(options: DescriptorClientOptions) {
    this.iface = options.iface;
    this.space = options.space;
    this.#options = options;
    this.#baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.#token = options.token;
    this.#renew = options.renewToken;
    this.#fetch = fetchOf(options.fetch);
    this.#site = {
      baseUrl: this.#baseUrl,
      fetch: this.#fetch,
      network: options.network,
      timeoutOption: 'callTimeoutMs',
      timeoutMs: options.callTimeoutMs ?? DEFAULT_CLIENT_CALL_TIMEOUT_MS,
    };
    this.#newKey = options.newIdempotencyKey ?? (() => crypto.randomUUID());
    this.#busyRetry = busyRetryOf(options.busyRetry, options.busyRetries);
    this.#conflictRetry = conflictRetryOf(options.conflictRetry, options.conflictRetries);
  }

  /**
   * What the file helpers (`uploadFile`, `fileUrl`, `downloadFile`) need: the
   * door, the space, the token, the `fetch` this client uses, and how it
   * names a call that got no answer and retries a busy door.
   */
  fileDoor(): FileDoor {
    return {
      baseUrl: this.#baseUrl,
      space: this.space,
      token: async () => this.#currentToken(),
      fetch: this.#fetch,
      wrap: async (e) => networkError(e, this.#site),
      busyRetry: this.#busyRetry,
    };
  }

  /**
   * Mint the token again after the door refused it: `true` when a new one
   * was minted, `false` when this client cannot or the door will not. The
   * SDK's own body ({@link renewedBy}); a failure to reach the door throws.
   */
  renewToken(): Promise<boolean> {
    return renewedBy(this.#renew);
  }

  /** A client bound to a different space; everything else is shared. */
  withSpace(space: string): DescriptorClient {
    return new DescriptorClient({
      ...this.#options,
      space,
      fetch: this.#fetch,
      newIdempotencyKey: this.#newKey,
      busyRetry: this.#busyRetry,
      conflictRetry: this.#conflictRetry,
    });
  }

  /**
   * The `OrdoNetworkError` for a stream's body that broke after the door had
   * answered, or `null` for an abort the caller asked for.
   */
  feedBroken(e: unknown): OrdoNetworkError | null {
    return streamError(e, this.#site);
  }

  /**
   * Call a backend function, `POST /fn/<name>`, as the signed-in person.
   * The SDK's own call ({@link callFunction}); a refusal is an `OrdoRefusal`.
   */
  callFunction<R = JsonValue>(
    name: string,
    params: Readonly<Record<string, unknown>> = {},
    options: FunctionCallOptions = {},
  ): Promise<FunctionResponse<R>> {
    return callFunction<R>(
      {
        baseUrl: this.#baseUrl,
        space: this.space,
        token: () => this.#currentToken(),
        transport: (url, init, onRetry) =>
          this.#send(url, init, busyOnly(this.#busyRetry), onRetry),
      },
      name,
      params,
      options,
    );
  }

  /** `.fetch()` — one shot over a read op. */
  async fetchOp<R = Row>(name: string, options: DescriptorCallOptions = {}): Promise<OpResponse<R>> {
    const op = this.iface.requireOp(name);
    assertTerminal(op, 'fetch');
    return this.#call<R>(op, options);
  }

  /**
   * `.pages()` — one page of a read op, cursor threaded by the caller (or by
   * {@link pageAll} / `useOpPages`, which own the loop).
   */
  async fetchPage<R = Row>(name: string, options: DescriptorCallOptions = {}): Promise<OpResponse<R>> {
    const op = this.iface.requireOp(name);
    assertTerminal(op, 'pages');
    return this.#call<R>(op, options);
  }

  /**
   * Page to exhaustion. Stops **only** when `next` is absent — never on an
   * empty page, which is ordinary (§7).
   */
  async *pageAll<R = Row>(name: string, options: DescriptorCallOptions = {}): AsyncGenerator<OpResponse<R>, void, void> {
    let cursor = options.page;
    for (;;) {
      const res: OpResponse<R> = await this.fetchPage<R>(name, { ...options, page: cursor });
      yield res;
      if (res.next === undefined || res.next === null || res.next === '') return;
      cursor = res.next;
    }
  }

  /** `.commit()` — the one verb a write or delete has. */
  async commit<R = Row>(name: string, options: DescriptorCallOptions = {}): Promise<OpResponse<R>> {
    const op = this.iface.requireOp(name);
    assertTerminal(op, 'commit');
    return this.#call<R>(op, options);
  }

  /**
   * `.live()` — open the op's subscription, `?live=1` (`api-engine-design.md`
   * §6). The body is the event stream: the ordinary envelope first, then one
   * `invalidate` per change. A refusal at the door is thrown as an
   * `OrdoRefusal`, with the door's `Retry-After`. A refinement refines the
   * first envelope (R6); the cursor does not apply to a live session. Most
   * callers want a read hook with `live`.
   */
  async openLive(
    name: string,
    options: Pick<DescriptorCallOptions, 'params' | 'signal' | 'refine'> = {},
  ): Promise<Response> {
    const op = this.iface.requireOp(name);
    assertTerminal(op, 'live');
    standing(op);
    const headers: Record<string, string> = {
      'X-Ordo-Space': this.space,
      'X-Ordo-Version': op.version,
      Accept: 'text/event-stream',
    };
    const token = await this.#currentToken();
    if (token) headers['Authorization'] = `Bearer ${token}`;
    const q = new URLSearchParams({ live: '1' });
    for (const [k, v] of Object.entries(options.params ?? {})) {
      if (v === null || v === undefined) continue;
      q.set(k, String(v));
    }
    const refine = liveRefine(op, options.refine);
    if (refine) q.set('$refine', JSON.stringify(refine));
    // Never through the browser's HTTP cache. A stream it caches holds the
    // cache entry for its URL, and Firefox makes the next request for that
    // URL (another tab, another space, a reconnect) wait until the stream
    // ends, which a live stream never does.
    const init: RequestInit = { method: 'GET', headers, cache: 'no-store' };
    if (options.signal) init.signal = options.signal;
    // A stream is reopened by the live hub on the same classes, so it is
    // sent once here.
    const response = await this.#send(`${this.#baseUrl}/op/${encodeURIComponent(op.op)}?${q}`, init, NO_RETRY);
    if (!response.ok) throw await refusalOf(response, op.op, op.refusals);
    return response;
  }

  /**
   * Open one shared live stream, `GET /live` (`api-engine-design.md` §6.1).
   * Its first frame is `ready` with the stream's id; reads are added to it
   * with {@link addLiveRead} and each `invalidate` names a read's key.
   */
  async openLiveStream(options: { signal?: AbortSignal | undefined } = {}): Promise<Response> {
    const headers: Record<string, string> = { Accept: 'text/event-stream' };
    const token = await this.#currentToken();
    if (token) headers['Authorization'] = `Bearer ${token}`;
    // Never through the browser's HTTP cache (see `openLive`).
    const init: RequestInit = { method: 'GET', headers, cache: 'no-store' };
    if (options.signal) init.signal = options.signal;
    const response = await this.#send(`${this.#baseUrl}/live`, init, NO_RETRY);
    if (!response.ok) throw await refusalOf(response, '');
    return response;
  }

  /**
   * Add one read to a shared stream under `key`. The door runs its whole
   * check for the op, as if it were called alone, and a refusal is thrown
   * as an `OrdoRefusal` while the stream stays open. A refinement rides
   * with the read as `refine` and meets the door's refinement checks (R6).
   */
  async addLiveRead(
    stream: string,
    key: string,
    name: string,
    options: Pick<DescriptorCallOptions, 'params' | 'signal' | 'refine'> = {},
  ): Promise<void> {
    const op = this.iface.requireOp(name);
    assertTerminal(op, 'live');
    standing(op);
    const headers: Record<string, string> = {
      'X-Ordo-Space': this.space,
      'X-Ordo-Version': op.version,
      'Content-Type': 'application/json',
      Accept: 'application/json',
    };
    const token = await this.#currentToken();
    if (token) headers['Authorization'] = `Bearer ${token}`;
    const params: Record<string, ParamValue> = {};
    for (const [k, v] of Object.entries(options.params ?? {})) {
      if (v === null || v === undefined) continue;
      params[k] = String(v);
    }
    const refine = liveRefine(op, options.refine);
    const init: RequestInit = {
      method: 'POST',
      headers,
      body: JSON.stringify({ key, op: op.op, params, ...(refine ? { refine } : {}) }),
    };
    if (options.signal) init.signal = options.signal;
    // Sent once: the live hub adds the read again on the same classes, with
    // the door's `Retry-After`, so a retry here would be a second loop.
    const response = await this.#send(`${this.#baseUrl}/live/${encodeURIComponent(stream)}`, init, NO_RETRY);
    if (!response.ok) throw await refusalOf(response, op.op, op.refusals);
    await readBody(response);
  }

  /**
   * A person's own download, `GET /download`: everything the app developer
   * marked `download` in the program that this person may read, as one zip
   * file (`docs/howto-leave.md`, "A person's own download"). Needs a token.
   * A refusal is thrown as an `OrdoRefusal`; `access_denied` names how the
   * app developer turns the download on. `useOwnDownload` wraps this.
   */
  async downloadOwn(options: { readonly signal?: AbortSignal | undefined } = {}): Promise<OwnDownload> {
    const token = await this.#currentToken();
    if (!token) {
      throw new OrdoLocalRefusal({
        class: 'access_denied',
        message: "a download is a signed-in person's own: this client has no token",
        op: DOWNLOAD_NAME,
        param: null,
      });
    }
    const headers: Record<string, string> = {
      Authorization: `Bearer ${token}`,
      'X-Ordo-Space': this.space,
      Accept: 'application/zip',
    };
    // Never through the browser's HTTP cache: the file is the person's own.
    const init: RequestInit = { method: 'GET', headers, cache: 'no-store' };
    if (options.signal) init.signal = options.signal;
    const response = await this.#send(`${this.#baseUrl}/download`, init, busyOnly(this.#busyRetry));
    if (!response.ok) throw await refusalOf(response, DOWNLOAD_NAME);
    return { blob: await response.blob(), name: downloadName(response.headers.get('Content-Disposition')) };
  }

  /** Drop one read from a shared stream. A key it does not carry is fine. */
  async dropLiveRead(stream: string, key: string): Promise<void> {
    const headers: Record<string, string> = {};
    const token = await this.#currentToken();
    if (token) headers['Authorization'] = `Bearer ${token}`;
    const url = `${this.#baseUrl}/live/${encodeURIComponent(stream)}/${encodeURIComponent(key)}`;
    await this.#send(url, { method: 'DELETE', headers }, busyOnly(this.#busyRetry));
  }

  async #currentToken(): Promise<string | undefined> {
    const t = this.#token;
    return t === undefined ? undefined : typeof t === 'function' ? await t() : t;
  }

  /** One call through the SDK's request helper. */
  #send(url: string, init: RequestInit, retry: RetryPolicy, onRetry?: OnRetry): Promise<Response> {
    return doorRequest(this.#site, url, init, { retry, onRetry });
  }

  async #call<R>(op: OpDescriptor, options: DescriptorCallOptions): Promise<OpResponse<R>> {
    // A standing error is knowable before the round trip. Raising it here
    // wears the descriptor's own class and message rather than inventing one.
    standing(op);

    const params = options.params ?? {};
    const write = op.kind !== 'read';
    // A public read is cached and shared, so it takes no refinement.
    if (op.public === true && options.refine) {
      throw new OrdoLocalRefusal({
        class: 'invalid_refinement',
        message: `${op.op} is a public op: a public read takes no $refine`,
        op: op.op,
        param: null,
      });
    }
    const checked = options.refine ? checkRefine(op, options.refine, 'door') : undefined;
    // An envelope that narrows nothing is not sent.
    const refine = checked && (checked.where || checked.pick) ? checked : undefined;

    const token = await this.#currentToken();
    // A tokenless public call names no space: the server uses the
    // deployment's own space, and naming a tenant's space is refused.
    const anonymousPublic = op.public === true && !token;
    const headers: Record<string, string> = {
      // The pin is **per-op** (§10) — a registry-wide pin marks every call
      // stale after any unrelated deploy, and a signal that fires constantly
      // is one clients learn to ignore.
      'X-Ordo-Version': op.version,
      Accept: 'application/json',
    };
    // Preferred over `$space` on every method: a space id in a query string
    // ends up in proxy logs and `Referer` headers (§2), and on POST the
    // header form costs nothing.
    if (!anonymousPublic) headers['X-Ordo-Space'] = this.space;
    if (token) headers['Authorization'] = `Bearer ${token}`;

    // Cursors are data, not addressing: the header form keeps them out of
    // proxy logs, browser history and `Referer` (SH-6).
    if (options.page) headers['X-Ordo-Page'] = options.page;

    // Honored on POST/DELETE only, and the door ignores it on reads — but
    // sending one on a read would still be a lie about what this call is.
    if (write) headers['Idempotency-Key'] = options.idempotencyKey ?? this.#newKey();

    let url = `${this.#baseUrl}/op/${encodeURIComponent(op.op)}`;
    let body: string | undefined;

    if (op.call.method === 'POST') {
      headers['Content-Type'] = 'application/json';
      body = JSON.stringify(refine ? { ...params, $refine: refine } : params);
    } else {
      // GET and DELETE carry params in the query string; `$refine` rides as
      // URL-encoded JSON there (§2).
      const q = new URLSearchParams();
      for (const [k, v] of Object.entries(params)) {
        if (v === null || v === undefined) continue;
        q.set(k, String(v));
      }
      if (refine) q.set('$refine', JSON.stringify(refine));
      const qs = q.toString();
      if (qs) url += `?${qs}`;
    }

    const init: RequestInit = { method: op.call.method, headers };
    if (body !== undefined) init.body = body;
    if (options.signal) init.signal = options.signal;

    const response = await this.#send(
      url,
      init,
      { busy: this.#busyRetry, conflict: this.#conflictRetry },
      options.onRetry,
    );
    if (!response.ok) throw await refusalOf(response, op.op, op.refusals);
    return opResponseOf<R>(response, await readBody(response), op.op, op.version);
  }
}

/** A standing registration failure, refused before the wire in the descriptor's own words. */
function standing(op: OpDescriptor): void {
  if (!op.error) return;
  throw new OrdoLocalRefusal({
    class: op.error.code as RefusalClass,
    message: op.error.message,
    op: op.op,
    param: null,
  });
}

/**
 * The terminal-verb contract (§B3), enforced at runtime because a descriptor
 * loaded at runtime cannot be enforced at compile time. Reads terminate in
 * `fetch`/`live`/`pages`; writes terminate in `commit` and have no read
 * verbs. Every mutation in an app is still `grep 'commit('`.
 */
export function assertTerminal(op: OpDescriptor, verb: 'fetch' | 'live' | 'pages' | 'commit'): void {
  if (op.call.terminal.includes(verb)) return;
  throw new OrdoLocalRefusal({
    class: 'method_mismatch',
    message:
      `${op.op} is a ${op.kind} op: it terminates in ${op.call.terminal.map((v) => `.${v}()`).join(' / ')}, ` +
      `not .${verb}(). The wire method (${op.call.method}) follows from the kind, so this call could ` +
      `only have been refused at the door.`,
    op: op.op,
    param: null,
  });
}

/** The param, or `undefined`. */
export function paramOf(op: OpDescriptor, name: string): ParamDescriptor | undefined {
  return op.params.find((p) => p.name === name);
}

/** A live read's refinement in its wire form, or `undefined` when it narrows nothing. */
function liveRefine(op: OpDescriptor, refine: Refine | undefined): Refine | undefined {
  const checked = refine ? checkRefine(op, refine, 'door') : undefined;
  return checked && (checked.where || checked.pick) ? checked : undefined;
}
